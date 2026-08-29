//! The image source the paint walk reads, implemented outside this workspace.
//!
//! Nothing in this crate holds decoded pixels. [`ImageStore`] is the whole
//! contract: the embedder owns the bytes, the cache, the eviction policy and
//! the decoder, and the engine asks it for one image at a time by the source
//! string CSS produced — a `url(…)` value for `background-image` and
//! `mask-image`, or a replaced element's source set through
//! [`Document::set_image_source`](crate::Document::set_image_source).
//!
//! # Why one trait has both a blocking-free and an async half
//!
//! The paint walk runs inside a commit on the document's owner thread —
//! [`Document::render`](crate::Document::render) — holding `&mut Scene` and
//! an open clip stack. It cannot suspend: a yield mid-walk would leave the
//! scene with unbalanced layer pushes, and it must not block either, because
//! a commit is what every frame waits on. So the walk calls exactly one
//! method, [`ImageStore::peek`], which must return already-resident pixels or
//! nothing at all — from whichever thread owns the document.
//!
//! Producing those pixels is the other half. [`ImageStore::get`] resolves,
//! fetches and decodes, and is awaited by the layer above this crate, outside
//! any frame; [`ImageStore::prefetch`] starts the same work without waiting for
//! it. When a `get` completes, the pixels are in the store, and the caller
//! tells the document with
//! [`Document::note_images_changed`](crate::Document::note_images_changed) so
//! the next frame rebuilds its scene and finds them through `peek`.
//!
//! A `peek` miss paints nothing and is not an error: layout's natural size and
//! the paint-side pixels arrive independently, so a frame between the two
//! skips the image, which is what a browser shows for a not-yet-loaded image.
//!
//! # What the store must not return
//!
//! Vello packs every scene image into one shared square atlas that grows by
//! doubling to [`MAX_RENDERABLE_DIMENSION`]. An image longer than that on
//! either axis never fits at any atlas size: the resolve pass leaves it
//! unallocated and zeroes the draw's dimensions, so it renders as nothing with
//! no error anywhere. Pixels past the bound are therefore skipped here rather
//! than handed to vello, and a store that decodes to a target size should cap
//! that target at the same number.

use std::future::Future;
use std::pin::Pin;

use vello::peniko::ImageData;

/// Largest per-axis pixel count vello can render.
///
/// Vello's image atlas starts at 1024 px and doubles until an image fits,
/// stopping at `vello_encoding`'s `MAX_ATLAS_SIZE` of 8192. An image within
/// this bound may still fail to allocate transiently when the atlas is full of
/// other images — vello resolves that itself by evicting or growing — but an
/// image past it can never be placed at all.
pub const MAX_RENDERABLE_DIMENSION: u32 = 8192;

/// Why [`ImageStore::get`] produced no pixels.
///
/// Boxed rather than enumerated: every failure mode belongs to the embedder's
/// own transport and codec, and this crate neither produces nor inspects one.
pub type ImageStoreError = Box<dyn std::error::Error + Send + Sync>;

/// The pending result of [`ImageStore::get`].
///
/// Not `Send`: the engine polls it on the thread it was created on, which is
/// the only thread that can then use the pixels, and requiring `Send` would
/// lock out the single-threaded executors the wasm embedder runs on.
pub type ImageFuture<'a> = Pin<Box<dyn Future<Output = Result<ImageData, ImageStoreError>> + 'a>>;

/// The embedder-supplied owner of every decoded image the document paints.
///
/// Install one with
/// [`Document::set_image_store`](crate::Document::set_image_store). A document
/// without one paints no images at all.
///
/// Implementations are shared across the script thread and the presenting
/// thread through an `Arc`, so they carry their own interior mutability.
pub trait ImageStore: Send + Sync {
    /// The pixels for `source` if they are resident, without fetching,
    /// decoding, blocking or allocating a buffer.
    ///
    /// Called once per image draw inside the paint walk, so it is on the frame
    /// path: it must not take a lock that any long operation holds, and must
    /// not start work. The returned [`ImageData`] shares the store's buffer
    /// through a reference count rather than copying pixels, which is what
    /// lets the store drop its own entry while a built scene still draws it.
    fn peek(&self, source: &str) -> Option<ImageData>;

    /// Resolves, fetches and decodes `source`, completing when its pixels are
    /// resident.
    ///
    /// Awaited outside the frame. A store is free to answer from its cache
    /// without doing any work, and to return the same buffer to concurrent
    /// callers asking for the same source.
    fn get<'a>(&'a self, source: &'a str) -> ImageFuture<'a>;

    /// Starts what [`ImageStore::get`] would do and returns immediately,
    /// discarding both the pixels and any failure.
    fn prefetch(&self, source: &str);
}

/// The store a document has until an embedder installs one: no image is ever
/// resident and no fetch is ever started.
#[derive(Debug)]
pub(crate) struct NoImages;

impl ImageStore for NoImages {
    fn peek(&self, _source: &str) -> Option<ImageData> {
        None
    }

    fn get<'a>(&'a self, source: &'a str) -> ImageFuture<'a> {
        Box::pin(
            async move { Err(format!("no image store is installed, cannot load {source}").into()) },
        )
    }

    fn prefetch(&self, _source: &str) {}
}

/// Whether vello can place these pixels in its atlas at all.
///
/// A zero axis is refused for the same reason as an oversized one: it occupies
/// a draw that can produce no pixels, and the brush transform that would scale
/// it divides by that axis.
#[must_use]
pub(crate) fn is_renderable(image: &ImageData) -> bool {
    image.width > 0
        && image.height > 0
        && image.width <= MAX_RENDERABLE_DIMENSION
        && image.height <= MAX_RENDERABLE_DIMENSION
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

    use super::{ImageFuture, ImageStore, MAX_RENDERABLE_DIMENSION, NoImages, is_renderable};

    /// Declares the given size without allocating that many pixels: every
    /// check under test reads the declared dimensions, not the buffer.
    fn image(width: u32, height: u32) -> ImageData {
        ImageData {
            data: Blob::from(vec![0_u8; 4]),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }
    }

    /// A store is shared between the script thread and the presenting thread
    /// through the document, and `Mutex<T>` is `Sync` only when `T` is `Send`.
    /// Sharing a document across those two threads is exactly what an embedder
    /// does with it.
    #[test]
    fn a_document_carrying_a_store_still_crosses_threads() {
        const fn assert_send<T: Send>() {}
        assert_send::<crate::Document<()>>();
    }

    #[test]
    fn the_default_store_is_resident_in_nothing() {
        assert!(NoImages.peek("app:///a.png").is_none());
        NoImages.prefetch("app:///a.png");
        let error = pollster::block_on(NoImages.get("app:///a.png"))
            .expect_err("a document with no store installed cannot load");
        assert!(format!("{error}").contains("app:///a.png"));
    }

    /// A real store fetches on a task that outlives the call, which needs
    /// owned data. This pins that the borrowed `source` does not stand in the
    /// way: an implementation copies it before its first await, and the
    /// returned future borrows only `self`. It also pins that a store reaches
    /// the engine as `Arc<dyn ImageStore>`, which requires `Send + Sync`.
    #[test]
    fn an_implementation_can_own_the_source_and_defer_the_work() {
        use std::sync::{Arc, Mutex};

        const fn assert_send_sync<T: Send + Sync>() {}

        #[derive(Default)]
        struct Deferred {
            asked: Mutex<Vec<String>>,
        }

        impl ImageStore for Deferred {
            fn peek(&self, _source: &str) -> Option<ImageData> {
                None
            }

            fn get<'a>(&'a self, source: &'a str) -> ImageFuture<'a> {
                // Copied before the first await, so everything after this
                // point could just as well run on a spawned task.
                let owned = source.to_owned();
                Box::pin(async move {
                    let work = async move { owned };
                    let owned = work.await;
                    self.asked.lock().expect("asked").push(owned.clone());
                    Err(format!("{owned} was deferred, not decoded").into())
                })
            }

            fn prefetch(&self, source: &str) {
                self.asked.lock().expect("asked").push(source.to_owned());
            }
        }

        let store: Arc<dyn ImageStore> = Arc::new(Deferred::default());
        store.prefetch("app:///warm.png");
        pollster::block_on(store.get("app:///cold.png")).expect_err("nothing is decoded here");
        assert_send_sync::<Arc<dyn ImageStore>>();
    }

    #[test]
    fn the_atlas_bound_is_inclusive_and_a_zero_axis_is_refused() {
        assert!(is_renderable(&image(1, 1)));
        assert!(is_renderable(&image(
            MAX_RENDERABLE_DIMENSION,
            MAX_RENDERABLE_DIMENSION
        )));
        assert!(!is_renderable(&image(MAX_RENDERABLE_DIMENSION + 1, 1)));
        assert!(!is_renderable(&image(1, MAX_RENDERABLE_DIMENSION + 1)));
        assert!(!is_renderable(&image(0, 4)));
        assert!(!is_renderable(&image(4, 0)));
    }
}
