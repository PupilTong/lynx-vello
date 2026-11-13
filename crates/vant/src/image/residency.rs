//! Image residency: what is known about every image source a host was
//! asked for, who waits for it, and the decoded-bitmap tier — the decisions
//! an image pipeline makes between a request and a frame's read, with none
//! of its IO.
//!
//! [`Residency`] is synchronous and runs nothing. The host asks it what a
//! request means ([`Residency::request`]), starts the load it is told to
//! start, feeds every completion back ([`Residency::loaded`],
//! [`Residency::parsed`], [`Residency::failed`], [`Residency::refined`], …)
//! and tells the waiters each completion hands back. A frame's read
//! ([`Residency::read`]) answers in one of three ways:
//!
//! - the resident bitmap is about the right size, and is returned;
//! - the resident bitmap is far larger (or smaller, with more to give) than the draw, and is
//!   returned with a [`Refinement`] at the drawn size for the host to start off its thread; the
//!   refined bitmap replaces it when [`Residency::refined`] is called;
//! - the bitmap was evicted, and the read answers with the [`Refinement`] that restores it — the
//!   encoded bytes when the entry kept them, the URL to read them back from otherwise — which the
//!   host runs synchronously and answers with [`Residency::restored`], because a read after a
//!   reported load must not miss.
//!
//! `W` is whoever waits for a source: the engine's per-view report handle.
//! The residency stores waiters and hands them back; it never calls them.
//!
//! # Entries
//!
//! A document a page wrote itself arrives as a document request
//! ([`Residency::request_document`]) and **files no entry at all**, before,
//! during or after its parse: two requests for one source are two parses and
//! two answers, and each request leaves the in-flight list when a parse of
//! its source completes. A fetched document leaves `Entry::Parsed`, a
//! marker that holds nothing and keeps the source known; a later request for
//! it fetches and parses it again. Documents are never retained: nothing of
//! one enters the bitmap tier, [`Residency::read`] answers
//! [`Read::Unavailable`] for it, and no refinement and no restore can start
//! for it.
//!
//! The document's state never regresses, and a bitmap's does not here: an
//! entry that failed stays failed, and one that loaded as a bitmap stays
//! loaded whatever the bitmap tier holds for it. The parsed marker is the
//! one entry a later request starts over. A completion for an entry that is
//! not in the state it completes is ignored.
//!
//! # Working sets
//!
//! Each view says which sources its frame draws ([`Residency::retain`]), and
//! the bitmap tier pins the union of every view's set: one view's frame
//! never unpins another's. A view that goes away says so
//! ([`Residency::release_view`]), and its set stops counting.

use std::sync::Arc;

use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;
use vello_common::peniko::ImageData;

use crate::image::{Bitmap, ImageSizeHint, bounded, wants_refinement};
use crate::image_header::ImageHeader;
use crate::memory::MemoryCache;
use crate::mime::ImageFormat;

/// What a [`Residency`] is built with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResidencyConfig {
    /// The bitmap tier's budget for decoded bitmaps. Best-effort: the frames
    /// being drawn are never evicted, and a single bitmap larger than the
    /// budget still has to exist while it is on screen.
    pub budget_bytes: usize,
    /// The largest per-axis size of an image's first decode, before any
    /// frame has said how large it draws it. Also bounded by
    /// [`Self::max_renderable`].
    pub initial_bound: u32,
    /// The largest axis the caller's rasterizer draws: no decode is asked
    /// for more.
    pub max_renderable: u32,
    /// A resident bitmap at least this many times larger than its draw on
    /// both axes is re-decoded at the drawn size.
    pub downsample_ratio: f32,
}

/// One view's name for its working set, minted by [`Residency::new_view`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewKey(u64);

/// What a request for a source means for the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    /// Already a bitmap: the caller reports its intrinsic size now.
    Loaded { intrinsic: (u32, u32) },
    /// Failed before, and failed for good: the caller reports the failure
    /// now.
    Failed,
    /// A load is in flight; the waiter joined it.
    Joined,
    /// Nothing known, or a document parsed before: the caller starts a load
    /// decoding at most `bound`. The waiter is filed with it.
    Start { bound: (u32, u32) },
}

/// A raster load's outcome: the bitmap, what the bytes were, and the bytes
/// themselves when nothing else could restore them.
#[derive(Debug)]
pub struct Decoded {
    pub url: Url,
    pub bitmap: Bitmap,
    pub format: ImageFormat,
    pub header: Option<ImageHeader>,
    /// The encoded bytes, kept when no other tier can hand them back.
    pub encoded: Option<Bytes>,
}

/// What a refinement re-decodes, and at what size. A restore inside a read
/// ([`Read::Restore`]) re-decodes the same things.
#[derive(Clone, Debug)]
pub struct Refinement {
    pub url: Url,
    /// The entry's own bytes, when it kept them; otherwise another tier is
    /// asked for them again.
    pub encoded: Option<Bytes>,
    pub format: ImageFormat,
    pub header: Option<ImageHeader>,
    pub target: (u32, u32),
}

/// What a frame's read of a source finds.
#[derive(Debug)]
pub enum Read {
    /// The resident bitmap, and the refinement the caller starts when it is
    /// far from the drawn size and none is in flight.
    Hit {
        image: ImageData,
        refine: Option<Refinement>,
    },
    /// Loaded, and evicted: the caller restores it now, synchronously, and
    /// answers with [`Residency::restored`].
    Restore(Refinement),
    /// Not a loaded bitmap: loading, failed, a document, or never asked for.
    Unavailable,
}

/// What is known about every source asked for, and the decoded bitmaps.
pub struct Residency<W> {
    entries: FxHashMap<Arc<str>, Entry<W>>,
    bitmaps: MemoryCache<ImageData>,
    /// Document requests whose parse is in flight, each with the waiter
    /// that asked: where a parse's answer goes, since a waiter cannot travel
    /// with the job. Not an entry: two requests for one source are two of
    /// these, two parses and two answers, and each leaves the list when a
    /// parse of its source completes.
    requested: Vec<(Arc<str>, W)>,
    /// Each view's working set; the bitmap tier pins their union.
    views: FxHashMap<ViewKey, Vec<Arc<str>>>,
    next_view: u64,
    config: ResidencyConfig,
}

impl<W> std::fmt::Debug for Residency<W> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Residency")
            .field("entries", &self.entries.len())
            .field("bitmaps", &self.bitmaps)
            .field("requested", &self.requested.len())
            .field("views", &self.views.len())
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

enum Entry<W> {
    /// Asked for; the job has not completed. Every waiter that asked is told
    /// when it does.
    Loading {
        waiters: Vec<W>,
    },
    /// A decoded raster image.
    Loaded(Loaded),
    /// A fetched document that parsed and was reported to every waiter that
    /// asked. Nothing of it is kept, since the engine owns the scene it
    /// encodes; the marker keeps the source known, and a later request
    /// fetches and parses it again. It has no bitmap, so the bitmap tier,
    /// refinement and restore never see it.
    Parsed,
    Failed,
}

struct Loaded {
    url: Url,
    intrinsic: (u32, u32),
    /// The container, for a decoder that wants to be told what it is given.
    /// Never [`ImageFormat::Svg`]: an SVG document completes as
    /// [`Entry::Parsed`], so no restore or refinement ever decodes one.
    format: ImageFormat,
    header: Option<ImageHeader>,
    /// The encoded bytes, kept when no other tier can hand them back: a
    /// registration that may be cleared, a `data:` URL, a response the byte
    /// tier did not store, and everything in the browser.
    encoded: Option<Bytes>,
    /// A refinement decode in flight, and the size it targets.
    refining: Option<(u32, u32)>,
}

impl<W> Residency<W> {
    #[must_use]
    pub fn new(config: ResidencyConfig) -> Self {
        Self {
            entries: FxHashMap::default(),
            bitmaps: MemoryCache::new(config.budget_bytes),
            requested: Vec::new(),
            views: FxHashMap::default(),
            next_view: 0,
            config,
        }
    }

    /// A key for one more view's working set.
    pub fn new_view(&mut self) -> ViewKey {
        self.next_view += 1;
        ViewKey(self.next_view)
    }

    /// Names `source` for `waiter`: what is known answers at once, a load
    /// in flight is joined, and otherwise the caller is told to start the
    /// load and `waiter` is filed with it. A document parsed before is
    /// fetched and parsed again.
    pub fn request(&mut self, source: &str, waiter: W) -> Requested {
        match self.entries.get_mut(source) {
            Some(Entry::Loaded(loaded)) => {
                return Requested::Loaded {
                    intrinsic: loaded.intrinsic,
                };
            }
            Some(Entry::Failed) => return Requested::Failed,
            Some(Entry::Loading { waiters }) => {
                waiters.push(waiter);
                return Requested::Joined;
            }
            Some(Entry::Parsed) | None => {}
        }
        self.entries.insert(
            Arc::from(source),
            Entry::Loading {
                waiters: vec![waiter],
            },
        );
        let bound = self.config.initial_bound.min(self.config.max_renderable);
        Requested::Start {
            bound: (bound, bound),
        }
    }

    /// Files a document request for `source` whose parse the caller starts.
    /// Nothing is consulted and no entry is filed: every request is one
    /// parse and one answer.
    pub fn request_document(&mut self, source: Arc<str>, waiter: W) {
        self.requested.push((source, waiter));
    }

    /// `source`'s load decoded: the bitmap enters the tier and the entry is
    /// loaded. Answers the waiters to tell and the intrinsic size to tell
    /// them, or `None` when no load of `source` was in flight.
    pub fn loaded(&mut self, source: &Arc<str>, decoded: Decoded) -> Option<(Vec<W>, (u32, u32))> {
        let Decoded {
            url,
            bitmap,
            format,
            header,
            encoded,
        } = decoded;
        let intrinsic = (bitmap.source_width, bitmap.source_height);
        let waiters = self.settle(
            source,
            Entry::Loaded(Loaded {
                url,
                intrinsic,
                format,
                header,
                encoded,
                refining: None,
            }),
        )?;
        let bytes = bitmap.byte_len();
        self.bitmaps
            .insert(Arc::clone(source), bitmap.into_image_data(), bytes);
        Some((waiters, intrinsic))
    }

    /// `source`'s load was a document, and it parsed: the entry becomes the
    /// parsed marker. Answers the waiters to tell, or `None` when no load of
    /// `source` was in flight.
    pub fn parsed(&mut self, source: &str) -> Option<Vec<W>> {
        self.settle(source, Entry::Parsed)
    }

    /// A parse of a document request for `source` completed: answers the
    /// waiter of one request for it, or `None` when none is in flight. Any
    /// request for this source will do: the source names its markup, so
    /// every parse of it is the same document.
    pub fn requested_document_done(&mut self, source: &str) -> Option<W> {
        let index = self
            .requested
            .iter()
            .position(|(requested, _)| &**requested == source)?;
        Some(self.requested.swap_remove(index).1)
    }

    /// `source`'s load failed, or it could not even be started: the entry
    /// fails for good. Answers the waiters to tell, or `None` when no load
    /// of `source` was in flight.
    pub fn failed(&mut self, source: &str) -> Option<Vec<W>> {
        self.settle(source, Entry::Failed)
    }

    /// A refinement of `source` at `target` decoded: the bitmap replaces the
    /// resident one, unless the entry is no longer refining to `target`.
    pub fn refined(&mut self, source: &Arc<str>, target: (u32, u32), bitmap: Bitmap) {
        let Some(Entry::Loaded(loaded)) = self.entries.get_mut(source) else {
            return;
        };
        if loaded.refining != Some(target) {
            return;
        }
        loaded.refining = None;
        let bytes = bitmap.byte_len();
        self.bitmaps
            .insert(Arc::clone(source), bitmap.into_image_data(), bytes);
    }

    /// A refinement of `source` failed. It leaves the loaded entry alone and
    /// only clears `refining`, so a later read may plan another.
    pub fn refine_failed(&mut self, source: &str) {
        if let Some(Entry::Loaded(loaded)) = self.entries.get_mut(source) {
            loaded.refining = None;
        }
    }

    /// The frame-side read of `source`, for a draw of `hint`.
    pub fn read(&mut self, source: &str, hint: ImageSizeHint) -> Read {
        let loaded = match self.entries.get_mut(source) {
            Some(Entry::Loaded(loaded)) => loaded,
            // The engine draws a document itself and never reads it here;
            // answering `Unavailable` is what keeps every restore and
            // refinement below unreachable for an SVG document.
            Some(Entry::Parsed | Entry::Loading { .. } | Entry::Failed) | None => {
                return Read::Unavailable;
            }
        };
        let target = bounded(
            hint.fit(loaded.intrinsic.0, loaded.intrinsic.1),
            self.config.max_renderable,
        );
        if let Some(image) = self.bitmaps.get(source) {
            let image = image.clone();
            let refine = (loaded.refining.is_none()
                && wants_refinement(
                    (image.width, image.height),
                    target,
                    loaded.intrinsic,
                    self.config.downsample_ratio,
                ))
            .then(|| {
                loaded.refining = Some(target);
                Refinement {
                    url: loaded.url.clone(),
                    encoded: loaded.encoded.clone(),
                    format: loaded.format,
                    header: loaded.header,
                    target,
                }
            });
            return Read::Hit { image, refine };
        }
        // Evicted: the caller restores it now, on its own thread. The
        // contract is that a reported load never misses.
        Read::Restore(Refinement {
            url: loaded.url.clone(),
            encoded: loaded.encoded.clone(),
            format: loaded.format,
            header: loaded.header,
            target,
        })
    }

    /// The bitmap a [`Read::Restore`] decoded for `source`: it re-enters the
    /// tier while the entry is loaded, and is what the read answers.
    pub fn restored(&mut self, source: &str, bitmap: Bitmap) -> ImageData {
        let bytes = bitmap.byte_len();
        let image = bitmap.into_image_data();
        if matches!(self.entries.get(source), Some(Entry::Loaded(_))) {
            self.bitmaps.insert(Arc::from(source), image.clone(), bytes);
        }
        image
    }

    /// Records `view`'s working set, replacing what it said before. The
    /// bitmap tier pins the union of every view's set, so those bitmaps are
    /// never evicted.
    pub fn retain(&mut self, view: ViewKey, frame: &[Arc<str>]) {
        self.views.insert(view, frame.to_vec());
        self.pin();
    }

    /// Forgets `view`'s working set: what only it pinned may be evicted.
    pub fn release_view(&mut self, view: ViewKey) {
        if self.views.remove(&view).is_some() {
            self.pin();
        }
    }

    #[must_use]
    pub fn budget(&self) -> usize {
        self.bitmaps.budget()
    }

    /// Changes the bitmap budget, evicting what no longer fits.
    pub fn set_budget(&mut self, budget: usize) {
        self.bitmaps.set_budget(budget);
    }

    /// Bytes held by decoded bitmaps.
    #[must_use]
    pub fn bitmap_bytes(&self) -> usize {
        self.bitmaps.used_bytes()
    }

    /// Bytes held by encoded images nothing else can restore.
    #[must_use]
    pub fn encoded_bytes(&self) -> usize {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                Entry::Loaded(loaded) => loaded.encoded.as_ref().map(Bytes::len),
                Entry::Loading { .. } | Entry::Parsed | Entry::Failed => None,
            })
            .sum()
    }

    /// Whether `source` has been asked for at all. A document request does
    /// not count: it files no entry.
    #[must_use]
    pub fn knows(&self, source: &str) -> bool {
        self.entries.contains_key(source)
    }

    /// Whether `source` has a resident bitmap.
    #[must_use]
    pub fn is_resident(&self, source: &str) -> bool {
        self.bitmaps.contains(source)
    }

    /// The resident bitmap's size, if any.
    #[must_use]
    pub fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        self.bitmaps
            .peek(source)
            .map(|image| (image.width, image.height))
    }

    /// Moves a loading entry to `next`, answering its waiters; leaves any
    /// other entry alone and answers `None`.
    fn settle(&mut self, source: &str, next: Entry<W>) -> Option<Vec<W>> {
        let entry = self.entries.get_mut(source)?;
        let Entry::Loading { waiters } = entry else {
            return None;
        };
        let waiters = std::mem::take(waiters);
        *entry = next;
        Some(waiters)
    }

    /// Pins the union of every view's working set, which trims what is no
    /// longer pinned.
    fn pin(&mut self) {
        let union: Vec<Arc<str>> = self.views.values().flatten().cloned().collect();
        let _ = self.bitmaps.pin(&union);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u32 = 8192;

    fn residency(budget_bytes: usize) -> Residency<&'static str> {
        Residency::new(ResidencyConfig {
            budget_bytes,
            initial_bound: 2048,
            max_renderable: MAX,
            downsample_ratio: 2.0,
        })
    }

    fn bitmap(width: u32, height: u32, intrinsic: (u32, u32)) -> Bitmap {
        Bitmap {
            width,
            height,
            source_width: intrinsic.0,
            source_height: intrinsic.1,
            premultiplied: false,
            rgba: vec![0; width as usize * height as usize * 4],
        }
    }

    fn url(source: &str) -> Url {
        Url::parse(&format!("app:///{source}")).expect("url")
    }

    fn decoded(source: &str, bitmap: Bitmap, encoded: Option<&'static [u8]>) -> Decoded {
        Decoded {
            url: url(source),
            bitmap,
            format: ImageFormat::Png,
            header: None,
            encoded: encoded.map(Bytes::from_static),
        }
    }

    /// Requests `source` for `waiter` and completes its load with `bitmap`.
    fn load(
        residency: &mut Residency<&'static str>,
        source: &str,
        bitmap: Bitmap,
        encoded: Option<&'static [u8]>,
    ) {
        assert!(matches!(
            residency.request(source, "loader"),
            Requested::Start { .. }
        ));
        let (waiters, _) = residency
            .loaded(&Arc::from(source), decoded(source, bitmap, encoded))
            .expect("in flight");
        assert_eq!(waiters, ["loader"]);
    }

    #[test]
    fn a_request_while_loading_joins_and_the_load_answers_every_waiter() {
        let mut residency = residency(1 << 20);
        assert_eq!(
            residency.request("a.png", "first"),
            Requested::Start {
                bound: (2048, 2048)
            }
        );
        assert!(residency.knows("a.png"));
        assert!(!residency.is_resident("a.png"));
        assert_eq!(residency.request("a.png", "second"), Requested::Joined);

        let (waiters, intrinsic) = residency
            .loaded(
                &Arc::from("a.png"),
                decoded("a.png", bitmap(4, 2, (40, 20)), None),
            )
            .expect("in flight");
        assert_eq!(waiters, ["first", "second"]);
        assert_eq!(intrinsic, (40, 20));
        assert_eq!(residency.resident_size("a.png"), Some((4, 2)));
        assert_eq!(
            residency.request("a.png", "third"),
            Requested::Loaded {
                intrinsic: (40, 20)
            },
            "a loaded source answers at once"
        );
        assert!(
            residency
                .loaded(
                    &Arc::from("a.png"),
                    decoded("a.png", bitmap(1, 1, (1, 1)), None)
                )
                .is_none(),
            "a second completion finds no load in flight and changes nothing"
        );
        assert_eq!(residency.resident_size("a.png"), Some((4, 2)));
    }

    #[test]
    fn the_first_decode_is_bounded_by_what_the_rasterizer_draws() {
        let mut residency = Residency::new(ResidencyConfig {
            budget_bytes: 0,
            initial_bound: 100_000,
            max_renderable: MAX,
            downsample_ratio: 2.0,
        });
        assert_eq!(
            residency.request("a.png", ()),
            Requested::Start { bound: (MAX, MAX) }
        );
    }

    #[test]
    fn a_failed_source_answers_failed_forever() {
        let mut residency = residency(1 << 20);
        residency.request("a.png", "first");
        residency.request("a.png", "second");
        assert_eq!(residency.failed("a.png"), Some(vec!["first", "second"]));
        assert_eq!(residency.request("a.png", "third"), Requested::Failed);
        assert!(residency.knows("a.png"));
        assert!(
            residency
                .loaded(
                    &Arc::from("a.png"),
                    decoded("a.png", bitmap(1, 1, (1, 1)), None)
                )
                .is_none(),
            "a late load does not revive a failure"
        );
        assert!(residency.parsed("a.png").is_none());
        assert!(residency.failed("a.png").is_none());
        assert_eq!(residency.request("a.png", "fourth"), Requested::Failed);
        assert!(matches!(
            residency.read("a.png", ImageSizeHint::UNBOUNDED),
            Read::Unavailable
        ));
    }

    #[test]
    fn a_parsed_document_keeps_nothing_and_is_requested_again_as_a_start() {
        let mut residency = residency(1 << 20);
        residency.request("icon.svg", "first");
        assert_eq!(residency.parsed("icon.svg"), Some(vec!["first"]));
        assert!(residency.knows("icon.svg"), "the marker keeps it known");
        assert!(!residency.is_resident("icon.svg"));
        assert_eq!(residency.bitmap_bytes(), 0);
        assert!(matches!(
            residency.read("icon.svg", ImageSizeHint::UNBOUNDED),
            Read::Unavailable
        ));
        assert!(matches!(
            residency.request("icon.svg", "second"),
            Requested::Start { .. }
        ));
        assert_eq!(residency.request("icon.svg", "third"), Requested::Joined);
    }

    #[test]
    fn a_document_request_files_no_entry_and_answers_once_per_request() {
        let mut residency = residency(1 << 20);
        residency.request_document(Arc::from("svg-content:a"), "first");
        residency.request_document(Arc::from("svg-content:a"), "second");
        assert!(!residency.knows("svg-content:a"));
        let mut answered = vec![
            residency
                .requested_document_done("svg-content:a")
                .expect("first parse"),
            residency
                .requested_document_done("svg-content:a")
                .expect("second parse"),
        ];
        answered.sort_unstable();
        assert_eq!(answered, ["first", "second"]);
        assert!(residency.requested_document_done("svg-content:a").is_none());
        assert!(!residency.knows("svg-content:a"));
    }

    #[test]
    fn only_a_loaded_bitmap_is_readable() {
        let mut residency = residency(1 << 20);
        residency.request("loading.png", "waiter");
        residency.request("failed.png", "waiter");
        residency.failed("failed.png");
        residency.request("parsed.svg", "waiter");
        residency.parsed("parsed.svg");
        for source in ["loading.png", "failed.png", "parsed.svg", "unknown.png"] {
            assert!(
                matches!(
                    residency.read(source, ImageSizeHint::UNBOUNDED),
                    Read::Unavailable
                ),
                "{source}"
            );
        }
        assert!(residency.parsed("unknown.png").is_none());
        assert!(residency.failed("unknown.png").is_none());
        residency.refined(&Arc::from("unknown.png"), (1, 1), bitmap(1, 1, (1, 1)));
        residency.refine_failed("unknown.png");
        assert!(!residency.knows("unknown.png"));
    }

    #[test]
    fn a_hit_far_larger_than_the_draw_plans_exactly_one_refinement() {
        let mut residency = residency(1 << 30);
        load(
            &mut residency,
            "big.png",
            bitmap(1000, 1000, (4000, 4000)),
            Some(b"bytes"),
        );
        let hint = ImageSizeHint::new(125, 125);
        let Read::Hit {
            image,
            refine: Some(refinement),
        } = residency.read("big.png", hint)
        else {
            panic!("a hit with a refinement");
        };
        assert_eq!((image.width, image.height), (1000, 1000));
        assert_eq!(refinement.target, (125, 125));
        assert_eq!(refinement.url, url("big.png"));
        assert_eq!(refinement.encoded.as_deref(), Some(&b"bytes"[..]));
        assert!(
            matches!(
                residency.read("big.png", hint),
                Read::Hit { refine: None, .. }
            ),
            "not a second while one is in flight"
        );

        residency.refined(
            &Arc::from("big.png"),
            (250, 250),
            bitmap(250, 250, (4000, 4000)),
        );
        assert_eq!(
            residency.resident_size("big.png"),
            Some((1000, 1000)),
            "a refinement for a stale target is ignored"
        );
        residency.refined(
            &Arc::from("big.png"),
            (125, 125),
            bitmap(125, 125, (4000, 4000)),
        );
        assert_eq!(residency.resident_size("big.png"), Some((125, 125)));
        assert!(
            matches!(
                residency.read("big.png", hint),
                Read::Hit { refine: None, .. }
            ),
            "the refined bitmap is the drawn size"
        );
    }

    #[test]
    fn a_failed_refinement_keeps_the_bitmap_and_lets_a_later_read_plan_again() {
        let mut residency = residency(1 << 30);
        load(
            &mut residency,
            "big.png",
            bitmap(1000, 1000, (4000, 4000)),
            None,
        );
        let hint = ImageSizeHint::new(125, 125);
        assert!(matches!(
            residency.read("big.png", hint),
            Read::Hit {
                refine: Some(_),
                ..
            }
        ));
        residency.refine_failed("big.png");
        assert_eq!(residency.resident_size("big.png"), Some((1000, 1000)));
        assert!(matches!(
            residency.read("big.png", hint),
            Read::Hit {
                refine: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn an_evicted_bitmap_is_restored_from_kept_bytes_or_its_url() {
        let mut residency = residency(1 << 20);
        load(
            &mut residency,
            "kept.png",
            bitmap(2, 2, (2, 2)),
            Some(b"kept"),
        );
        load(&mut residency, "fetched.png", bitmap(2, 2, (2, 2)), None);
        residency.set_budget(0);
        assert!(!residency.is_resident("kept.png"));
        assert!(!residency.is_resident("fetched.png"));
        assert!(
            residency.knows("kept.png"),
            "eviction never unloads an entry"
        );

        let Read::Restore(plan) = residency.read("kept.png", ImageSizeHint::UNBOUNDED) else {
            panic!("a restore");
        };
        assert_eq!(plan.encoded.as_deref(), Some(&b"kept"[..]));
        assert_eq!(plan.target, (2, 2));
        let Read::Restore(plan) = residency.read("fetched.png", ImageSizeHint::new(1, 1)) else {
            panic!("a restore");
        };
        assert!(plan.encoded.is_none());
        assert_eq!(plan.url, url("fetched.png"));
        assert_eq!(plan.target, (1, 1), "restored at the drawn size");

        let image = residency.restored("fetched.png", bitmap(1, 1, (2, 2)));
        assert_eq!((image.width, image.height), (1, 1));
        assert!(
            residency.is_resident("fetched.png"),
            "the restore re-enters the tier, its own insert spared"
        );
        assert!(matches!(
            residency.read("fetched.png", ImageSizeHint::new(1, 1)),
            Read::Hit { .. }
        ));
        let _ = residency.restored("unknown.png", bitmap(1, 1, (1, 1)));
        assert!(!residency.is_resident("unknown.png"));
    }

    #[test]
    fn a_read_never_asks_for_more_than_the_rasterizer_draws() {
        let mut residency = residency(0);
        load(
            &mut residency,
            "huge.png",
            bitmap(1, 1, (20_000, 10_000)),
            None,
        );
        residency.set_budget(0);
        let Read::Restore(plan) = residency.read("huge.png", ImageSizeHint::UNBOUNDED) else {
            panic!("a restore");
        };
        assert_eq!(plan.target, (MAX, MAX / 2));
    }

    #[test]
    fn budget_accounting_counts_bitmaps_and_kept_bytes() {
        let mut residency = residency(1 << 20);
        assert_eq!(residency.budget(), 1 << 20);
        load(&mut residency, "a.png", bitmap(2, 2, (2, 2)), Some(b"abc"));
        load(&mut residency, "b.png", bitmap(1, 1, (1, 1)), None);
        assert_eq!(residency.bitmap_bytes(), 2 * 2 * 4 + 4);
        assert_eq!(residency.encoded_bytes(), 3);
        residency.set_budget(4);
        assert_eq!(residency.budget(), 4);
        assert_eq!(residency.bitmap_bytes(), 4, "the oldest went first");
        assert!(!residency.is_resident("a.png"));
        assert_eq!(
            residency.encoded_bytes(),
            3,
            "kept bytes stay with the entry when its bitmap goes"
        );
    }

    #[test]
    fn two_views_working_sets_are_both_pinned() {
        let mut residency = residency(1 << 20);
        load(&mut residency, "a.png", bitmap(2, 2, (2, 2)), None);
        load(&mut residency, "b.png", bitmap(2, 2, (2, 2)), None);
        let first = residency.new_view();
        let second = residency.new_view();
        assert_ne!(first, second);
        residency.retain(first, &[Arc::from("a.png")]);
        residency.retain(second, &[Arc::from("b.png")]);
        residency.set_budget(0);
        assert!(
            residency.is_resident("a.png"),
            "the second view's frame does not unpin the first view's"
        );
        assert!(residency.is_resident("b.png"));

        residency.retain(first, &[]);
        assert!(
            !residency.is_resident("a.png"),
            "a view's new set replaces its own old one"
        );
        assert!(residency.is_resident("b.png"));

        residency.release_view(second);
        assert!(
            !residency.is_resident("b.png"),
            "a released view's set stops counting"
        );
        residency.release_view(second);
    }
}
