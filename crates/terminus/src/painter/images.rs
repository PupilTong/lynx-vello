//! The painter's per-commit pixels: what one committed frame draws, asked of
//! the host's resource system once per commit, and let go of once the
//! rasterizer holds them.
//!
//! The whole image resource system lives on the painting thread, owned by
//! the host and serviced in its own turns. The thread that builds the
//! document holds names and load states; it never sees a store, a buffer or
//! a `peniko::ImageData`. A painter asks that store through
//! [`FrameImages`](crate::render::image::FrameImages) what each source draws
//! as, tells it which sources it has nothing to draw from, and tells it when
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
//! 1. Per commit, each distinct source is described
//!    ([`FrameImages::describe`](crate::render::image::FrameImages::describe)). Pixels the store
//!    holds are held here too; a source whose pixels it dropped is drawn from its upload when that
//!    can be leased ([`BitmapLeases`]). When it cannot, the source is *wanted*: the frame draws
//!    nothing for it and the store is told so, once
//!    ([`FrameImages::want`](crate::render::image::FrameImages::want)).
//! 2. Every turn, each wanted source is described again, and one the store holds pixels for now
//!    takes its draws; the painter then composes again ([`Painter::adopt`](super::Painter::adopt)).
//!    A source the store now answers nothing for is no longer wanted, and draws nothing.
//! 3. Every turn, each source still held whose upload has been made is leased, its pixels let go,
//!    and the store told it may drop them
//!    ([`FrameImages::release`](crate::render::image::FrameImages::release)).
//! 4. A render that found neither an upload nor the pixels it was lent marks the table stale, and
//!    the next turn describes the commit again.
//!
//! All of them run in [`Painter::adopt`](super::Painter::adopt), before a
//! swap-chain image is acquired. The composed list and the filter bakes are
//! lent the pixels ([`ImageRef::lent`]) rather than given them, so they keep
//! nothing alive.

use std::sync::Arc;

use crate::paint::display_list::ImageRef;
use crate::peniko::ImageData;
use crate::render::gpu::SharedGpu;
use crate::render::image::{FrameImage, FrameImages, ImageDescriptor, ImageSizeHint};
use crate::render::sparse::BitmapLeases;

/// One distinct source the current commit draws.
#[derive(Debug)]
struct Source {
    name: Arc<str>,
    hint: ImageSizeHint,
    /// The bitmap the frame draws, once the store answered one it can draw.
    descriptor: Option<ImageDescriptor>,
    /// The pixels, while the rasterizer may still need them for an upload.
    pixels: Option<ImageData>,
    /// Whether this painter's leases hold its upload.
    leased: bool,
    /// The store answered it without pixels and the rasterizer holds no
    /// upload of it: the frame draws nothing for it, and the store was told
    /// it is wanted. Cleared by the describe that answers pixels, an upload,
    /// or nothing to draw.
    wanted: bool,
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
    /// One entry per distinct source of that commit, in first-draw order.
    sources: Vec<Source>,
    /// Per image draw of that commit, in draw order, the index of its
    /// source in `sources`.
    draws: Vec<usize>,
    /// One entry per image draw of that commit, in draw order, its pixels
    /// lent.
    resolved: Vec<Option<ImageRef>>,
    /// Scratch, reused across commits: the distinct sources' names, which
    /// the store is told as the working set, and the sources it may release.
    names: Vec<Arc<str>>,
    /// The leases on the output's uploads; `None` for an output with no
    /// rasterizer.
    leases: Option<BitmapLeases>,
    /// The upload count at the last lease attempt, or `None` once the
    /// entries changed since: what lets [`Self::release_held`] skip the
    /// attempt.
    checked: Option<u64>,
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

    /// Whether the frame lacks a picture that may still arrive: a source is
    /// wanted, or a render lost pixels and the commit has not been described
    /// again. False once the store has gone, since then nothing will.
    pub(super) fn awaits(&self) -> bool {
        !self.orphaned && (self.stale || self.sources.iter().any(|source| source.wanted))
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

    /// Describes every image `frame` draws to `store`, once per commit, and
    /// tells it which of them the frame has nothing to draw from.
    ///
    /// The commit is the whole key. A report that changes what a frame draws
    /// dirties the document, and every rebuild takes a new commit id, so a
    /// separate image counter would only ever agree with this one. Pixels
    /// that arrive for a wanted source within one commit are taken by
    /// [`Self::refresh_wanted`].
    ///
    /// Never waits: every call it makes of `store` is non-blocking.
    pub(super) fn resolve(
        &mut self,
        frame: &crate::visual::CommittedFrame,
        store: &dyn FrameImages,
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
            .map(|(name, hint)| {
                let mut source = Source {
                    name: Arc::clone(name),
                    hint,
                    descriptor: None,
                    pixels: None,
                    leased: false,
                    wanted: false,
                };
                source.describe(store, self.leases.as_ref());
                if source.wanted {
                    store.want(&source.name, source.hint);
                }
                source
            })
            .collect();
        if let Some(leases) = &self.leases {
            leases.replace(
                sources
                    .iter()
                    .filter(|source| source.leased)
                    .filter_map(|source| source.descriptor.as_ref()),
            );
        }
        self.sources = sources;
        self.fill_resolved();
        store.retain(&self.names);
        self.key = Some(frame.commit_id());
        self.stale = false;
        self.orphaned = false;
        self.checked = None;
    }

    /// Describes every wanted source again and takes what the store now
    /// answers; answers whether a draw changed, which is when the frame has
    /// to be composed again.
    ///
    /// Asks nothing of a source that is not wanted, and wants nothing again:
    /// a source stays wanted, with its restore in the store's hands, until
    /// a describe answers its pixels, an upload the rasterizer holds, or
    /// nothing to draw.
    pub(super) fn refresh_wanted(&mut self, store: &dyn FrameImages) -> bool {
        let mut changed = false;
        for source in &mut self.sources {
            if !source.wanted {
                continue;
            }
            source.describe(store, self.leases.as_ref());
            changed |= source.descriptor.is_some();
        }
        if changed {
            self.fill_resolved();
            self.checked = None;
        }
        changed
    }

    /// The store has gone: nothing a wanted source waits for will arrive,
    /// and nothing is waited for.
    pub(super) fn orphan(&mut self) {
        self.orphaned = true;
        for source in &mut self.sources {
            source.wanted = false;
        }
    }

    /// Lets go of the pixels of every source whose upload the rasterizer
    /// now holds: leases them, drops this table's reference, and tells
    /// `store`, if there is one, that it may drop its own.
    ///
    /// Cheap enough for every turn: nothing is attempted unless a bitmap
    /// was uploaded, or the table changed, since the last attempt. Without
    /// leases — an output with no rasterizer — nothing is let go.
    pub(super) fn release_held(&mut self, store: Option<&dyn FrameImages>) {
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
                self.names.push(Arc::clone(&source.name));
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
        if let Some(leases) = &self.leases {
            leases.replace([]);
        }
    }

    /// Rebuilds the per-draw table from the sources: each draw lent its
    /// source's pixels, or named by its upload, or nothing.
    fn fill_resolved(&mut self) {
        let Self {
            sources,
            draws,
            resolved,
            ..
        } = self;
        resolved.clear();
        resolved.extend(draws.iter().map(|&index| {
            let source = &sources[index];
            let descriptor = source.descriptor?;
            Some(match &source.pixels {
                Some(pixels) => ImageRef::lent(pixels),
                None => ImageRef::uploaded(descriptor),
            })
        }));
    }
}

impl Source {
    /// Asks `store` what this source draws as: its pixels when the store
    /// holds them, its upload when the store dropped the pixels and
    /// `leases` can hold it, and otherwise nothing yet — the source is
    /// wanted. A source the store answers nothing for draws nothing and is
    /// not wanted.
    fn describe(&mut self, store: &dyn FrameImages, leases: Option<&BitmapLeases>) {
        self.wanted = false;
        let Some(FrameImage { descriptor, pixels }) = store.describe(&self.name, self.hint) else {
            return;
        };
        if let Some(pixels) = pixels {
            self.descriptor = Some(ImageDescriptor::of(&pixels));
            self.pixels = Some(pixels);
            return;
        }
        if leases.is_some_and(|leases| leases.lease(&descriptor)) {
            self.descriptor = Some(descriptor);
            self.leased = true;
            return;
        }
        self.wanted = true;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::super::{FrameSize, GesturePolicy, Output, Painter, Screenshot, Viewport};
    use crate::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
    use crate::render::gpu::{Headless, SharedGpu};
    use crate::render::image::{
        FrameImage, FrameImages, ImageDescriptor, ImageEvent, ImageRole, ImageSizeHint,
    };
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
        fn describe(&self, source: &str, _: ImageSizeHint) -> Option<FrameImage> {
            let entries = self.entries.borrow();
            let entry = entries.get(source)?;
            if entry.lost {
                return None;
            }
            Some(FrameImage {
                descriptor: entry.descriptor,
                pixels: entry.pixels.clone(),
            })
        }

        fn want(&self, source: &str, _: ImageSizeHint) {
            self.wants.borrow_mut().push(source.to_owned());
            self.unanswered.borrow_mut().push(source.to_owned());
        }

        fn retain(&self, _: &[Arc<str>]) {}

        fn release(&self, sources: &[Arc<str>]) {
            self.released.borrow_mut().push(sources.to_vec());
            let mut entries = self.entries.borrow_mut();
            for source in sources {
                if let Some(entry) = entries.get_mut(&**source) {
                    entry.pixels = None;
                }
            }
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
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.apply_image_events(&[loaded(SRC)]);
        (document, image)
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
        document.set_image_source(image, ImageRole::Source, Some(OTHER));
        document.apply_image_events(&[loaded(OTHER)]);
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
        document.set_image_source(image, ImageRole::Source, Some(SRC));
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

        document.set_image_source(image, ImageRole::Source, Some(SRC));
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
        document.set_image_source(image, ImageRole::Source, Some(SRC));
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

        document.set_image_source(image, ImageRole::Source, Some(SRC));
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert!(painter.awaits_pictures());
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, None));
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
        store.release(&[Arc::from(SRC)]);
        let (mut images, mut sources) = (Vec::new(), Vec::new());

        assert_eq!(frame.resolve_images(&store, &mut images, &mut sources), 1);
        assert_eq!(*store.wants.borrow(), [SRC.to_owned()]);
        assert!(images.iter().all(Option::is_none), "drawn as nothing");

        store.answer();
        assert_eq!(frame.resolve_images(&store, &mut images, &mut sources), 0);
        assert!(images.iter().all(Option::is_some), "drawn once it is there");
        assert_eq!(store.wants(), 1);
    }
}
