//! The painter's per-commit pixels: what one committed frame draws, asked of
//! the host's resource system once per commit, and let go of once the
//! rasterizer holds them.
//!
//! The whole image resource system lives on the painting thread, owned by
//! the host and serviced in its own turns. The thread that builds the
//! document holds names and load states; it never sees a store, a buffer or
//! a `peniko::ImageData`. A painter asks that store through
//! [`FrameImages`](crate::render::image::FrameImages) what each picture draws
//! as, tells it which pictures it has nothing to draw from, and tells it when
//! it may drop pixels.
//!
//! Nothing here waits for pixels. A host that decodes off-thread
//! synchronises that itself: it already drives the painter's turns, so it
//! has somewhere to do it, and the painter asks again at each of them.
//!
//! # One copy of a bitmap
//!
//! A bitmap the frame draws lives on the CPU until the rasterizer holds its
//! upload, one for every draw of it whatever the sampler, and on the GPU
//! afterwards:
//!
//! 1. Per commit, each distinct picture is described
//!    ([`FrameImages::describe`](crate::render::image::FrameImages::describe)). Pixels the store
//!    holds are held here too; a picture whose pixels it dropped is drawn from its upload when that
//!    can be leased ([`BitmapLeases`]). When it cannot, the picture is *wanted*: the frame draws
//!    nothing for it and the store is told so, once
//!    ([`FrameImages::want`](crate::render::image::FrameImages::want)).
//! 2. Every turn, each picture that can change without a commit is described again — a wanted one,
//!    one the page does not name by URL ([`PictureKey::Id`]), and one whose store named the instant
//!    it next changes — and a change takes its draws: its compose key moves, so the painter
//!    composes again ([`Painter::adopt`](super::Painter::adopt)). A picture the store now answers
//!    nothing for is no longer wanted, and draws nothing.
//! 3. Every turn, each picture still held whose upload has been made is leased, its pixels let go,
//!    and the store told it may drop them
//!    ([`FrameImages::release`](crate::render::image::FrameImages::release)).
//! 4. A render that found neither an upload nor the pixels it was lent marks the table stale, and
//!    the next turn describes the commit again.
//!
//! # Live pictures
//!
//! An `Id` picture's frames are not lent. When the store answers a
//! generation other than the one this painter drew, its newest frame is
//! taken out of its slot
//! ([`FrameImages::take_frame`](crate::render::image::FrameImages::take_frame)) and written
//! into the picture's own texture at once ([`BitmapLeases::write_picture`]): the same texture
//! and the same texture id for every frame of the picture, rewritten in place, reallocated
//! only when a frame's size differs from the last. The frame is dropped as soon as it is
//! written. A picture with no frame to take and no texture is wanted.
//!
//! All of it runs in [`Painter::adopt`](super::Painter::adopt), before a
//! swap-chain image is acquired. The composed list and the filter bakes are
//! lent the pixels ([`ImageRef::lent`]) rather than given them, so they keep
//! nothing alive.
//!
//! # What a change costs
//!
//! [`PainterImages::revision`] counts the changes of what the commit's draws
//! resolve to, and is the third term of the painter's compose key, so a new
//! frame of a picture recomposes the target with no commit. A URL picture
//! that moves to another bitmap within a commit (an animated image's next
//! frame) ends the lease on the bitmap it drew before, so the uploads a
//! long-adopted commit keeps leased are one per picture, and the earlier
//! frames are released under the rasterizer's budget. A `filter` or
//! `backdrop-filter` bake keys on the bitmaps its own range draws
//! (`render::blur`), so the change re-bakes only the entries that draw it.
//! A commit whose pictures are all static URL images is described once and
//! never again: [`PainterImages::refresh`] answers at its first test.

use std::convert::Infallible;
use std::sync::Arc;

use crate::clock::ClockInstant;
use crate::paint::display_list::ImageRef;
use crate::peniko::ImageData;
use crate::render::gpu::SharedGpu;
use crate::render::image::{FrameImage, FrameImages, ImageDescriptor, ImageSizeHint};
use crate::render::picture::{PictureFrame, PictureId, PictureKey};
use crate::render::sparse::BitmapLeases;

/// A store the painter can draw from: one whose frames are CPU pixels. A
/// platform frame needs a writer of the embedder's, which is not built.
pub(super) trait CpuFrames: FrameImages<PlatformFrame = Infallible> {}

impl<S: FrameImages<PlatformFrame = Infallible> + ?Sized> CpuFrames for S {}

/// One distinct picture the current commit draws.
#[derive(Debug)]
struct Source {
    key: PictureKey,
    hint: ImageSizeHint,
    /// The bitmap the frame draws, once the store answered one it can draw.
    descriptor: Option<ImageDescriptor>,
    /// The pixels, while the rasterizer may still need them for an upload.
    pixels: Option<ImageData>,
    /// Whether this painter's leases hold its upload, or for an `Id`
    /// picture its texture.
    leased: bool,
    /// The store answered it without pixels and the rasterizer holds no
    /// upload of it either: the frame draws nothing for it, and the store
    /// was told it is wanted. Cleared by the describe that answers pixels,
    /// an upload, or nothing to draw.
    wanted: bool,
    /// The generation of what the frame draws for it.
    generation: u64,
    /// When the store said it will next change by itself.
    next_change: Option<ClockInstant>,
}

/// The bitmaps the current commit draws.
///
/// The per-draw table is deliberately **not** a cache. It is one commit's
/// bitmaps in draw order, rebuilt whenever the commit moves, applying no
/// policy of its own beyond letting go of pixels the rasterizer holds.
/// Every decision about what stays in memory belongs to the host.
///
/// It is indexed rather than keyed: composition replays the program on every
/// frame that scrolls, and a slice index costs nothing where a URL hash would
/// have cost a lookup per draw per frame.
#[derive(Debug, Default)]
pub(super) struct PainterImages {
    /// The commit the table was built for.
    key: Option<u64>,
    /// A render lost pixels it was lent: the next adoption describes the
    /// commit again.
    stale: bool,
    /// The store the table was described from has gone, so nothing it is
    /// missing will arrive.
    orphaned: bool,
    /// One entry per distinct picture of that commit, in first-draw order.
    sources: Vec<Source>,
    /// Per image draw of that commit, in draw order, the index of its
    /// picture in `sources`.
    draws: Vec<usize>,
    /// One entry per image draw of that commit, in draw order, its pixels
    /// lent.
    resolved: Vec<Option<ImageRef>>,
    /// Scratch, reused across commits: the distinct pictures' keys, which
    /// the store is told as the drawn set, and the pictures it may release.
    names: Vec<PictureKey>,
    /// The leases on the output's uploads; `None` for an output with no
    /// rasterizer.
    leases: Option<BitmapLeases>,
    /// The upload count at the last lease attempt, or `None` once the
    /// entries changed since: what lets [`Self::release_held`] skip the
    /// attempt.
    checked: Option<u64>,
    /// How many times what the draws resolve to has changed: the compose
    /// key's picture term.
    revision: u64,
    /// Whether some picture is described again every turn
    /// ([`Source::watched`]).
    watching: bool,
}

impl PainterImages {
    /// Whether the table already holds this commit's bitmaps.
    pub(super) fn holds(&self, commit: u64) -> bool {
        self.key == Some(commit) && !self.stale
    }

    /// Whether a render lost pixels the table lent it since the last
    /// resolve.
    pub(super) fn is_stale(&self) -> bool {
        self.stale
    }

    /// Whether the frame lacks a picture that may still arrive: a picture is
    /// wanted, or a render lost pixels and the commit has not been described
    /// again. False once the store has gone, since then nothing will.
    pub(super) fn awaits(&self) -> bool {
        !self.orphaned && (self.stale || self.sources.iter().any(|source| source.wanted))
    }

    /// How many times what the commit's draws resolve to has changed: a
    /// term of the compose key, which moves exactly when a picture's draws
    /// do.
    pub(super) const fn revision(&self) -> u64 {
        self.revision
    }

    /// The earliest instant a picture the frame draws said it will next
    /// change by itself.
    pub(super) fn next_change(&self) -> Option<ClockInstant> {
        if !self.watching {
            return None;
        }
        self.sources
            .iter()
            .filter_map(|source| source.next_change)
            .min()
    }

    /// Takes leases on `gpu`'s uploads, the output's stack: leases on
    /// another device's, or with `None` any, end. A table that had drawn
    /// from the old uploads is stale.
    pub(super) fn bind(&mut self, gpu: Option<&Arc<SharedGpu>>) {
        if match (&self.leases, gpu) {
            (Some(leases), Some(gpu)) => leases.on(gpu),
            (None, None) => true,
            _ => false,
        } {
            return;
        }
        self.leases = gpu.map(BitmapLeases::new);
        self.checked = None;
        if self.sources.iter().any(|source| source.leased) {
            self.stale = true;
        }
        for source in &mut self.sources {
            source.leased = false;
        }
    }

    /// Describes every picture `frame` draws to `store`, once per commit,
    /// and tells it which of them the frame has nothing to draw from.
    ///
    /// The commit is the key for what is drawn. A report that changes what a
    /// frame draws dirties the document, and every rebuild takes a new commit
    /// id. What changes within one commit — pixels that arrive for a wanted
    /// picture, a picture's new frame — is taken by [`Self::refresh`].
    ///
    /// Never waits: every call it makes of `store` is non-blocking.
    pub(super) fn resolve<S: CpuFrames + ?Sized>(
        &mut self,
        frame: &crate::visual::CommittedFrame,
        store: &S,
    ) {
        let mut hints = Vec::new();
        frame.image_sources(&mut self.names, &mut hints, &mut self.draws);
        // The old entries stay until the new ones are built: their leases
        // hold uploads the new commit may share, and are let go only once
        // the new set is leased.
        let sources: Vec<Source> = self
            .names
            .iter()
            .zip(hints)
            .map(|(key, hint)| {
                let mut source = Source {
                    key: key.clone(),
                    hint,
                    descriptor: None,
                    pixels: None,
                    leased: false,
                    wanted: false,
                    generation: 0,
                    next_change: None,
                };
                source.describe(store, self.leases.as_ref());
                if source.wanted {
                    store.want(&source.key, source.hint);
                }
                source
            })
            .collect();
        if let Some(leases) = &self.leases {
            lease_exactly(leases, &sources);
        }
        self.sources = sources;
        self.watching = self.sources.iter().any(Source::watched);
        self.fill_resolved();
        store.retain(&self.names);
        self.key = Some(frame.commit_id());
        self.stale = false;
        self.orphaned = false;
        self.checked = None;
        self.revision += 1;
    }

    /// Describes every picture that can change without a commit again and
    /// takes what the store now answers; answers whether a draw changed,
    /// which is when the frame has to be composed again.
    ///
    /// A wanted picture is described until a describe answers its pixels, an
    /// upload the rasterizer holds, or nothing to draw, and is not wanted
    /// again meanwhile: its restore is in the store's hands. An `Id`
    /// picture, and one whose store named its next change, is described
    /// every turn, and its newest frame taken when its generation moved.
    /// A commit with neither asks nothing.
    pub(super) fn refresh<S: CpuFrames + ?Sized>(&mut self, store: &S) -> bool {
        if !self.watching {
            return false;
        }
        let mut changed = false;
        for source in &mut self.sources {
            if !source.watched() {
                continue;
            }
            let was_wanted = source.wanted;
            let drawn = source.drawn();
            source.describe(store, self.leases.as_ref());
            changed |= source.drawn() != drawn;
            // A picture that was drawn and now has nothing to draw from is
            // wanted for the first time.
            if source.wanted && !was_wanted {
                store.want(&source.key, source.hint);
            }
        }
        self.watching = self.sources.iter().any(Source::watched);
        if changed {
            // A URL picture that moved to another bitmap, an animated
            // image's next frame, lets go of the one it drew before, so a
            // commit that stays adopted while its pictures change leases one
            // bitmap per picture rather than every bitmap it ever drew.
            if let Some(leases) = &self.leases {
                lease_exactly(leases, &self.sources);
            }
            self.fill_resolved();
            self.checked = None;
            self.revision += 1;
        }
        changed
    }

    /// The store has gone: nothing a wanted picture waits for will arrive,
    /// and nothing is waited for.
    pub(super) fn orphan(&mut self) {
        self.orphaned = true;
        for source in &mut self.sources {
            source.wanted = false;
        }
    }

    /// Lets go of the pixels of every picture whose upload the rasterizer
    /// now holds: leases them, drops this table's reference, and tells
    /// `store`, if there is one, that it may drop its own.
    ///
    /// Cheap enough for every turn: nothing is attempted unless a bitmap
    /// was uploaded, or the table changed, since the last attempt. Without
    /// leases — an output with no rasterizer — nothing is let go.
    pub(super) fn release_held<S: CpuFrames + ?Sized>(&mut self, store: Option<&S>) {
        let Some(leases) = &self.leases else {
            return;
        };
        let uploads = leases.uploads();
        if self.checked == Some(uploads) {
            return;
        }
        self.checked = Some(uploads);
        self.names.clear();
        for source in &mut self.sources {
            if source.pixels.is_none() {
                continue;
            }
            let Some(descriptor) = source.descriptor else {
                continue;
            };
            if leases.lease(&descriptor) {
                source.pixels = None;
                source.leased = true;
                self.names.push(source.key.clone());
            }
        }
        if let Some(store) = store
            && !self.names.is_empty()
        {
            store.release(&self.names);
        }
    }

    /// A render lost pixels this table lent it: the next adoption describes
    /// the commit again.
    pub(super) fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// This commit's bitmaps, in draw order.
    pub(super) fn resolved(&self) -> &[Option<ImageRef>] {
        &self.resolved
    }

    /// Drops the table and its leases, so the next commit resolves from
    /// scratch.
    ///
    /// What a painter does when it stops observing a view: the bitmaps came
    /// out of that view's store, and commit ids restart at one per document,
    /// so a table still keyed by the old page's commit would be handed to the
    /// next one.
    pub(super) fn forget(&mut self) {
        self.key = None;
        self.stale = false;
        self.orphaned = false;
        self.sources.clear();
        self.draws.clear();
        self.resolved.clear();
        self.names.clear();
        self.checked = None;
        self.watching = false;
        self.revision += 1;
        if let Some(leases) = &self.leases {
            leases.replace([], []);
        }
    }

    /// Rebuilds the per-draw table from the sources: each draw lent its
    /// picture's pixels, or named by its upload or its texture, or nothing.
    fn fill_resolved(&mut self) {
        let Self {
            sources,
            draws,
            resolved,
            ..
        } = self;
        resolved.clear();
        resolved.extend(draws.iter().map(|&index| sources[index].image_ref()));
    }
}

/// Makes `leases` exactly the uploads `sources` are drawn from: each URL
/// picture's bitmap and each `Id` picture's texture its entry holds.
fn lease_exactly(leases: &BitmapLeases, sources: &[Source]) {
    leases.replace(
        sources
            .iter()
            .filter(|source| source.leased && source.key.id().is_none())
            .filter_map(|source| source.descriptor.as_ref()),
        sources
            .iter()
            .filter(|source| source.leased)
            .filter_map(|source| source.key.id()),
    );
}

impl Source {
    /// Whether the picture is described again every turn: it is wanted, it
    /// is one an element owns, or its store named when it next changes.
    fn watched(&self) -> bool {
        self.wanted || self.key.id().is_some() || self.next_change.is_some()
    }

    /// What the frame draws for it, as identity: the bitmap and its
    /// generation.
    fn drawn(&self) -> (Option<ImageDescriptor>, u64) {
        (self.descriptor, self.generation)
    }

    /// The draw of this picture: its texture, its pixels lent, its upload,
    /// or nothing.
    fn image_ref(&self) -> Option<ImageRef> {
        let descriptor = self.descriptor?;
        Some(match (&self.key, &self.pixels) {
            (_, Some(pixels)) => ImageRef::lent(pixels),
            (PictureKey::Id(id), None) => ImageRef::written(*id, descriptor),
            (PictureKey::Url(_), None) => ImageRef::uploaded(descriptor),
        })
    }

    /// Asks `store` what this picture draws as. See [`Self::describe_url`]
    /// and [`Self::describe_picture`].
    fn describe<S: CpuFrames + ?Sized>(&mut self, store: &S, leases: Option<&BitmapLeases>) {
        self.wanted = false;
        match self.key.id() {
            Some(id) => self.describe_picture(id, store, leases),
            None => self.describe_url(store, leases),
        }
    }

    /// A URL picture: its pixels when the store holds them, its upload when
    /// the store dropped the pixels and `leases` can hold it, and otherwise
    /// nothing yet — the picture is wanted. A picture the store answers
    /// nothing for draws nothing and is not wanted.
    fn describe_url<S: CpuFrames + ?Sized>(&mut self, store: &S, leases: Option<&BitmapLeases>) {
        let Some(FrameImage {
            descriptor,
            pixels,
            generation,
            next_change,
        }) = store.describe(&self.key, self.hint)
        else {
            // Nothing to draw it from any more.
            self.descriptor = None;
            self.pixels = None;
            self.next_change = None;
            return;
        };
        self.next_change = next_change;
        if let Some(pixels) = pixels {
            let descriptor = ImageDescriptor::of(&pixels);
            // The lease held is of the bitmap drawn before, if any other.
            self.leased &= self.descriptor == Some(descriptor);
            self.descriptor = Some(descriptor);
            self.pixels = Some(pixels);
            self.generation = generation;
            return;
        }
        if self.descriptor == Some(descriptor) && (self.leased || self.pixels.is_some()) {
            return;
        }
        if leases.is_some_and(|leases| leases.lease(&descriptor)) {
            self.descriptor = Some(descriptor);
            self.pixels = None;
            self.leased = true;
            self.generation = generation;
            return;
        }
        self.wanted = true;
    }

    /// An `Id` picture: the frame its texture holds when that is the
    /// generation the store answers; otherwise its newest frame, taken and
    /// written into the texture; otherwise the older frame the texture still
    /// holds; otherwise nothing yet — the picture is wanted. Without a
    /// rasterizer the taken frame's pixels are held instead. A picture the
    /// store answers nothing for draws nothing and is not wanted.
    fn describe_picture<S: CpuFrames + ?Sized>(
        &mut self,
        id: PictureId,
        store: &S,
        leases: Option<&BitmapLeases>,
    ) {
        let Some(state) = store.describe(&self.key, self.hint) else {
            self.descriptor = None;
            self.pixels = None;
            self.next_change = None;
            return;
        };
        self.next_change = state.next_change;
        if self.descriptor.is_some() && self.generation == state.generation {
            return;
        }
        if !self.leased
            && let Some((generation, descriptor)) =
                leases.and_then(|leases| leases.lease_picture(id))
        {
            self.descriptor = Some(descriptor);
            self.generation = generation;
            self.leased = true;
            if generation == state.generation {
                return;
            }
        }
        if let Some(taken) = store.take_frame(id) {
            let pixels = match taken.frame {
                PictureFrame::Pixels(pixels) => pixels,
                PictureFrame::Platform(never) => match never {},
            };
            if let Some(leases) = leases {
                if leases.write_picture(id, taken.generation, &pixels) {
                    self.descriptor = Some(ImageDescriptor::of(&pixels));
                    self.generation = taken.generation;
                    self.leased = true;
                }
            } else {
                self.descriptor = Some(ImageDescriptor::of(&pixels));
                self.pixels = Some(pixels);
                self.generation = taken.generation;
            }
            return;
        }
        if self.descriptor.is_none() {
            self.wanted = true;
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::sync::Arc;

    use super::super::{FrameSize, GesturePolicy, Output, Painter, Screenshot, Viewport};
    use crate::clock::ClockInstant;
    use crate::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
    use crate::render::gpu::{Headless, SharedGpu};
    use crate::render::image::{
        FrameImage, FrameImages, ImageDescriptor, ImageEvent, ImageSizeHint,
    };
    use crate::render::picture::{PictureId, PictureKey, PictureSlot, TakenFrame};
    use crate::render::sparse::{BitmapLeases, SharedRaster};
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::Document;
    use crate::tree::document::tests::device;

    const SRC: &str = "app:///a.png";
    const OTHER: &str = "app:///b.png";
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A store that drops a source's pixels when it is told it may, and
    /// restores them into a new blob only when the test answers what was
    /// wanted: a production host's policy, with the restore outside the
    /// painter's calls. Every want and every release is logged.
    struct Releasing {
        entries: RefCell<HashMap<&'static str, Entry>>,
        /// Every want, in order.
        wants: RefCell<Vec<String>>,
        /// The wants not answered yet.
        unanswered: RefCell<Vec<String>>,
        released: RefCell<Vec<Vec<Arc<str>>>>,
    }

    /// One source of a [`Releasing`] store.
    struct Entry {
        /// The colour of its one texel, which a restore decodes again.
        texel: [u8; 4],
        /// The pixels, while the store holds them.
        pixels: Option<ImageData>,
        /// The bitmap it was last answered as.
        descriptor: ImageDescriptor,
        /// Its restore failed: there is nothing to draw it from.
        lost: bool,
    }

    impl Releasing {
        fn new(sources: [(&'static str, [u8; 4]); 2]) -> Self {
            let entries = sources
                .into_iter()
                .map(|(source, texel)| {
                    let pixels = texel_image(texel);
                    let descriptor = ImageDescriptor::of(&pixels);
                    let entry = Entry {
                        texel,
                        pixels: Some(pixels),
                        descriptor,
                        lost: false,
                    };
                    (source, entry)
                })
                .collect();
            Self {
                entries: RefCell::new(entries),
                wants: RefCell::default(),
                unanswered: RefCell::default(),
                released: RefCell::default(),
            }
        }

        fn holds(&self, source: &str) -> bool {
            self.entries.borrow()[source].pixels.is_some()
        }

        fn descriptor(&self, source: &str) -> ImageDescriptor {
            self.entries.borrow()[source].descriptor
        }

        fn wants(&self) -> usize {
            self.wants.borrow().len()
        }

        /// Restores every source wanted so far into a new blob: what a
        /// host's restore job does before it wakes the host.
        fn answer(&self) {
            let mut entries = self.entries.borrow_mut();
            for source in self.unanswered.borrow_mut().drain(..) {
                let entry = entries.get_mut(source.as_str()).expect("a known source");
                let image = texel_image(entry.texel);
                entry.descriptor = ImageDescriptor::of(&image);
                entry.pixels = Some(image);
            }
        }

        /// Fails every restore wanted so far: those sources have nothing to
        /// draw from any more.
        fn fail(&self) {
            let mut entries = self.entries.borrow_mut();
            for source in self.unanswered.borrow_mut().drain(..) {
                entries
                    .get_mut(source.as_str())
                    .expect("a known source")
                    .lost = true;
            }
        }
    }

    impl FrameImages for Releasing {
        type PlatformFrame = Infallible;

        fn describe(&self, picture: &PictureKey, _: ImageSizeHint) -> Option<FrameImage> {
            let entries = self.entries.borrow();
            let entry = entries.get(picture.url()?)?;
            if entry.lost {
                return None;
            }
            Some(FrameImage {
                descriptor: entry.descriptor,
                pixels: entry.pixels.clone(),
                generation: 0,
                next_change: None,
            })
        }

        fn want(&self, picture: &PictureKey, _: ImageSizeHint) {
            let source = picture.url().expect("a URL store is asked about URLs");
            self.wants.borrow_mut().push(source.to_owned());
            self.unanswered.borrow_mut().push(source.to_owned());
        }

        fn retain(&self, _: &[PictureKey]) {}

        fn release(&self, pictures: &[PictureKey]) {
            let sources: Vec<Arc<str>> = pictures
                .iter()
                .filter_map(|picture| match picture {
                    PictureKey::Url(url) => Some(Arc::clone(url)),
                    PictureKey::Id(_) => None,
                })
                .collect();
            let mut entries = self.entries.borrow_mut();
            for source in &sources {
                if let Some(entry) = entries.get_mut(&**source) {
                    entry.pixels = None;
                }
            }
            self.released.borrow_mut().push(sources);
        }
    }

    /// A one-texel straight-alpha RGBA8 bitmap of `texel`, in a blob of its
    /// own.
    fn texel_image(texel: [u8; 4]) -> ImageData {
        ImageData {
            data: Blob::new(Arc::new(texel.to_vec())),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 1,
            height: 1,
        }
    }

    fn loaded(source: &str) -> ImageEvent {
        ImageEvent::Loaded {
            source: Arc::from(source),
            width: 1,
            height: 1,
        }
    }

    /// Whether `gpu` holds an upload of `image`: a lease of it succeeds
    /// only then, and ends as the leases are dropped here.
    fn uploaded(gpu: &Arc<SharedGpu>, image: &ImageDescriptor) -> bool {
        BitmapLeases::new(gpu).lease(image)
    }

    fn at(screenshot: &Screenshot, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * screenshot.size.width + x) * 4) as usize;
        screenshot.pixels[at..at + 4]
            .try_into()
            .expect("four channels")
    }

    /// The texel at `x`, `y` of what the painter's offscreen target holds,
    /// read without a capture, which would refuse a frame that lacks a
    /// picture on its way.
    fn held_at(painter: &mut Painter, x: u32, y: u32) -> [u8; 4] {
        let width = painter.frame_size.width;
        let Output::Offscreen(gpu) = &mut painter.output else {
            panic!("an offscreen painter");
        };
        let pixels = gpu.read_pixels().expect("the target reads back");
        let at = ((y * width + x) * 4) as usize;
        pixels[at..at + 4].try_into().expect("four channels")
    }

    /// A painter drawing offscreen on `gpu`, 16 × 16 px.
    fn painter_on(gpu: &Arc<SharedGpu>) -> Painter {
        let frame_size = FrameSize::for_viewport(16.0, 16.0, 1.0).expect("a valid size");
        Painter::with_output(
            Viewport::new(16.0, 16.0),
            frame_size,
            Output::Offscreen(Box::new(Headless::on(Arc::clone(gpu)))),
            GesturePolicy {
                tap_slop: 50.0,
                long_press_move_slop: 8.0,
                drag_slop: 8.0,
                long_press_seconds: 0.5,
                tap_event: "press",
                long_press_event: "hold",
            },
        )
    }

    /// A page of one 8 × 8 `image` presenting `SRC`, loaded, styled by
    /// `css` besides its size.
    fn page(css: &str) -> (Document<()>, crate::tree::document::NodeId) {
        let mut document = Document::new(device(), "page", ());
        document.add_stylesheet(
            &format!("image {{ width: 8px; height: 8px; {css} }}"),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);
        document.apply_image_events(&[loaded(SRC)]);
        present(&mut document, image, SRC);
        (document, image)
    }

    /// Presents `source` on `image` at the natural size the document knows
    /// of it, as an embedder's element does.
    fn present(document: &mut Document<()>, image: crate::tree::document::NodeId, source: &str) {
        let natural = document.request_image(source).natural_size();
        document.set_picture(image, Some((PictureKey::Url(Arc::from(source)), natural)));
    }

    /// Draws `SRC` from the store's pixels, lets the rasterizer upload them
    /// and the store drop them, then commits a frame drawing `OTHER` and
    /// renders until the rasterizer has let `SRC`'s upload go too: `SRC` is
    /// then in neither place. Answers `SRC`'s descriptor as first drawn.
    fn evict_src(
        gpu: &Arc<SharedGpu>,
        painter: &mut Painter,
        store: &Releasing,
        document: &mut Document<()>,
        image: crate::tree::document::NodeId,
    ) -> ImageDescriptor {
        assert!(painter.adopt(Some(document.commit()), Some(store)));
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 4, 4), RED);
        let drawn = store.descriptor(SRC);
        assert!(uploaded(gpu, &drawn), "the render uploaded it");
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(store)));
        assert_eq!(*store.released.borrow(), [vec![Arc::<str>::from(SRC)]]);
        assert!(!store.holds(SRC), "the store dropped its pixels");

        // A commit that no longer draws it ends the lease, and renders of
        // another bitmap, past the ones the target pins what it drew in,
        // release the upload.
        document.apply_image_events(&[loaded(OTHER)]);
        present(document, image, OTHER);
        assert!(painter.adopt(Some(document.commit()), Some(store)));
        for _ in 0..8 {
            assert!(painter.draw_offscreen(0.0, true).expect("render"));
        }
        assert!(!uploaded(gpu, &drawn), "the upload was released");
        assert!(!store.holds(SRC));
        assert_eq!(store.wants(), 0, "nothing was wanted while it was held");
        drawn
    }

    /// A source with neither pixels in the store nor an upload in the
    /// rasterizer draws nothing in the frame that draws it again, and the
    /// store is told it is wanted — once, however many turns pass before it
    /// answers; a capture is refused meanwhile. The turn after the store
    /// holds the pixels again takes them with no commit and asks for a
    /// frame, which draws them; and once their upload exists they are let
    /// go again.
    #[test]
    fn a_source_with_neither_pixels_nor_an_upload_draws_nothing_until_the_store_restores_it() {
        let gpu = SharedGpu::private(SharedRaster::new);
        // Every upload no lease and no recent render holds is released after
        // each render. The stack is this test's alone.
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let (mut document, image) = page("");
        let drawn = evict_src(&gpu, &mut painter, &store, &mut document, image);

        // 1. The commit that draws it again draws nothing for it, and wants it.
        present(&mut document, image, SRC);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(*store.wants.borrow(), [SRC.to_owned()]);
        assert!(painter.awaits_pictures());
        assert!(painter.images.resolved().iter().all(Option::is_none));
        assert!(painter.take_redraw(), "the new commit asks for its frame");
        assert!(painter.draw_offscreen(0.0, false).expect("render"));
        assert_ne!(held_at(&mut painter, 4, 4), RED, "that frame draws nothing");
        assert!(
            painter.capture(0.0).is_err(),
            "a capture of a frame lacking a picture on its way is refused"
        );

        // 2. Turns before the store answers want nothing more and ask for no frame.
        for _ in 0..3 {
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
        }
        assert_eq!(
            store.wants(),
            1,
            "wanted once while the restore is in flight"
        );
        assert!(painter.awaits_pictures());
        assert!(!painter.take_redraw());

        // 3. The store answers: the next turn takes the pixels, with no commit, and asks for a
        //    frame, which draws them.
        store.answer();
        let commit = painter.frame().map(|frame| frame.commit_id());
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(painter.frame().map(|frame| frame.commit_id()), commit);
        assert!(painter.take_redraw(), "the arrival asks for a frame");
        assert!(!painter.awaits_pictures());
        let restored = store.descriptor(SRC);
        assert_ne!(restored, drawn, "a restore is a new blob");
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 4, 4), RED, "drawn from the restored pixels");
        assert_eq!(painter.output.lost_pixels(), 0, "no draw lost its pixels");
        assert!(uploaded(&gpu, &restored), "the render uploaded them");

        // 4. The next turn leases the new upload and lets the pixels go again, with no further
        //    want.
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(store.released.borrow().len(), 2, "{:?}", store.released);
        assert_eq!(
            store.released.borrow()[1],
            [Arc::<str>::from(SRC)],
            "the second release names the source"
        );
        assert!(!store.holds(SRC));
        let source = &painter.images.sources[0];
        assert!(source.pixels.is_none() && source.leased, "{source:?}");
        assert_eq!(store.wants(), 1, "no further want");
    }

    /// A wanted source the store can no longer restore is answered with
    /// nothing to draw: nothing waits for it, and the frame is captured
    /// without it.
    #[test]
    fn a_wanted_source_the_store_cannot_restore_draws_nothing_and_is_not_awaited() {
        let gpu = SharedGpu::private(SharedRaster::new);
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let (mut document, image) = page("");
        evict_src(&gpu, &mut painter, &store, &mut document, image);

        present(&mut document, image, SRC);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert!(painter.awaits_pictures());
        store.fail();
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert!(!painter.awaits_pictures(), "nothing is on its way");
        let screenshot = painter.capture(0.0).expect("capture");
        assert_ne!(at(&screenshot, 4, 4), RED);
        assert_eq!(store.wants(), 1);
    }

    /// A wanted source inside a `filter: blur()` group arrives within one
    /// commit, so the bake, keyed by commit, is taken again with it.
    #[test]
    fn a_wanted_source_that_arrives_is_baked_into_its_filter_again() {
        let gpu = SharedGpu::private(SharedRaster::new);
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let (mut document, image) = page("");
        document.add_stylesheet(
            ".blurred { filter: blur(0.25px); }",
            StylesheetOrigin::Author,
        );
        evict_src(&gpu, &mut painter, &store, &mut document, image);

        document.set_classes(image, "blurred");
        present(&mut document, image, SRC);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert!(painter.draw_offscreen(0.0, false).expect("render"));
        assert_ne!(held_at(&mut painter, 4, 4), RED, "baked without it");

        store.answer();
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        let screenshot = painter.capture(0.0).expect("capture");
        let [red, green, blue, _] = at(&screenshot, 4, 4);
        assert!(
            red > 240 && green < 16 && blue < 16,
            "baked again with it: {:?}",
            at(&screenshot, 4, 4)
        );
    }

    /// A store that has gone answers nothing any more: what the frame
    /// wanted from it is no longer waited for.
    #[test]
    fn nothing_is_awaited_from_a_store_that_has_gone() {
        let gpu = SharedGpu::private(SharedRaster::new);
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let (mut document, image) = page("");
        evict_src(&gpu, &mut painter, &store, &mut document, image);

        present(&mut document, image, SRC);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert!(painter.awaits_pictures());
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, None::<&Releasing>));
        assert!(!painter.awaits_pictures());
        assert!(painter.capture(0.0).is_ok());
    }

    /// A caller with no rasterizer resolves a frame's images itself: a
    /// source the store answers without pixels draws nothing there, is
    /// wanted, and is counted, so a capture can refuse the table; once the
    /// store holds the pixels again, the same frame resolves them.
    #[test]
    fn resolving_without_a_rasterizer_wants_and_counts_a_source_with_no_pixels() {
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let (mut document, _image) = page("");
        let frame = document.commit();
        store.release(&[PictureKey::Url(Arc::from(SRC))]);
        let (mut images, mut sources) = (Vec::new(), Vec::new());

        assert_eq!(frame.resolve_images(&store, &mut images, &mut sources), 1);
        assert_eq!(*store.wants.borrow(), [SRC.to_owned()]);
        assert!(images.iter().all(Option::is_none), "drawn as nothing");

        store.answer();
        assert_eq!(frame.resolve_images(&store, &mut images, &mut sources), 0);
        assert!(images.iter().all(Option::is_some), "drawn once it is there");
        assert_eq!(store.wants(), 1);
    }

    /// A URL store whose one source moves to a new bitmap each time the test
    /// says so, as an animated image's next frame does, and drops the
    /// pixels of the one shown once told the rasterizer holds its upload.
    struct Animated {
        frames: RefCell<Vec<ImageData>>,
        shown: std::cell::Cell<usize>,
        held: std::cell::Cell<bool>,
    }

    impl FrameImages for Animated {
        type PlatformFrame = Infallible;

        fn describe(&self, picture: &PictureKey, _: ImageSizeHint) -> Option<FrameImage> {
            (picture.url()? == SRC).then_some(())?;
            let frame = self.frames.borrow()[self.shown.get()].clone();
            Some(FrameImage {
                descriptor: ImageDescriptor::of(&frame),
                pixels: self.held.get().then_some(frame),
                generation: self.shown.get() as u64,
                next_change: Some(ClockInstant::now() + std::time::Duration::from_mins(5)),
            })
        }

        fn want(&self, _: &PictureKey, _: ImageSizeHint) {
            self.held.set(true);
        }

        fn retain(&self, _: &[PictureKey]) {}

        fn release(&self, _: &[PictureKey]) {
            self.held.set(false);
        }
    }

    /// A URL picture that moves from bitmap to bitmap within one commit
    /// keeps a lease on the one it draws only: the uploads of the bitmaps it
    /// drew before are released under the rasterizer's budget, however long
    /// the commit stays adopted.
    #[test]
    fn a_url_picture_that_changes_bitmap_leases_only_the_one_it_draws() {
        let gpu = SharedGpu::private(SharedRaster::new);
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let colours = [RED, BLUE, RED, BLUE, RED, BLUE];
        let store = Animated {
            frames: RefCell::new(colours.iter().map(|&texel| texel_image(texel)).collect()),
            shown: std::cell::Cell::new(0),
            held: std::cell::Cell::new(true),
        };
        let (mut document, _image) = page("");
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        let mut drawn = Vec::new();
        for (index, &colour) in colours.iter().enumerate() {
            store.shown.set(index);
            store.held.set(true);
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), colour);
            // The next turn leases the upload, and the store drops its pixels.
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert!(!store.held.get(), "released");
            drawn.push(ImageDescriptor::of(&store.frames.borrow()[index]));
        }
        // Renders past the ones that pin what they drew.
        for _ in 0..8 {
            assert!(painter.draw_offscreen(0.0, true).expect("render"));
        }
        let (last, earlier) = drawn.split_last().expect("frames");
        assert!(uploaded(&gpu, last), "the bitmap drawn stays leased");
        for descriptor in earlier {
            assert!(
                !uploaded(&gpu, descriptor),
                "an earlier bitmap was released: {descriptor:?}"
            );
        }
    }

    /// A store of pictures elements own: one slot per picture, which a test
    /// publishes into as a producer on any thread would. Every describe,
    /// take, want and drawn set is logged.
    #[derive(Default)]
    struct Live {
        slots: RefCell<HashMap<PictureId, PictureSlot>>,
        described: RefCell<Vec<PictureId>>,
        /// Each frame taken, with its generation.
        taken: RefCell<Vec<(PictureId, u64)>>,
        wants: RefCell<Vec<PictureId>>,
        drawn: RefCell<Vec<Vec<PictureKey>>>,
    }

    impl Live {
        /// `picture`'s slot, the producer's end.
        fn slot(&self, picture: PictureId) -> PictureSlot {
            self.slots.borrow_mut().entry(picture).or_default().clone()
        }

        fn generations_taken(&self, picture: PictureId) -> Vec<u64> {
            self.taken
                .borrow()
                .iter()
                .filter(|(taken, _)| *taken == picture)
                .map(|&(_, generation)| generation)
                .collect()
        }
    }

    impl FrameImages for Live {
        type PlatformFrame = Infallible;

        fn describe(&self, picture: &PictureKey, _: ImageSizeHint) -> Option<FrameImage> {
            let id = picture.id()?;
            self.described.borrow_mut().push(id);
            self.slots.borrow().get(&id)?.describe()
        }

        fn take_frame(&self, picture: PictureId) -> Option<TakenFrame> {
            let taken = self.slots.borrow().get(&picture)?.take()?;
            self.taken.borrow_mut().push((picture, taken.generation));
            Some(taken)
        }

        fn want(&self, picture: &PictureKey, _: ImageSizeHint) {
            self.wants.borrow_mut().push(
                picture
                    .id()
                    .expect("a live store is asked about its pictures"),
            );
        }

        fn retain(&self, drawn: &[PictureKey]) {
            self.drawn.borrow_mut().push(drawn.to_vec());
        }
    }

    /// A `width` × `height` straight-alpha RGBA8 bitmap whose texels are
    /// `texels`, row-major, repeated to fill it.
    fn bitmap(width: u32, height: u32, texels: &[[u8; 4]]) -> ImageData {
        let data: Vec<u8> = texels
            .iter()
            .cycle()
            .take((width * height) as usize)
            .flatten()
            .copied()
            .collect();
        ImageData {
            data: Blob::new(Arc::new(data)),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }
    }

    /// A page of 8 × 8 elements, side by side from the left edge, each
    /// presenting the picture at its index at a natural size of 1 × 1,
    /// with the classes `classes` names for it and `css` besides.
    fn picture_page(
        css: &str,
        pictures: &[(PictureId, &str)],
    ) -> (Document<()>, Vec<crate::tree::document::NodeId>) {
        let mut document = Document::new(device(), "page", ());
        document.add_stylesheet(
            &format!(
                "image {{ position: absolute; top: 0px; width: 8px; height: 8px; }} \
                 .hidden {{ display: none; }} {css}"
            ),
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let nodes = pictures
            .iter()
            .enumerate()
            .map(|(index, &(picture, classes))| {
                let node = document.create_element("image", ());
                document.append_child(root, node);
                document.set_inline_style(node, &format!("left: {}px;", index * 8));
                document.set_classes(node, classes);
                document.set_picture(node, Some((PictureKey::Id(picture), Some((1, 1)))));
                node
            })
            .collect();
        (document, nodes)
    }

    /// A picture whose producer publishes a new frame with no commit is
    /// taken at the next adoption, written into the same texture under the
    /// same id, and composed again: the target shows the new pixels.
    #[test]
    fn a_picture_that_changes_with_no_commit_is_recomposed_and_shows_the_new_pixels() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let picture = PictureId::new();
        let slot = store.slot(picture);
        assert!(slot.publish_pixels(bitmap(1, 1, &[RED]), None));
        let (mut document, _) = picture_page("", &[(picture, "")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), RED);
        let texture = gpu.raster.picture_texture(picture).expect("written");
        assert!(painter.take_redraw());

        // A producer on another thread publishes; the host's next turn
        // adopts the same commit.
        let woken = std::thread::scope(|scope| {
            scope
                .spawn(|| slot.publish_pixels(bitmap(1, 1, &[BLUE]), None))
                .join()
                .expect("the producer")
        });
        assert!(woken, "the last frame was taken, so the host is woken");
        let commit = painter.frame().map(|frame| frame.commit_id());
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(painter.frame().map(|frame| frame.commit_id()), commit);
        assert!(painter.take_redraw(), "the new frame owes a frame");
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), BLUE);
        assert_eq!(
            gpu.raster.picture_texture(picture),
            Some(texture),
            "rewritten in place, under the one id"
        );
        assert_eq!(store.generations_taken(picture), [1, 2]);
        assert!(store.wants.borrow().is_empty());
    }

    /// A picture inside a `filter: blur()` group that changes with no
    /// commit is baked into the filter again.
    #[test]
    fn a_picture_that_changes_under_a_blur_is_baked_again() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let picture = PictureId::new();
        let slot = store.slot(picture);
        assert!(slot.publish_pixels(bitmap(1, 1, &[RED]), None));
        let (mut document, _) = picture_page(
            ".blurred { filter: blur(0.25px); }",
            &[(picture, "blurred")],
        );
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        let [red, green, blue, _] = at(&painter.capture(0.0).expect("capture"), 4, 4);
        assert!(red > 240 && green < 16 && blue < 16, "baked red");

        assert!(slot.publish_pixels(bitmap(1, 1, &[BLUE]), None));
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        let [red, green, blue, _] = at(&painter.capture(0.0).expect("capture"), 4, 4);
        assert!(red < 16 && green < 16 && blue > 240, "baked again blue");
    }

    /// A picture the commit does not draw is in no drawn set, and is
    /// neither described, taken nor wanted, however many frames its
    /// producer publishes; nor does a frame of it ask for a frame.
    #[test]
    fn a_picture_that_is_not_drawn_is_not_asked_for() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let (shown, hidden) = (PictureId::new(), PictureId::new());
        assert!(store.slot(shown).publish_pixels(bitmap(1, 1, &[RED]), None));
        assert!(
            store
                .slot(hidden)
                .publish_pixels(bitmap(1, 1, &[BLUE]), None)
        );
        let (mut document, _) = picture_page("", &[(shown, ""), (hidden, "hidden")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(*store.drawn.borrow(), [vec![PictureKey::Id(shown)]]);
        assert!(painter.take_redraw());

        let _ = store
            .slot(hidden)
            .publish_pixels(bitmap(1, 1, &[RED]), None);
        for _ in 0..3 {
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert!(!painter.take_redraw(), "nothing it draws changed");
        }
        assert!(!store.described.borrow().contains(&hidden));
        assert!(store.generations_taken(hidden).is_empty());
        assert!(store.wants.borrow().is_empty());
    }

    /// Two draws of one picture are one picture to the store and one
    /// texture to the rasterizer: one describe per adoption, one take per
    /// frame, one upload, and both draws show it.
    #[test]
    fn two_draws_of_one_picture_share_one_upload() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let picture = PictureId::new();
        assert!(
            store
                .slot(picture)
                .publish_pixels(bitmap(1, 1, &[RED]), None)
        );
        let (mut document, _) = picture_page("", &[(picture, ""), (picture, "")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 4, 4), RED);
        assert_eq!(at(&screenshot, 12, 4), RED);
        assert_eq!(*store.described.borrow(), [picture]);
        assert_eq!(store.generations_taken(picture), [1]);
        assert_eq!(painter.images.sources.len(), 1);
        assert_eq!(painter.images.resolved().len(), 2);
        assert!(gpu.raster.picture_texture(picture).is_some());
    }

    /// A frame of another size replaces the picture's texture with one of
    /// that size under the same id; a frame of the size it has is written
    /// into it.
    #[test]
    fn a_picture_that_changes_size_keeps_its_texture_id() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let picture = PictureId::new();
        let slot = store.slot(picture);
        assert!(slot.publish_pixels(bitmap(1, 1, &[RED]), None));
        let (mut document, _) = picture_page("", &[(picture, "")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), RED);
        let (id, width, height) = gpu.raster.picture_texture(picture).expect("written");
        assert_eq!((width, height), (1, 1));

        assert!(slot.publish_pixels(bitmap(2, 1, &[RED, BLUE]), None));
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 1, 4), RED);
        assert_eq!(at(&screenshot, 6, 4), BLUE);
        assert_eq!(gpu.raster.picture_texture(picture), Some((id, 2, 1)));

        assert!(slot.publish_pixels(bitmap(2, 1, &[BLUE, RED]), None));
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 1, 4), BLUE);
        assert_eq!(at(&screenshot, 6, 4), RED);
        assert_eq!(gpu.raster.picture_texture(picture), Some((id, 2, 1)));
    }

    /// Frame demand: each new frame owes exactly one frame, turns between
    /// changes owe none and render nothing, and a live picture never counts
    /// as animating. The next change a picture names is the painter's to
    /// report, for a host's timer, until a frame without one arrives.
    #[test]
    fn frame_demand_fires_once_per_change_and_not_continuously() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let picture = PictureId::new();
        let slot = store.slot(picture);
        let next = ClockInstant::now() + std::time::Duration::from_mins(5);
        assert!(slot.publish_pixels(bitmap(1, 1, &[RED]), Some(next)));
        let (mut document, _) = picture_page("", &[(picture, "")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert!(painter.take_redraw());
        assert!(painter.draw_offscreen(0.0, false).expect("render"));
        assert_eq!(painter.next_picture_change(), Some(next));

        for _ in 0..3 {
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert!(!painter.take_redraw(), "no change, no frame");
            assert!(!painter.is_animating(|| false));
            assert!(
                !painter.draw_offscreen(0.0, false).expect("render"),
                "the target already holds this frame"
            );
        }

        for texel in [BLUE, RED] {
            assert!(slot.publish_pixels(bitmap(1, 1, &[texel]), None));
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert!(painter.take_redraw(), "one frame per change");
            assert!(!painter.is_animating(|| false));
            assert!(painter.draw_offscreen(0.0, false).expect("render"));
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert!(!painter.take_redraw(), "and only one");
        }
        assert_eq!(painter.next_picture_change(), None);
        assert_eq!(store.generations_taken(picture), [1, 2, 3]);
    }

    /// Captures taken between two frames of a live picture are of the same
    /// pixels, however many turns pass between them, and take nothing.
    #[test]
    fn a_capture_between_changes_is_deterministic() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Live::default();
        let picture = PictureId::new();
        let slot = store.slot(picture);
        assert!(slot.publish_pixels(bitmap(2, 1, &[RED, BLUE]), None));
        let (mut document, _) = picture_page("", &[(picture, "")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        let first = painter.capture(0.0).expect("capture");
        let second = painter.capture(0.0).expect("capture");
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        let third = painter.capture(0.0).expect("capture");
        assert_eq!(first.pixels, second.pixels);
        assert_eq!(first.pixels, third.pixels);
        assert_eq!(store.generations_taken(picture), [1]);
    }
}
