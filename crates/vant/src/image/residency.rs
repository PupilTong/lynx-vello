//! Image residency: what is known about every image source a host was
//! asked for, who waits for it, and the decoded-bitmap tier — the decisions
//! an image pipeline makes between a request and a frame's draw, with none
//! of its IO.
//!
//! [`Residency`] is synchronous and runs nothing. The host asks it what a
//! request means ([`Residency::request`]), starts the load it is told to
//! start, feeds every completion back ([`Residency::loaded`],
//! [`Residency::parsed`], [`Residency::failed`], [`Residency::refined`], …)
//! and tells the waiters each completion hands back. A frame asks what it
//! can draw for a source ([`Residency::describe`]), which never starts a
//! decode the frame waits for, and answers in one of three ways:
//!
//! - the resident bitmap is about the right size, and is returned;
//! - the resident bitmap is far larger (or smaller, with more to give) than the draw, and is
//!   returned with a [`Refinement`] at the drawn size for the host to start off its thread; the
//!   refined bitmap replaces it when [`Residency::refined`] is called;
//! - the tier no longer holds the bitmap — released or evicted — and the answer is the
//!   [`ImageDescriptor`] of the bitmap last inserted for the source: enough for a rasterizer that
//!   still holds an upload of that bitmap to draw it.
//!
//! A caller whose rasterizer holds the upload a frame needs of a source
//! tells the tier so ([`Residency::release`]), and the tier drops its copy,
//! pinned or not. A caller whose rasterizer holds no upload either says the
//! source is wanted ([`Residency::want`]), and is answered with the
//! [`Refinement`] that restores it — the encoded bytes when the entry kept
//! them, the URL to read them back from otherwise — which the host runs off
//! the frame's thread, like any refinement, and answers with
//! [`Residency::refined`] or [`Residency::refine_failed`]. Until then the
//! frame draws nothing for the source. A restore that fails leaves the
//! source with nothing to draw: it is described as unavailable from then
//! on, and nothing retries it.
//!
//! `W` is whoever waits for a source: the engine's per-view report handle.
//! The residency stores waiters and hands them back; it never calls them.
//!
//! `F` is the decoder's platform frame type ([`crate::image::PictureFrame`]),
//! [`Infallible`] for a decoder that answers CPU pixels only. A platform
//! frame is held, released, evicted and restored exactly as CPU pixels are:
//! the tier holds it until the caller's rasterizer holds its upload, then
//! drops it — which is what lets the platform free it — keeps the
//! descriptor minted for it, and decodes the source again when it is wanted.
//! Its bytes are counted from its size, four a pixel.
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
//! one enters the bitmap tier, [`Residency::describe`] answers
//! [`Described::Unavailable`] for it, and no refinement and no restore can
//! start for it.
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

use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use rustc_hash::FxHashMap;
use url::Url;

use crate::image::{
    DecodedImage, ImageDescriptor, ImageSizeHint, PictureFrame, bounded, wants_refinement,
};
use crate::image_header::ImageHeader;
use crate::memory::MemoryCache;
use crate::mime::ImageFormat;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResidencyConfig {
    pub budget_bytes: usize,
    pub initial_bound: u32,
    pub max_renderable: u32,
    pub downsample_ratio: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewKey(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    Loaded {
        intrinsic: (u32, u32),
        animated: bool,
    },
    Failed,
    Joined,
    Start {
        bound: (u32, u32),
    },
}

#[derive(Debug)]
pub struct Decoded<F = Infallible> {
    pub url: Url,
    pub bitmap: DecodedImage<F>,
    pub format: ImageFormat,
    pub header: Option<ImageHeader>,
    pub encoded: Option<Bytes>,
    pub animated: bool,
}

#[derive(Clone, Debug)]
pub struct Refinement {
    pub url: Url,
    pub encoded: Option<Bytes>,
    pub format: ImageFormat,
    pub header: Option<ImageHeader>,
    pub target: (u32, u32),
}

#[derive(Debug)]
pub enum Described<F = Infallible> {
    Resident {
        image: PictureFrame<F>,
        descriptor: ImageDescriptor,
        refine: Option<Refinement>,
    },
    Absent {
        descriptor: ImageDescriptor,
        refine: Option<Refinement>,
    },
    Unavailable,
}

pub struct Residency<W, F = Infallible> {
    entries: FxHashMap<Arc<str>, Entry<W>>,
    bitmaps: MemoryCache<PictureFrame<F>>,
    requested: Vec<(Arc<str>, W)>,
    views: FxHashMap<ViewKey, Vec<Arc<str>>>,
    next_view: u64,
    config: ResidencyConfig,
}

impl<W, F> std::fmt::Debug for Residency<W, F> {
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

type Found<'a, F> = (&'a mut Loaded, (u32, u32), Option<PictureFrame<F>>);

enum Entry<W> {
    Loading { waiters: Vec<W> },
    Loaded(Loaded),
    Parsed,
    Failed,
}

struct Loaded {
    url: Url,
    intrinsic: (u32, u32),
    animated: bool,
    format: ImageFormat,
    header: Option<ImageHeader>,
    encoded: Option<Bytes>,
    refining: Option<(u32, u32)>,
    unrestorable: bool,
    descriptor: ImageDescriptor,
}

impl Loaded {
    fn decode_at(&self, target: (u32, u32)) -> Refinement {
        Refinement {
            url: self.url.clone(),
            encoded: self.encoded.clone(),
            format: self.format,
            header: self.header,
            target,
        }
    }

    fn plan(&mut self, size: (u32, u32), target: (u32, u32), ratio: f32) -> Option<Refinement> {
        (self.refining.is_none() && wants_refinement(size, target, self.intrinsic, ratio)).then(
            || {
                self.refining = Some(target);
                self.decode_at(target)
            },
        )
    }
}

impl<W, F: Clone> Residency<W, F> {
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

    pub fn new_view(&mut self) -> ViewKey {
        self.next_view += 1;
        ViewKey(self.next_view)
    }

    pub fn request(&mut self, source: &str, waiter: W) -> Requested {
        match self.entries.get_mut(source) {
            Some(Entry::Loaded(loaded)) => {
                return Requested::Loaded {
                    intrinsic: loaded.intrinsic,
                    animated: loaded.animated,
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

    pub fn request_document(&mut self, source: Arc<str>, waiter: W) {
        self.requested.push((source, waiter));
    }

    pub fn loaded(
        &mut self,
        source: &Arc<str>,
        decoded: Decoded<F>,
    ) -> Option<(Vec<W>, (u32, u32))> {
        let Decoded {
            url,
            bitmap,
            format,
            header,
            encoded,
            animated,
        } = decoded;
        let intrinsic = bitmap.source_size();
        let bytes = bitmap.byte_len();
        let (image, descriptor) = bitmap.into_frame();
        let waiters = self.settle(
            source,
            Entry::Loaded(Loaded {
                url,
                intrinsic,
                animated,
                format,
                header,
                encoded,
                refining: None,
                unrestorable: false,
                descriptor,
            }),
        )?;
        self.bitmaps.insert(Arc::clone(source), image, bytes);
        Some((waiters, intrinsic))
    }

    pub fn parsed(&mut self, source: &str) -> Option<Vec<W>> {
        self.settle(source, Entry::Parsed)
    }

    pub fn requested_document_done(&mut self, source: &str) -> Option<W> {
        let index = self
            .requested
            .iter()
            .position(|(requested, _)| &**requested == source)?;
        Some(self.requested.swap_remove(index).1)
    }

    pub fn failed(&mut self, source: &str) -> Option<Vec<W>> {
        self.settle(source, Entry::Failed)
    }

    pub fn refined(
        &mut self,
        source: &Arc<str>,
        target: (u32, u32),
        bitmap: impl Into<DecodedImage<F>>,
    ) {
        let Some(Entry::Loaded(loaded)) = self.entries.get_mut(source) else {
            return;
        };
        if loaded.refining != Some(target) {
            return;
        }
        loaded.refining = None;
        loaded.unrestorable = false;
        let bitmap = bitmap.into();
        let bytes = bitmap.byte_len();
        let (image, descriptor) = bitmap.into_frame();
        loaded.descriptor = descriptor;
        self.bitmaps.insert(Arc::clone(source), image, bytes);
    }

    pub fn refine_failed(&mut self, source: &str) {
        let resident = self.bitmaps.contains(source);
        if let Some(Entry::Loaded(loaded)) = self.entries.get_mut(source) {
            loaded.refining = None;
            loaded.unrestorable |= !resident;
        }
    }

    pub fn describe(&mut self, source: &str, hint: ImageSizeHint) -> Described<F> {
        let ratio = self.config.downsample_ratio;
        let Some((loaded, target, image)) = self.find(source, hint) else {
            return Described::Unavailable;
        };
        if let Some(image) = image {
            let descriptor = loaded.descriptor;
            return Described::Resident {
                refine: loaded.plan((descriptor.width, descriptor.height), target, ratio),
                image,
                descriptor,
            };
        }
        if loaded.unrestorable {
            return Described::Unavailable;
        }
        let descriptor = loaded.descriptor;
        Described::Absent {
            refine: loaded.plan((descriptor.width, descriptor.height), target, ratio),
            descriptor,
        }
    }

    pub fn want(&mut self, source: &str, hint: ImageSizeHint) -> Option<Refinement> {
        let (loaded, target, image) = self.find(source, hint)?;
        if image.is_some() || loaded.refining.is_some() || loaded.unrestorable {
            return None;
        }
        loaded.refining = Some(target);
        Some(loaded.decode_at(target))
    }

    pub fn release(&mut self, sources: &[Arc<str>]) {
        for source in sources {
            let _ = self.bitmaps.remove(source);
        }
    }

    fn find(&mut self, source: &str, hint: ImageSizeHint) -> Option<Found<'_, F>> {
        let loaded = match self.entries.get_mut(source) {
            Some(Entry::Loaded(loaded)) => loaded,
            Some(Entry::Parsed | Entry::Loading { .. } | Entry::Failed) | None => return None,
        };
        let target = bounded(
            hint.fit(loaded.intrinsic.0, loaded.intrinsic.1),
            self.config.max_renderable,
        );
        let image = self.bitmaps.get(source).cloned();
        Some((loaded, target, image))
    }

    pub fn retain(&mut self, view: ViewKey, frame: &[Arc<str>]) {
        self.views.insert(view, frame.to_vec());
        self.pin();
    }

    pub fn release_view(&mut self, view: ViewKey) {
        if self.views.remove(&view).is_some() {
            self.pin();
        }
    }

    #[must_use]
    pub fn budget(&self) -> usize {
        self.bitmaps.budget()
    }

    pub fn set_budget(&mut self, budget: usize) {
        self.bitmaps.set_budget(budget);
    }

    #[must_use]
    pub fn bitmap_bytes(&self) -> usize {
        self.bitmaps.used_bytes()
    }

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

    #[must_use]
    pub fn knows(&self, source: &str) -> bool {
        self.entries.contains_key(source)
    }

    #[must_use]
    pub fn is_resident(&self, source: &str) -> bool {
        self.bitmaps.contains(source)
    }

    #[must_use]
    pub fn resident_size(&self, source: &str) -> Option<(u32, u32)> {
        if !self.bitmaps.contains(source) {
            return None;
        }
        match self.entries.get(source) {
            Some(Entry::Loaded(loaded)) => {
                Some((loaded.descriptor.width, loaded.descriptor.height))
            }
            Some(Entry::Loading { .. } | Entry::Parsed | Entry::Failed) | None => None,
        }
    }

    fn settle(&mut self, source: &str, next: Entry<W>) -> Option<Vec<W>> {
        let entry = self.entries.get_mut(source)?;
        let Entry::Loading { waiters } = entry else {
            return None;
        };
        let waiters = std::mem::take(waiters);
        *entry = next;
        Some(waiters)
    }

    fn pin(&mut self) {
        let union: Vec<Arc<str>> = self.views.values().flatten().cloned().collect();
        let _ = self.bitmaps.pin(&union);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Bitmap;

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
            bitmap: bitmap.into(),
            format: ImageFormat::Png,
            header: None,
            encoded: encoded.map(Bytes::from_static),
            animated: false,
        }
    }

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
                intrinsic: (40, 20),
                animated: false,
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
        let mut residency: Residency<()> = Residency::new(ResidencyConfig {
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
            residency.describe("a.png", ImageSizeHint::UNBOUNDED),
            Described::Unavailable
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
            residency.describe("icon.svg", ImageSizeHint::UNBOUNDED),
            Described::Unavailable
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
    fn only_a_loaded_bitmap_is_described() {
        let mut residency = residency(1 << 20);
        residency.request("loading.png", "waiter");
        residency.request("failed.png", "waiter");
        residency.failed("failed.png");
        residency.request("parsed.svg", "waiter");
        residency.parsed("parsed.svg");
        for source in ["loading.png", "failed.png", "parsed.svg", "unknown.png"] {
            assert!(
                matches!(
                    residency.describe(source, ImageSizeHint::UNBOUNDED),
                    Described::Unavailable
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
        let Described::Resident {
            image,
            refine: Some(refinement),
            ..
        } = residency.describe("big.png", hint)
        else {
            panic!("a hit with a refinement");
        };
        let image = image.into_pixels();
        assert_eq!((image.width, image.height), (1000, 1000));
        assert_eq!(refinement.target, (125, 125));
        assert_eq!(refinement.url, url("big.png"));
        assert_eq!(refinement.encoded.as_deref(), Some(&b"bytes"[..]));
        assert!(
            matches!(
                residency.describe("big.png", hint),
                Described::Resident { refine: None, .. }
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
                residency.describe("big.png", hint),
                Described::Resident { refine: None, .. }
            ),
            "the refined bitmap is the drawn size"
        );
    }

    #[test]
    fn a_failed_refinement_keeps_the_bitmap_and_lets_a_later_describe_plan_again() {
        let mut residency = residency(1 << 30);
        load(
            &mut residency,
            "big.png",
            bitmap(1000, 1000, (4000, 4000)),
            None,
        );
        let hint = ImageSizeHint::new(125, 125);
        assert!(matches!(
            residency.describe("big.png", hint),
            Described::Resident {
                refine: Some(_),
                ..
            }
        ));
        residency.refine_failed("big.png");
        assert_eq!(residency.resident_size("big.png"), Some((1000, 1000)));
        assert!(matches!(
            residency.describe("big.png", hint),
            Described::Resident {
                refine: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn an_evicted_bitmap_is_restored_from_kept_bytes_or_its_url_once_wanted() {
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
        assert!(
            matches!(
                residency.describe("kept.png", ImageSizeHint::UNBOUNDED),
                Described::Absent { refine: None, .. }
            ),
            "a describe restores nothing"
        );

        let plan = residency
            .want("kept.png", ImageSizeHint::UNBOUNDED)
            .expect("a restore");
        assert_eq!(plan.encoded.as_deref(), Some(&b"kept"[..]));
        assert_eq!(plan.target, (2, 2));
        let plan = residency
            .want("fetched.png", ImageSizeHint::new(1, 1))
            .expect("a restore");
        assert!(plan.encoded.is_none());
        assert_eq!(plan.url, url("fetched.png"));
        assert_eq!(plan.target, (1, 1), "restored at the drawn size");
        assert!(
            residency
                .want("fetched.png", ImageSizeHint::new(1, 1))
                .is_none(),
            "one restore while it is in flight"
        );
        assert!(
            residency
                .want("unknown.png", ImageSizeHint::UNBOUNDED)
                .is_none()
        );

        residency.refined(&Arc::from("fetched.png"), plan.target, bitmap(1, 1, (2, 2)));
        assert!(
            residency.is_resident("fetched.png"),
            "the restore re-enters the tier"
        );
        let Described::Resident { image, .. } =
            residency.describe("fetched.png", ImageSizeHint::new(1, 1))
        else {
            panic!("a restored bitmap is resident");
        };
        let image = image.into_pixels();
        assert_eq!((image.width, image.height), (1, 1));
        assert!(
            residency
                .want("fetched.png", ImageSizeHint::new(1, 1))
                .is_none(),
            "a resident bitmap is not restored"
        );
    }

    #[test]
    fn a_failed_restore_leaves_nothing_to_draw_and_is_not_retried() {
        let mut residency = residency(1 << 20);
        load(&mut residency, "a.png", bitmap(2, 2, (2, 2)), None);
        residency.release(&[Arc::from("a.png")]);
        assert!(residency.want("a.png", ImageSizeHint::UNBOUNDED).is_some());
        residency.refine_failed("a.png");
        assert!(matches!(
            residency.describe("a.png", ImageSizeHint::UNBOUNDED),
            Described::Unavailable
        ));
        assert!(
            residency.want("a.png", ImageSizeHint::UNBOUNDED).is_none(),
            "nothing retries it"
        );
        assert!(
            matches!(
                residency.request("a.png", "again"),
                Requested::Loaded {
                    intrinsic: (2, 2),
                    animated: false
                }
            ),
            "and the source stays loaded"
        );

        load(
            &mut residency,
            "big.png",
            bitmap(1000, 1000, (4000, 4000)),
            None,
        );
        let hint = ImageSizeHint::new(125, 125);
        assert!(matches!(
            residency.describe("big.png", hint),
            Described::Resident {
                refine: Some(_),
                ..
            }
        ));
        residency.refine_failed("big.png");
        residency.release(&[Arc::from("big.png")]);
        assert!(matches!(
            residency.describe("big.png", hint),
            Described::Absent { .. }
        ));
    }

    #[test]
    fn a_restore_never_asks_for_more_than_the_rasterizer_draws() {
        let mut residency = residency(0);
        load(
            &mut residency,
            "huge.png",
            bitmap(1, 1, (20_000, 10_000)),
            None,
        );
        residency.set_budget(0);
        let plan = residency
            .want("huge.png", ImageSizeHint::UNBOUNDED)
            .expect("a restore");
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
    fn a_released_bitmap_is_described_absent_and_restored_once_wanted() {
        let mut residency = residency(1 << 20);
        load(&mut residency, "a.png", bitmap(2, 2, (2, 2)), Some(b"kept"));
        let view = residency.new_view();
        residency.retain(view, &[Arc::from("a.png")]);
        let Described::Resident {
            image,
            refine: None,
            ..
        } = residency.describe("a.png", ImageSizeHint::UNBOUNDED)
        else {
            panic!("a resident bitmap is described with its pixels");
        };
        let image = image.into_pixels();
        let descriptor = ImageDescriptor::of(&image);
        drop(image);

        residency.release(&[Arc::from("a.png")]);
        assert!(
            !residency.is_resident("a.png"),
            "a release drops a pinned bitmap too"
        );
        assert_eq!(residency.bitmap_bytes(), 0);
        assert!(residency.knows("a.png"), "the entry stays loaded");
        assert!(matches!(
            residency.request("a.png", "again"),
            Requested::Loaded {
                intrinsic: (2, 2),
                animated: false
            }
        ));
        let Described::Absent {
            descriptor: absent,
            refine: None,
        } = residency.describe("a.png", ImageSizeHint::UNBOUNDED)
        else {
            panic!("a released bitmap is described by its descriptor");
        };
        assert_eq!(absent, descriptor, "the descriptor of the bitmap it had");
        assert!(
            !residency.is_resident("a.png"),
            "a describe restores nothing"
        );

        let plan = residency
            .want("a.png", ImageSizeHint::UNBOUNDED)
            .expect("a wanted released bitmap is restored");
        assert_eq!(plan.encoded.as_deref(), Some(&b"kept"[..]));
        residency.refined(&Arc::from("a.png"), plan.target, bitmap(2, 2, (2, 2)));
        let Described::Resident { image, .. } =
            residency.describe("a.png", ImageSizeHint::UNBOUNDED)
        else {
            panic!("a restored bitmap is resident again");
        };
        let image = image.into_pixels();
        assert_ne!(
            ImageDescriptor::of(&image),
            descriptor,
            "a restore is a new blob"
        );
        residency.release(&[Arc::from("unknown.png")]);
    }

    #[test]
    fn an_evicted_bitmap_is_described_absent_and_refined_once_from_its_size() {
        let mut residency = residency(1 << 30);
        load(
            &mut residency,
            "big.png",
            bitmap(1000, 1000, (4000, 4000)),
            None,
        );
        let Described::Resident {
            image,
            refine: None,
            ..
        } = residency.describe("big.png", ImageSizeHint::new(1000, 1000))
        else {
            panic!("resident at the drawn size");
        };
        let image = image.into_pixels();
        let descriptor = ImageDescriptor::of(&image);
        residency.set_budget(0);
        assert!(!residency.is_resident("big.png"), "evicted, not released");
        let hint = ImageSizeHint::new(125, 125);
        let Described::Absent {
            descriptor: absent,
            refine: Some(refinement),
        } = residency.describe("big.png", hint)
        else {
            panic!("an evicted bitmap far larger than the draw plans a refinement");
        };
        assert_eq!(absent, descriptor);
        assert_eq!(refinement.target, (125, 125));
        assert!(
            matches!(
                residency.describe("big.png", hint),
                Described::Absent { refine: None, .. }
            ),
            "not a second while one is in flight"
        );
        assert!(matches!(
            residency.describe("missing.png", hint),
            Described::Unavailable
        ));
    }

    #[derive(Clone, Debug)]
    struct Frame(std::rc::Rc<std::cell::Cell<bool>>);

    fn platform(width: u32, height: u32, intrinsic: (u32, u32)) -> (DecodedImage<Frame>, Frame) {
        let frame = Frame(std::rc::Rc::default());
        let decoded = DecodedImage::Platform(crate::image::PlatformBitmap {
            frame: frame.clone(),
            width,
            height,
            source_width: intrinsic.0,
            source_height: intrinsic.1,
        });
        (decoded, frame)
    }

    #[test]
    fn a_platform_frame_is_held_until_released_then_restored_by_a_decode() {
        let mut residency: Residency<&'static str, Frame> = Residency::new(ResidencyConfig {
            budget_bytes: 1 << 20,
            initial_bound: 2048,
            max_renderable: MAX,
            downsample_ratio: 2.0,
        });
        assert!(matches!(
            residency.request("a.png", "loader"),
            Requested::Start { .. }
        ));
        let (bitmap, frame) = platform(4, 2, (4, 2));
        let (waiters, intrinsic) = residency
            .loaded(
                &Arc::from("a.png"),
                Decoded {
                    url: url("a.png"),
                    bitmap,
                    format: ImageFormat::Png,
                    header: None,
                    encoded: Some(Bytes::from_static(b"kept")),
                    animated: false,
                },
            )
            .expect("in flight");
        assert_eq!((waiters, intrinsic), (vec!["loader"], (4, 2)));
        assert_eq!(residency.bitmap_bytes(), 4 * 2 * 4, "four bytes a pixel");
        assert_eq!(residency.resident_size("a.png"), Some((4, 2)));

        let Described::Resident {
            image: PictureFrame::Platform(_),
            descriptor,
            refine: None,
        } = residency.describe("a.png", ImageSizeHint::UNBOUNDED)
        else {
            panic!("the platform frame is described with its minted descriptor");
        };
        assert_eq!((descriptor.width, descriptor.height), (4, 2));

        residency.release(&[Arc::from("a.png")]);
        assert_eq!(
            std::rc::Rc::strong_count(&frame.0),
            1,
            "the tier let go of its frame"
        );
        assert_eq!(residency.bitmap_bytes(), 0);
        assert!(matches!(
            residency.describe("a.png", ImageSizeHint::UNBOUNDED),
            Described::Absent { descriptor: absent, .. } if absent == descriptor
        ));
        let plan = residency
            .want("a.png", ImageSizeHint::UNBOUNDED)
            .expect("a release is restored by a decode");
        assert_eq!(plan.encoded.as_deref(), Some(&b"kept"[..]));
        let (restored, _) = platform(4, 2, (4, 2));
        residency.refined(&Arc::from("a.png"), plan.target, restored);
        let Described::Resident {
            descriptor: again, ..
        } = residency.describe("a.png", ImageSizeHint::UNBOUNDED)
        else {
            panic!("restored");
        };
        assert_ne!(again, descriptor, "a decode is a new frame");
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
