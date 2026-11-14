//! The painter's per-commit pixels: what one committed frame draws, read out
//! of the host's resource system once per commit, and let go of once the
//! rasterizer holds them.
//!
//! The whole image resource system lives on the painting thread, owned by
//! the host and serviced in its own turns. The thread that builds the
//! document holds names and load states; it never sees a store, a buffer or
//! a `peniko::ImageData`. A painter asks that store through
//! [`FrameImages`](crate::render::image::FrameImages) what each source draws
//! as, reads pixels out of it only when nothing else has them, and tells it
//! when it may drop them.
//!
//! A host that decodes off-thread synchronises that itself. It already drives
//! the painter's turns, so it has somewhere to do it; putting the machinery
//! here would charge every host for a capability the browser — the one host
//! that will actually load images — does not use, since its decode callbacks
//! land on the embedder's own event loop.
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
//!    can be leased ([`BitmapLeases`]), and read back
//!    ([`FrameImages::read`](crate::render::image::FrameImages::read), the blocking restore) when
//!    it cannot.
//! 2. Every turn, each source still held whose upload has been made is leased, its pixels let go,
//!    and the store told it may drop them
//!    ([`FrameImages::release`](crate::render::image::FrameImages::release)).
//! 3. A render that found neither an upload nor the pixels it was lent marks the table stale, and
//!    the next turn describes the commit again, restoring what is missing.
//!
//! Both of the first two run in [`Painter::adopt`](super::Painter::adopt),
//! before a swap-chain image is acquired. The composed list and the filter
//! bakes are lent the pixels ([`ImageRef::lent`]) rather than given them,
//! so they keep nothing alive.

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
    /// The bitmap the frame draws, once the store answered one.
    descriptor: Option<ImageDescriptor>,
    /// The pixels, while the rasterizer may still need them for an upload.
    pixels: Option<ImageData>,
    /// Whether this painter's leases hold its upload.
    leased: bool,
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
    /// One entry per distinct source of that commit, in first-draw order.
    sources: Vec<Source>,
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

    /// Describes every image `frame` draws to `store`, once per commit.
    ///
    /// The commit is the whole key. A report that changes what a frame draws
    /// dirties the document, and every rebuild takes a new commit id, so a
    /// separate image counter would only ever agree with this one.
    ///
    /// **May block.** A source whose pixels the store dropped and whose
    /// upload cannot be leased is read, and a store is allowed —
    /// required, in fact — to restore a bitmap it evicted inside that read.
    /// When it is safe to pay that is the painter's own adoption to say
    /// ([`Painter::adopt`](super::Painter::adopt)): adopting a commit is the
    /// only thing that calls this, and it runs before a swap-chain image is
    /// acquired.
    pub(super) fn resolve(
        &mut self,
        frame: &crate::visual::CommittedFrame,
        store: &dyn FrameImages,
    ) {
        let mut hints = Vec::new();
        let mut indices = Vec::new();
        frame.image_sources(&mut self.names, &mut hints, &mut indices);
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
                };
                source.describe(store, self.leases.as_ref());
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
        self.resolved.clear();
        self.resolved.extend(indices.iter().map(|&index| {
            let source = &self.sources[index];
            let descriptor = source.descriptor?;
            Some(match &source.pixels {
                Some(pixels) => ImageRef::lent(pixels),
                None => ImageRef::uploaded(descriptor),
            })
        }));
        store.retain(&self.names);
        self.key = Some(frame.commit_id());
        self.stale = false;
        self.checked = None;
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
        self.sources.clear();
        self.resolved.clear();
        self.names.clear();
        self.checked = None;
        if let Some(leases) = &self.leases {
            leases.replace([]);
        }
    }
}

impl Source {
    /// Asks `store` what this source draws as: its pixels when the store
    /// holds them, its upload when the store dropped the pixels and
    /// `leases` can hold it, and otherwise the pixels read back.
    fn describe(&mut self, store: &dyn FrameImages, leases: Option<&BitmapLeases>) {
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
        // The rasterizer does not hold it either: the blocking restore.
        if let Some(pixels) = store.read(&self.name, self.hint) {
            self.descriptor = Some(ImageDescriptor::of(&pixels));
            self.pixels = Some(pixels);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::{Cell, RefCell};
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
    /// restores them into a new blob when they are read: a production host's
    /// policy, with every read counted and every release logged.
    struct Releasing {
        entries: RefCell<HashMap<&'static str, Entry>>,
        reads: Cell<u32>,
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
                    };
                    (source, entry)
                })
                .collect();
            Self {
                entries: RefCell::new(entries),
                reads: Cell::new(0),
                released: RefCell::default(),
            }
        }

        fn holds(&self, source: &str) -> bool {
            self.entries.borrow()[source].pixels.is_some()
        }

        fn descriptor(&self, source: &str) -> ImageDescriptor {
            self.entries.borrow()[source].descriptor
        }
    }

    impl FrameImages for Releasing {
        fn read(&self, source: &str, _: ImageSizeHint) -> Option<ImageData> {
            self.reads.set(self.reads.get() + 1);
            let mut entries = self.entries.borrow_mut();
            let entry = entries.get_mut(source)?;
            let texel = entry.texel;
            let image = entry
                .pixels
                .get_or_insert_with(|| texel_image(texel))
                .clone();
            entry.descriptor = ImageDescriptor::of(&image);
            Some(image)
        }

        fn retain(&self, _: &[Arc<str>]) {}

        fn describe(&self, source: &str, _: ImageSizeHint) -> Option<FrameImage> {
            let entries = self.entries.borrow();
            let entry = entries.get(source)?;
            Some(FrameImage {
                descriptor: entry.descriptor,
                pixels: entry.pixels.clone(),
            })
        }

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

    /// A source whose pixels the store dropped and whose upload the
    /// rasterizer released is read back once, inside the adoption of the
    /// commit that draws it again; it draws from those pixels, and once the
    /// new upload exists the pixels are let go and the store told so again.
    #[test]
    fn a_source_with_neither_pixels_nor_an_upload_is_read_back_once_and_released_again() {
        let gpu = SharedGpu::private(SharedRaster::new);
        // Every upload no lease and no recent render holds is released after
        // each render. The stack is this test's alone.
        gpu.raster.set_bitmap_budget(0);
        let mut painter = painter_on(&gpu);
        let store = Releasing::new([(SRC, RED), (OTHER, BLUE)]);
        let mut document = Document::new(device(), "page", ());
        document.add_stylesheet(
            "image { width: 8px; height: 8px; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        document.apply_image_events(&[loaded(SRC)]);

        // 1. Drawn from the store's pixels, uploaded, then leased: the painter and the store both
        //    let the pixels go.
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 4, 4), RED);
        let drawn = store.descriptor(SRC);
        assert!(uploaded(&gpu, &drawn), "the render uploaded it");
        let current = painter.frame().cloned();
        assert!(painter.adopt(current, Some(&store)));
        assert_eq!(*store.released.borrow(), [vec![Arc::<str>::from(SRC)]]);
        assert!(!store.holds(SRC), "the store dropped its pixels");
        let source = &painter.images.sources[0];
        assert!(source.pixels.is_none() && source.leased, "{source:?}");
        assert_eq!(store.reads.get(), 0, "describing was enough");

        // 2. A commit that no longer draws it ends the lease, and renders of another bitmap, past
        //    the ones the target pins what it drew in, release the upload.
        document.set_image_source(image, ImageRole::Source, Some(OTHER));
        document.apply_image_events(&[loaded(OTHER)]);
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        for _ in 0..8 {
            assert!(painter.draw_offscreen(0.0, true).expect("render"));
        }
        assert!(!uploaded(&gpu, &drawn), "the upload was released");
        assert!(!store.holds(SRC));
        assert_eq!(store.reads.get(), 0);

        // 3. The commit that draws it again reads it back, once, in the adoption, and draws those
        //    pixels with nothing lost.
        document.set_image_source(image, ImageRole::Source, Some(SRC));
        assert!(painter.adopt(Some(document.commit()), Some(&store)));
        assert_eq!(
            store.reads.get(),
            1,
            "a source with neither pixels nor an upload is read in the adoption"
        );
        let restored = store.descriptor(SRC);
        assert_ne!(restored, drawn, "a restore is a new blob");
        let screenshot = painter.capture(0.0).expect("capture");
        assert_eq!(at(&screenshot, 4, 4), RED, "drawn from the pixels read");
        assert_eq!(painter.output.lost_pixels(), 0, "no draw lost its pixels");
        assert!(!painter.images.is_stale());
        assert!(uploaded(&gpu, &restored), "the render uploaded them");

        // 4. The next turn leases the new upload and lets the pixels go again, with no further
        //    read.
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
        assert_eq!(store.reads.get(), 1, "no further read");
    }
}
