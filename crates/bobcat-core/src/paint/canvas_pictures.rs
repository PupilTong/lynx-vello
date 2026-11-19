//! The engine's side of a placeholder `canvas`'s commits: the `Send` handle
//! an `OffscreenCanvas`'s owner commits its bitmap through, and the
//! painter's answer for those pictures.
//!
//! The element and its link are `terminus`'s
//! ([`terminus::link::html::canvas`]): `transferControlToOffscreen()` puts a
//! canvas in placeholder mode and answers its [`CanvasFrames`]. What is not
//! a DOM concept is here. [`transfer_control_to_offscreen`] registers the
//! link in the view's [`CanvasPictures`], which the view's main side and its
//! seat share by `Arc`, and answers a [`CanvasPresenter`]: the canvas's owner
//! on any thread — `bobcat-main`, `bobcat-workers` — commits its bitmap
//! through it ([`CanvasPresenter::commit`]). The commit copies the canvas's
//! texture into the picture's on the GPU, then wakes the view's host when a
//! wake is owed and, for a bitmap of a new size, sends `bobcat-main` one
//! [`ToMain::CanvasResized`] so the document applies the size
//! ([`update_placeholders`]). The painter answers a registered picture from
//! its link ([`ViewImages`]): the generation of the newest commit, whose
//! pixels are already in the texture it draws; there is never a frame to
//! take. Every other picture is asked of the embedder's store.
//!
//! A canvas never transferred has no entry: its picture is asked of the
//! embedder's store, which knows nothing of it, and so draws nothing and is
//! not waited for. Neither is a placeholder nothing was committed to.
//!
//! [`update_placeholders`]: terminus::link::html::canvas::update_placeholders

use std::sync::{Arc, Mutex, PoisonError, Weak};

use bobcat_canvas::Canvas;
use rustc_hash::FxHashMap;
use terminus::link::html::canvas::{CanvasFrames, InvalidState};
use terminus::link::{
    FrameImage, FrameImages, FrameWriter, ImageSizeHint, NodeId, PictureId, PictureKey, TakenFrame,
};
use tokio::sync::mpsc;

use crate::link::{ToMain, ViewOutbox};
use crate::main::tree::LynxDocument;
use crate::view::EventRequester;

/// One placeholder canvas's link and where a commit's owed wakes go.
struct CanvasPicture {
    frames: CanvasFrames,
    requester: Arc<dyn EventRequester>,
    /// The view's command channel, held weakly: what a resize is sent on.
    main: Option<mpsc::WeakUnboundedSender<ToMain>>,
}

/// One view's placeholder canvases, by picture.
#[derive(Default)]
pub(crate) struct CanvasPictures(Mutex<FxHashMap<PictureId, Arc<CanvasPicture>>>);

impl CanvasPictures {
    fn lock(&self) -> std::sync::MutexGuard<'_, FxHashMap<PictureId, Arc<CanvasPicture>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, picture: PictureId) -> Option<Arc<CanvasPicture>> {
        self.lock().get(&picture).cloned()
    }
}

/// What a canvas's owner, on any thread, commits its bitmap through.
///
/// Holds its entry weakly, and the entry holds the element's link weakly:
/// once the element is freed a commit does nothing and wakes nobody, and
/// once the view is gone the handle keeps none of its state alive.
#[derive(Clone)]
pub(crate) struct CanvasPresenter(Weak<CanvasPicture>);

impl CanvasPresenter {
    /// Registers `frames` in `pictures` and answers the handle on it. A
    /// commit wakes `requester` when a wake is owed and sends
    /// [`ToMain::CanvasResized`] on `main` when a resize is. Entries of
    /// freed canvases are dropped here.
    pub(crate) fn register(
        pictures: &CanvasPictures,
        frames: CanvasFrames,
        requester: Arc<dyn EventRequester>,
        main: Option<mpsc::WeakUnboundedSender<ToMain>>,
    ) -> Self {
        let picture = frames.picture();
        let entry = Arc::new(CanvasPicture {
            frames,
            requester,
            main,
        });
        let presenter = Self(Arc::downgrade(&entry));
        let mut entries = pictures.lock();
        entries.retain(|_, entry| entry.frames.is_live());
        entries.insert(picture, entry);
        presenter
    }

    /// HTML's update of the placeholder: `canvas`, with every draw recorded
    /// on it, becomes what the placeholder shows (`Canvas::commit`). Nothing
    /// is queued: a wake is asked for only when the painter has described
    /// every earlier commit, and a resize sent only when none is pending.
    pub(crate) fn commit(&self, canvas: &mut Canvas) {
        let Some(entry) = self.0.upgrade() else {
            return;
        };
        let committed = canvas.commit(&entry.frames);
        if committed.resize
            && let Some(main) = entry
                .main
                .as_ref()
                .and_then(mpsc::WeakUnboundedSender::upgrade)
        {
            let _ = main.send(ToMain::CanvasResized);
        }
        if committed.wake {
            entry.requester.request_event();
        }
    }
}

impl std::fmt::Debug for CanvasPresenter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CanvasPresenter")
            .field("live", &(self.0.strong_count() > 0))
            .finish()
    }
}

/// What [`transfer_control_to_offscreen`] answers: the size the
/// `OffscreenCanvas` is created at and the handle its bitmap is committed
/// through.
#[derive(Debug)]
pub(crate) struct Transferred {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) presenter: CanvasPresenter,
}

/// HTML's `transferControlToOffscreen()` on `canvas` in the view `outbox`
/// publishes for, on `bobcat-main`: the element becomes a placeholder
/// ([`terminus::link::html::canvas::transfer_control_to_offscreen`]) and its
/// link is registered for the view's painter.
///
/// # Errors
///
/// [`InvalidState`] when `canvas` is not a canvas element in context mode
/// none.
pub(crate) fn transfer_control_to_offscreen(
    outbox: &ViewOutbox,
    document: &mut LynxDocument,
    canvas: NodeId,
) -> Result<Transferred, InvalidState> {
    let placeholder =
        terminus::link::html::canvas::transfer_control_to_offscreen(document, canvas)?;
    Ok(Transferred {
        width: placeholder.width,
        height: placeholder.height,
        presenter: CanvasPresenter::register(
            outbox.canvases(),
            placeholder.frames,
            Arc::clone(outbox.requester()),
            outbox.main().cloned(),
        ),
    })
}

/// What a painter asks about a view's pictures: the view's placeholder
/// canvases from their links, and every other picture from the embedder's
/// store, unchanged.
pub(crate) struct ViewImages<'a, P> {
    pub(crate) canvases: &'a CanvasPictures,
    pub(crate) store: &'a dyn FrameImages<PlatformFrame = P>,
}

impl<P: FrameWriter> ViewImages<'_, P> {
    fn canvas(&self, picture: &PictureKey) -> Option<Arc<CanvasPicture>> {
        self.canvases.get(picture.id()?)
    }
}

impl<P: FrameWriter> FrameImages for ViewImages<'_, P> {
    type PlatformFrame = P;

    fn describe(&self, picture: &PictureKey, hint: ImageSizeHint) -> Option<FrameImage<P>> {
        match self.canvas(picture) {
            Some(canvas) => canvas.frames.describe(),
            None => self.store.describe(picture, hint),
        }
    }

    /// A canvas's commits are copied into its texture: none is taken.
    fn take_frame(&self, picture: PictureId) -> Option<TakenFrame<P>> {
        match self.canvases.get(picture) {
            Some(_) => None,
            None => self.store.take_frame(picture),
        }
    }

    /// A canvas is never wanted from the store: its texture is the link's.
    fn want(&self, picture: &PictureKey, hint: ImageSizeHint) {
        if self.canvas(picture).is_none() {
            self.store.want(picture, hint);
        }
    }

    fn retain(&self, drawn: &[PictureKey]) {
        self.store.retain(drawn);
    }

    fn release(&self, pictures: &[PictureKey]) {
        self.store.release(pictures);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "canvas_tests.rs"]
mod tests;
