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
//! # Platform frames
//!
//! A store whose frames are of the embedder's own type
//! ([`FrameImages::PlatformFrame`](crate::render::image::FrameImages::PlatformFrame))
//! lends or hands over a frame no render can read: nothing but its writer
//! ([`FrameWriter`](crate::render::picture::FrameWriter)) can copy it into a
//! texture. So a URL picture's platform frame is uploaded the moment it is
//! described ([`BitmapLeases::upload_frame`]), leased like any upload, and
//! released to the store in the same adoption; an `Id` picture's is written
//! into its texture as CPU pixels are. Without a rasterizer to lease from, a
//! platform frame draws nothing.
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

use std::sync::Arc;

use crate::clock::ClockInstant;
use crate::paint::display_list::ImageRef;
use crate::peniko::ImageData;
use crate::render::gpu::SharedGpu;
use crate::render::image::{FrameImage, FrameImages, ImageDescriptor, ImageSizeHint};
use crate::render::picture::{PictureFrame, PictureId, PictureKey};
use crate::render::sparse::BitmapLeases;

#[derive(Debug)]
struct Source {
    key: PictureKey,
    hint: ImageSizeHint,
    descriptor: Option<ImageDescriptor>,
    pixels: Option<ImageData>,
    leased: bool,
    owed: bool,
    wanted: bool,
    generation: u64,
    next_change: Option<ClockInstant>,
}

#[derive(Debug, Default)]
pub(super) struct PainterImages {
    key: Option<u64>,
    stale: bool,
    orphaned: bool,
    sources: Vec<Source>,
    draws: Vec<usize>,
    resolved: Vec<Option<ImageRef>>,
    names: Vec<PictureKey>,
    leases: Option<BitmapLeases>,
    checked: Option<u64>,
    revision: u64,
    watching: bool,
}

impl PainterImages {
    pub(super) fn holds(&self, commit: u64) -> bool {
        self.key == Some(commit) && !self.stale
    }

    pub(super) fn is_stale(&self) -> bool {
        self.stale
    }

    pub(super) fn awaits(&self) -> bool {
        !self.orphaned && (self.stale || self.sources.iter().any(|source| source.wanted))
    }

    pub(super) const fn revision(&self) -> u64 {
        self.revision
    }

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

    pub(super) fn resolve<S: FrameImages + ?Sized>(
        &mut self,
        frame: &crate::visual::CommittedFrame,
        store: &S,
    ) {
        let mut hints = Vec::new();
        frame.image_sources(&mut self.names, &mut hints, &mut self.draws);
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
                    owed: false,
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

    pub(super) fn refresh<S: FrameImages + ?Sized>(&mut self, store: &S) -> bool {
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
            if source.wanted && !was_wanted {
                store.want(&source.key, source.hint);
            }
        }
        self.watching = self.sources.iter().any(Source::watched);
        if changed {
            if let Some(leases) = &self.leases {
                lease_exactly(leases, &self.sources);
            }
            self.fill_resolved();
            self.checked = None;
            self.revision += 1;
        }
        changed
    }

    pub(super) fn orphan(&mut self) {
        self.orphaned = true;
        for source in &mut self.sources {
            source.wanted = false;
        }
    }

    pub(super) fn release_held<S: FrameImages + ?Sized>(&mut self, store: Option<&S>) {
        let Some(leases) = &self.leases else {
            return;
        };
        let uploads = leases.uploads();
        if self.checked == Some(uploads) && !self.sources.iter().any(|source| source.owed) {
            return;
        }
        self.checked = Some(uploads);
        self.names.clear();
        for source in &mut self.sources {
            if source.owed {
                source.owed = false;
                self.names.push(source.key.clone());
                continue;
            }
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

    pub(super) fn mark_stale(&mut self) {
        self.stale = true;
    }

    pub(super) fn resolved(&self) -> &[Option<ImageRef>] {
        &self.resolved
    }

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
    fn watched(&self) -> bool {
        self.wanted || self.key.id().is_some() || self.next_change.is_some()
    }

    fn drawn(&self) -> (Option<ImageDescriptor>, u64) {
        (self.descriptor, self.generation)
    }

    fn image_ref(&self) -> Option<ImageRef> {
        let descriptor = self.descriptor?;
        Some(match (&self.key, &self.pixels) {
            (_, Some(pixels)) => ImageRef::lent(pixels),
            (PictureKey::Id(id), None) => ImageRef::written(*id, descriptor),
            (PictureKey::Url(_), None) => ImageRef::uploaded(descriptor),
        })
    }

    fn describe<S: FrameImages + ?Sized>(&mut self, store: &S, leases: Option<&BitmapLeases>) {
        self.wanted = false;
        match self.key.id() {
            Some(id) => self.describe_picture(id, store, leases),
            None => self.describe_url(store, leases),
        }
    }

    fn describe_url<S: FrameImages + ?Sized>(&mut self, store: &S, leases: Option<&BitmapLeases>) {
        let Some(FrameImage {
            descriptor,
            pixels,
            generation,
            next_change,
        }) = store.describe(&self.key, self.hint)
        else {
            self.descriptor = None;
            self.pixels = None;
            self.next_change = None;
            return;
        };
        self.next_change = next_change;
        match pixels {
            Some(PictureFrame::Pixels(pixels)) => {
                let descriptor = ImageDescriptor::of(&pixels);
                self.leased &= self.descriptor == Some(descriptor);
                self.descriptor = Some(descriptor);
                self.pixels = Some(pixels);
                self.generation = generation;
                return;
            }
            Some(PictureFrame::Platform(frame)) => {
                if self.descriptor == Some(descriptor) && self.leased {
                    return;
                }
                if leases.is_some_and(|leases| leases.upload_frame(&descriptor, &frame)) {
                    self.descriptor = Some(descriptor);
                    self.pixels = None;
                    self.leased = true;
                    self.owed = true;
                    self.generation = generation;
                } else {
                    self.descriptor = None;
                    self.pixels = None;
                    self.leased = false;
                }
                return;
            }
            None => {}
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

    fn describe_picture<S: FrameImages + ?Sized>(
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
        if let Some((generation, descriptor)) = leases.and_then(|leases| leases.lease_picture(id)) {
            self.descriptor = Some(descriptor);
            self.generation = generation;
            self.leased = true;
            if generation == state.generation {
                return;
            }
        }
        if let Some(taken) = store.take_frame(id) {
            if let Some(leases) = leases {
                if leases.write_picture(id, taken.generation, taken.descriptor, &taken.frame) {
                    self.descriptor = Some(taken.descriptor);
                    self.generation = taken.generation;
                    self.leased = true;
                }
            } else if let PictureFrame::Pixels(pixels) = taken.frame {
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
    use crate::render::picture::{PictureFrame, PictureId, PictureKey, PictureSlot, TakenFrame};
    use crate::render::sparse::{BitmapLeases, SharedRaster};
    use crate::style::engine::StylesheetOrigin;
    use crate::tree::document::Document;
    use crate::tree::document::tests::device;

    const SRC: &str = "app:///a.png";
    const OTHER: &str = "app:///b.png";
    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    struct Releasing {
        entries: RefCell<HashMap<&'static str, Entry>>,
        wants: RefCell<Vec<String>>,
        unanswered: RefCell<Vec<String>>,
        released: RefCell<Vec<Vec<Arc<str>>>>,
    }

    struct Entry {
        texel: [u8; 4],
        pixels: Option<ImageData>,
        descriptor: ImageDescriptor,
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

        fn answer(&self) {
            let mut entries = self.entries.borrow_mut();
            for source in self.unanswered.borrow_mut().drain(..) {
                let entry = entries.get_mut(source.as_str()).expect("a known source");
                let image = texel_image(entry.texel);
                entry.descriptor = ImageDescriptor::of(&image);
                entry.pixels = Some(image);
            }
        }

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
                pixels: entry.pixels.clone().map(PictureFrame::Pixels),
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
            animated: false,
        }
    }

    fn uploaded(gpu: &Arc<SharedGpu>, image: &ImageDescriptor) -> bool {
        BitmapLeases::new(gpu).lease(image)
    }

    fn at(screenshot: &Screenshot, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * screenshot.size.width + x) * 4) as usize;
        screenshot.pixels[at..at + 4]
            .try_into()
            .expect("four channels")
    }

    fn held_at(painter: &mut Painter, x: u32, y: u32) -> [u8; 4] {
        let width = painter.frame_size.width;
        let Output::Offscreen(gpu) = &mut painter.output else {
            panic!("an offscreen painter");
        };
        let pixels = gpu.read_pixels().expect("the target reads back");
        let at = ((y * width + x) * 4) as usize;
        pixels[at..at + 4].try_into().expect("four channels")
    }

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

    fn present(document: &mut Document<()>, image: crate::tree::document::NodeId, source: &str) {
        let natural = document.request_image(source).natural_size();
        document.set_picture(image, Some((PictureKey::Url(Arc::from(source)), natural)));
    }

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

    #[test]
    fn a_source_with_neither_pixels_nor_an_upload_draws_nothing_until_the_store_restores_it() {
        let gpu = SharedGpu::private(SharedRaster::new);
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let (mut document, image) = page("");
        let drawn = evict_src(&gpu, &mut painter, &store, &mut document, image);

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
                pixels: self.held.get().then_some(PictureFrame::Pixels(frame)),
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
            let current = painter.frame().cloned();
            assert!(painter.adopt(current, Some(&store)));
            assert!(!store.held.get(), "released");
            drawn.push(ImageDescriptor::of(&store.frames.borrow()[index]));
        }
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

    #[derive(Default)]
    struct Live {
        slots: RefCell<HashMap<PictureId, PictureSlot>>,
        described: RefCell<Vec<PictureId>>,
        taken: RefCell<Vec<(PictureId, u64)>>,
        wants: RefCell<Vec<PictureId>>,
        drawn: RefCell<Vec<Vec<PictureKey>>>,
    }

    impl Live {
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
        assert_eq!(store.generations_taken(hidden), [] as [u64; 0]);
        assert!(store.wants.borrow().is_empty());
    }

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
        assert_eq!(store.generations_taken(picture), [1, 2, 3]);
    }

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

    #[derive(Clone, Debug)]
    struct Solid {
        texel: [u8; 4],
        writes: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Solid {
        fn new(texel: [u8; 4]) -> Self {
            Self {
                texel,
                writes: Arc::default(),
            }
        }

        fn writes(&self) -> usize {
            self.writes.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl crate::render::picture::FrameWriter for Solid {
        fn write(&self, target: &crate::render::picture::FrameTarget<'_>) -> bool {
            let (width, height) = target.size;
            assert_eq!(
                (target.texture.width(), target.texture.height()),
                target.size
            );
            assert!(
                target
                    .texture
                    .usage()
                    .contains(wgpu::TextureUsages::RENDER_ATTACHMENT),
                "the destination a browser copy requires"
            );
            let texels: Vec<u8> = self
                .texel
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect();
            target.queue.write_texture(
                target.texture.as_image_copy(),
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        }
    }

    struct Platform {
        frame: RefCell<Option<Solid>>,
        descriptor: ImageDescriptor,
        slots: RefCell<HashMap<PictureId, PictureSlot<Solid>>>,
        released: RefCell<Vec<Vec<PictureKey>>>,
        wants: RefCell<Vec<PictureKey>>,
    }

    impl Platform {
        fn new(frame: Solid) -> Self {
            Self {
                frame: RefCell::new(Some(frame)),
                descriptor: ImageDescriptor::platform(1, 1),
                slots: RefCell::default(),
                released: RefCell::default(),
                wants: RefCell::default(),
            }
        }

        fn slot(&self, picture: PictureId) -> PictureSlot<Solid> {
            self.slots.borrow_mut().entry(picture).or_default().clone()
        }
    }

    impl FrameImages for Platform {
        type PlatformFrame = Solid;

        fn describe(&self, picture: &PictureKey, _: ImageSizeHint) -> Option<FrameImage<Solid>> {
            match picture {
                PictureKey::Url(url) => {
                    (&**url == SRC).then_some(())?;
                    Some(FrameImage {
                        descriptor: self.descriptor,
                        pixels: self.frame.borrow().clone().map(PictureFrame::Platform),
                        generation: 0,
                        next_change: None,
                    })
                }
                PictureKey::Id(id) => self.slots.borrow().get(id)?.describe(),
            }
        }

        fn take_frame(&self, picture: PictureId) -> Option<TakenFrame<Solid>> {
            self.slots.borrow().get(&picture)?.take()
        }

        fn want(&self, picture: &PictureKey, _: ImageSizeHint) {
            self.wants.borrow_mut().push(picture.clone());
        }

        fn retain(&self, _: &[PictureKey]) {}

        fn release(&self, pictures: &[PictureKey]) {
            if pictures.iter().any(|picture| picture.url() == Some(SRC)) {
                self.frame.borrow_mut().take();
            }
            self.released.borrow_mut().push(pictures.to_vec());
        }
    }

    #[test]
    fn a_platform_frame_a_store_lends_is_uploaded_drawn_and_released() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let frame = Solid::new(RED);
        let store = Platform::new(frame.clone());
        let (mut document, _) = page("");
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(frame.writes(), 1, "written in the adoption");
        assert_eq!(
            *store.released.borrow(),
            [vec![PictureKey::Url(Arc::from(SRC))]],
            "released in the same adoption"
        );
        assert!(store.frame.borrow().is_none(), "the store let go of it");
        assert!(uploaded(&gpu, &store.descriptor));
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), RED);

        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), RED);
        assert_eq!(frame.writes(), 1, "drawn from its upload");
        assert!(store.wants.borrow().is_empty());
        assert!(!painter.awaits_pictures());
    }

    #[cfg(feature = "painter-test-utils")]
    #[test]
    fn a_platform_frame_draws_nothing_without_a_rasterizer() {
        let mut painter = Painter::without_output(
            16.0,
            16.0,
            1.0,
            GesturePolicy {
                tap_slop: 50.0,
                long_press_move_slop: 8.0,
                drag_slop: 8.0,
                long_press_seconds: 0.5,
                tap_event: "press",
                long_press_event: "hold",
            },
        );
        let frame = Solid::new(RED);
        let store = Platform::new(frame.clone());
        let (mut document, _) = page("");
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert!(painter.images.resolved().iter().all(Option::is_none));
        assert_eq!(frame.writes(), 0);
        assert!(store.wants.borrow().is_empty());
        assert!(!painter.awaits_pictures());
    }

    #[test]
    fn a_platform_frame_published_into_a_slot_is_written_into_the_picture_texture() {
        let gpu = SharedGpu::private(SharedRaster::new);
        let mut painter = painter_on(&gpu);
        let store = Platform::new(Solid::new(RED));
        let picture = PictureId::new();
        let slot = store.slot(picture);
        assert!(slot.publish_pixels(bitmap(1, 1, &[RED]), None));
        let (mut document, _) = picture_page("", &[(picture, "")]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), RED);
        let (id, ..) = gpu.raster.picture_texture(picture).expect("written");

        let blue = Solid::new(BLUE);
        assert!(slot.publish(
            PictureFrame::Platform(blue.clone()),
            ImageDescriptor::platform(1, 1),
            None,
        ));
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(blue.writes(), 1);
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), BLUE);
        assert_eq!(
            gpu.raster.picture_texture(picture),
            Some((id, 1, 1)),
            "a CPU frame's texture is replaced under the same id"
        );

        let red = Solid::new(RED);
        assert!(slot.publish(
            PictureFrame::Platform(red.clone()),
            ImageDescriptor::platform(2, 2),
            None,
        ));
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(red.writes(), 1);
        assert_eq!(at(&painter.capture(0.0).expect("capture"), 4, 4), RED);
        assert_eq!(gpu.raster.picture_texture(picture), Some((id, 2, 2)));
        assert!(store.wants.borrow().is_empty());
    }
}
