//! The Canvas 2D API's host state: one [`CanvasTable`] per realm, holding
//! the Canvas 2D objects (`terminus::link::html::canvas`) that realm's
//! `OffscreenCanvas`, `Path2D`, `CanvasGradient`, `CanvasPattern` and
//! `ImageBitmap` objects name by id.
//!
//! The table is installed with the core every realm has
//! ([`crate::realm::open_realm`], and each `ShadowRealm` child its own), so
//! `bobcat:canvas` links in every realm kind. Its members are
//! [`members::install`]'s; `crates/bobcat-core/docs/canvas.md` is the topic.
//!
//! - **Ids** are handed out from 1 and never reused, so a stale JavaScript object reaches nothing
//!   rather than another object's entry. A drop (finalization, `ImageBitmap.close()`) removes the
//!   entry; the table goes with the realm.
//! - **Entries** are owned `Send` values ([`Entry`]), so an entry can leave this table for another
//!   realm's.
//! - **The per-entry step** is [`CanvasTable::settle_drawn`]: the owner's epilogue runs it after
//!   every entry. Each canvas drawn during the entry replays what it recorded, so a recording
//!   nothing reads stays one entry long, and one bound to a placeholder `<canvas>` that changed is
//!   committed to it.
//! - **Transfer** ([`Transfer`]): a `postMessage` whose transfer list names `OffscreenCanvas` and
//!   `ImageBitmap` objects takes their entries out of the sending realm's table
//!   ([`CanvasTable::take_posted`]) and the message carries them; the receiving realm's reviver
//!   files each in its own table as the message is read ([`Transfer::deliver`]).

mod css;
mod members;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell, RefMut};
use std::rc::Weak;
use std::sync::Arc;

use quickjs_rust_bridge::{HostArgument, HostValue, Revival, TransferredClone};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use terminus::link::html::canvas::{
    CanvasError, Context2d, Gradient, Image, ImageDataRead, InvalidState, OffscreenCanvas, Path,
    Pattern,
};
use terminus::link::{NodeId, TextContext};
use tokio_util::sync::CancellationToken;

pub(crate) use self::members::install;
use crate::jobs::JsThreadHandle;
use crate::paint::canvas_pictures::{CanvasPresenter, Transferred};
use crate::view::WorkerFonts;

/// One object a realm's JavaScript names by id.
pub(crate) enum Entry {
    Canvas(CanvasEntry),
    Path(Path),
    Gradient(Gradient),
    Pattern(Pattern),
    /// An `ImageBitmap`'s pixels.
    Bitmap(Image),
}

/// An `OffscreenCanvas`: its size, its bitmap with the context state, made
/// at the first call that needs it, and the placeholder `<canvas>` it
/// commits to, if it came from one.
pub(crate) struct CanvasEntry {
    width: u32,
    height: u32,
    /// Boxed, so an entry of any kind is small to move.
    canvas: Option<Box<OffscreenCanvas>>,
    /// Listed in [`CanvasTable::drawn`] since the last
    /// [`CanvasTable::settle_drawn`].
    drawn: bool,
    /// What the bitmap is committed through, for a canvas made by
    /// `transferControlToOffscreen()`.
    presenter: Option<CanvasPresenter>,
}

// The table's entries move between realms, and so between threads.
const _: () = {
    const fn send<T: Send>() {}
    send::<Entry>();
    send::<Transfer>();
};

/// The entries a [`Transfer`] carries, behind a trait object whose `Send`
/// is stated in its type. A type holding a message is then proven `Send`
/// without the compiler walking an entry's GPU types, which nest deeper than
/// the compiler's default recursion limit lets it go.
trait Moved: Send {
    fn into_entries(self: Box<Self>) -> Vec<Entry>;

    fn len(&self) -> usize;
}

impl Moved for Vec<Entry> {
    fn into_entries(self: Box<Self>) -> Vec<Entry> {
        *self
    }

    fn len(&self) -> usize {
        Vec::len(self)
    }
}

/// What one `postMessage` posts: a value as the host boundary described
/// it, or a message written with a non-empty transfer list.
#[derive(Debug)]
pub(crate) enum Posted {
    Value(HostValue),
    Transfer(Transfer),
}

/// A message written with a transfer list, as it travels to the realm it is
/// posted to: the serializer's stream, one slot per list entry, and the
/// entry of each listed `OffscreenCanvas` or `ImageBitmap`, taken out of the
/// sending realm's table, in list order. Dropped undelivered — a worker
/// that ended, a realm that is gone — it drops what it carries.
pub(crate) struct Transfer {
    data: TransferredClone,
    /// One per slot of `data`.
    moved: Box<dyn Moved>,
}

impl std::fmt::Debug for Transfer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transfer")
            .field("slots", &self.moved.len())
            .finish_non_exhaustive()
    }
}

impl Transfer {
    /// Lends this message into `call` as the argument it reads as in the
    /// realm of `table`. The objects it carries wait in `table` for that
    /// realm's transfer reviver, which `bobcat:canvas` sets and which files
    /// each one in the table as the read reaches its slot (`canvasAdopt`).
    /// Whatever was not filed when `call` returns — the realm has no
    /// reviver, the reviver threw, the realm was not called — is dropped
    /// then.
    pub(crate) fn deliver<R>(
        self,
        table: &CanvasTable,
        call: impl FnOnce(HostArgument<'_>) -> R,
    ) -> R {
        let Self { data, moved } = self;
        let moved = moved.into_entries();
        #[expect(clippy::cast_precision_loss, reason = "a slot index is small")]
        let slots: SmallVec<[HostArgument<'_>; 4]> = (0..moved.len())
            .map(|slot| HostArgument::Number(slot as f64))
            .collect();
        let revival = Revival::new(data.as_bytes(), &slots)
            .expect("a transfer carries one entry per slot of its stream");
        let waiting = table.arrived.replace(moved.into_iter().map(Some).collect());
        let answer = call(HostArgument::Revive(&revival));
        // Not filed: dropped here, after the call.
        drop(table.arrived.replace(waiting));
        answer
    }
}

impl CanvasEntry {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            canvas: None,
            drawn: false,
            presenter: None,
        }
    }

    /// The `OffscreenCanvas` a placeholder's transfer answers. Its bitmap is
    /// made now, so a size set before any draw reaches it and is committed.
    fn transferred(transferred: Transferred) -> Self {
        let Transferred {
            width,
            height,
            presenter,
        } = transferred;
        Self {
            canvas: Some(Box::new(OffscreenCanvas::new(width, height))),
            presenter: Some(presenter),
            ..Self::new(width, height)
        }
    }

    fn canvas(&mut self) -> &mut OffscreenCanvas {
        let (width, height) = (self.width, self.height);
        self.canvas
            .get_or_insert_with(|| Box::new(OffscreenCanvas::new(width, height)))
    }

    /// Setting `width` or `height`: the bitmap and the context state reset.
    fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        if let Some(canvas) = &mut self.canvas {
            canvas.set_size(width, height);
        }
    }
}

/// What a realm's canvases have of a document.
#[derive(Clone)]
pub(crate) enum CanvasHost {
    /// None: a worker realm's. Text shapes with the view's host fonts and
    /// the platform's, in a context the realm makes at its first text call,
    /// as a worker on the web does not see the document's fonts.
    Worker(Arc<WorkerFonts>),
    /// The view's document: the MTS realm's and its `ShadowRealm`
    /// children's. Text shapes with the document's text context,
    /// `@font-face` faces included, and a `<canvas>` element's control is
    /// transferred from it (`canvasTransferControl`).
    Document(Weak<dyn CanvasDocument>),
}

/// The document a realm's canvases shape text with and transfer
/// placeholders from.
pub(crate) trait CanvasDocument {
    /// Runs `shape` with the document's text context; `false`, without
    /// running it, while there is no document or it is borrowed.
    fn with_text_context(&self, shape: &mut dyn FnMut(&mut TextContext)) -> bool;

    /// HTML's `transferControlToOffscreen()` on the element `node`
    /// (`crate::paint::canvas_pictures::transfer_control_to_offscreen`).
    ///
    /// # Errors
    ///
    /// The outer error, a message the member throws, for a node that is not
    /// a live element or a document that is not there; the inner one for an
    /// element that is not a canvas in context mode none.
    fn transfer_control_to_offscreen(
        &self,
        node: NodeId,
    ) -> Result<Result<Transferred, InvalidState>, String>;
}

/// What a read of a canvas's pixels waits with: natively nothing, the read
/// being done when it is issued; on wasm32 the realm's engine thread, parked
/// until the Render Worker answers or the realm's token is cancelled.
#[derive(Clone)]
pub(crate) struct ReadWait {
    #[cfg(target_arch = "wasm32")]
    token: CancellationToken,
    #[cfg(target_arch = "wasm32")]
    thread: JsThreadHandle,
}

impl ReadWait {
    /// Waits on `thread`, ending when `token` is cancelled.
    pub(crate) fn new(token: &CancellationToken, thread: &JsThreadHandle) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                token: token.clone(),
                thread: thread.clone(),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (token, thread);
            Self {}
        }
    }

    /// The pixels of `read`: `InvalidState` when the GPU did not answer, or
    /// on wasm32 the realm ended first.
    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(clippy::unused_self, reason = "on wasm32 the wait is the realm's")
    )]
    fn wait(&self, read: ImageDataRead) -> Result<Vec<u8>, CanvasError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            read.wait()
        }
        // The job parks as a synchronous `require` does: the thread's tasks
        // go on running, and the realm's end is the biased first arm.
        #[cfg(target_arch = "wasm32")]
        {
            let token = self.token.clone();
            self.thread.wait(async move {
                tokio::select! {
                    biased;
                    () = token.cancelled() => Err(CanvasError::InvalidState),
                    pixels = read.wait_async() => pixels,
                }
            })
        }
    }
}

enum Fonts {
    Worker(Arc<WorkerFonts>, RefCell<Option<Box<TextContext>>>),
    Document(Weak<dyn CanvasDocument>),
}

/// Every canvas object one realm's JavaScript holds, by id.
pub(crate) struct CanvasTable {
    entries: RefCell<FxHashMap<u64, Entry>>,
    next: Cell<u64>,
    /// The canvases drawn since the last [`Self::settle_drawn`].
    drawn: RefCell<Vec<u64>>,
    /// The entries a message being delivered carries, by slot, until the
    /// realm's reviver files them ([`Transfer::deliver`]); empty otherwise.
    arrived: RefCell<Vec<Option<Entry>>>,
    fonts: Fonts,
    reads: ReadWait,
    /// How many canvases [`Self::settle_drawn`] has flushed or committed,
    /// for a test.
    #[cfg(test)]
    pub(crate) settled: Cell<usize>,
}

impl CanvasTable {
    pub(crate) fn new(host: &CanvasHost, reads: ReadWait) -> Self {
        Self {
            entries: RefCell::default(),
            next: Cell::new(1),
            drawn: RefCell::default(),
            arrived: RefCell::default(),
            fonts: match host {
                CanvasHost::Worker(fonts) => Fonts::Worker(Arc::clone(fonts), RefCell::new(None)),
                CanvasHost::Document(document) => Fonts::Document(Weak::clone(document)),
            },
            reads,
            #[cfg(test)]
            settled: Cell::new(0),
        }
    }

    /// What runs after every entry into the realm, at the end of the
    /// entry: each canvas drawn during it replays what it recorded, and one
    /// bound to a placeholder whose bitmap changed is committed to it
    /// (HTML's update of the placeholder, in "update the rendering"). One
    /// test of an empty list for an entry that drew nothing.
    pub(crate) fn settle_drawn(&self) {
        let mut drawn = self.drawn.borrow_mut();
        if drawn.is_empty() {
            return;
        }
        // No JavaScript runs below, so neither borrow can be met again.
        let mut entries = self.entries.borrow_mut();
        for id in drawn.drain(..) {
            if let Some(Entry::Canvas(entry)) = entries.get_mut(&id) {
                entry.drawn = false;
                if let Some(canvas) = &mut entry.canvas {
                    match &entry.presenter {
                        Some(presenter) if canvas.has_changes() => presenter.commit(canvas),
                        _ => canvas.flush(),
                    }
                }
                #[cfg(test)]
                self.settled.set(self.settled.get() + 1);
            }
        }
    }

    /// Files `entry` under a new id.
    fn insert(&self, entry: Entry) -> u64 {
        let id = self.next.get();
        // 2^53 ids are not handed out in a realm's life, so every id is an
        // exact JavaScript number and none is reused.
        self.next.set(id + 1);
        self.entries.borrow_mut().insert(id, entry);
        id
    }

    /// Takes the entry under `id` out of the table.
    pub(crate) fn remove(&self, id: u64) -> Option<Entry> {
        self.entries.borrow_mut().remove(&id)
    }

    fn entries(&self) -> RefMut<'_, FxHashMap<u64, Entry>> {
        self.entries.borrow_mut()
    }

    /// The 2D context of the canvas under `id`, for a call that reads or
    /// sets context state.
    fn context<R>(&self, id: u64, call: impl FnOnce(&mut Context2d<'_>) -> R) -> Result<R, String> {
        match self.entries().get_mut(&id) {
            Some(Entry::Canvas(entry)) => Ok(call(&mut entry.canvas().context_2d())),
            _ => Err(no_such("canvas", id)),
        }
    }

    /// The canvas under `id`, for a call that may change the bitmap, which
    /// lists the canvas as drawn during this entry.
    fn draw_canvas<R>(
        &self,
        id: u64,
        call: impl FnOnce(&mut OffscreenCanvas) -> R,
    ) -> Result<R, String> {
        let mut entries = self.entries();
        let Some(Entry::Canvas(entry)) = entries.get_mut(&id) else {
            return Err(no_such("canvas", id));
        };
        self.note_drawn(id, entry);
        Ok(call(entry.canvas()))
    }

    /// [`Self::context`] for a call that may change the bitmap, which lists
    /// the canvas as drawn during this entry.
    fn draw<R>(&self, id: u64, call: impl FnOnce(&mut Context2d<'_>) -> R) -> Result<R, String> {
        self.draw_canvas(id, |canvas| call(&mut canvas.context_2d()))
    }

    /// Lists the canvas `entry`, under `id`, as drawn during this entry.
    fn note_drawn(&self, id: u64, entry: &mut CanvasEntry) {
        if !entry.drawn {
            entry.drawn = true;
            self.drawn.borrow_mut().push(id);
        }
    }

    /// The pixels of the canvas or bitmap under `id`, for `drawImage`,
    /// `createPattern` and `createImageBitmap`.
    fn image(&self, id: u64) -> Result<Image, String> {
        match self.entries().get_mut(&id) {
            Some(Entry::Canvas(entry)) => Ok(entry.canvas().image()),
            Some(Entry::Bitmap(image)) => Ok(image.clone()),
            _ => Err(no_such("canvas or image bitmap", id)),
        }
    }

    /// Runs `shape` with the text context this realm's canvas text is
    /// shaped in; `None` when the realm's document is not there to lend its
    /// own.
    fn with_fonts<R>(&self, shape: impl FnOnce(&mut TextContext) -> R) -> Option<R> {
        match &self.fonts {
            Fonts::Worker(fonts, context) => {
                let mut context = context.borrow_mut();
                let context = context.get_or_insert_with(|| Box::new(fonts.text_context()));
                Some(shape(context))
            }
            Fonts::Document(document) => {
                let document = document.upgrade()?;
                let mut shape = Some(shape);
                let mut shaped = None;
                document.with_text_context(&mut |context| {
                    if let Some(shape) = shape.take() {
                        shaped = Some(shape(context));
                    }
                });
                shaped
            }
        }
    }

    /// How many entries of each kind the table holds — canvases, paths,
    /// gradients, patterns, bitmaps — for a test.
    #[cfg(test)]
    pub(crate) fn census(&self) -> [usize; 5] {
        let mut census = [0; 5];
        for entry in self.entries.borrow().values() {
            census[match entry {
                Entry::Canvas(_) => 0,
                Entry::Path(_) => 1,
                Entry::Gradient(_) => 2,
                Entry::Pattern(_) => 3,
                Entry::Bitmap(_) => 4,
            }] += 1;
        }
        census
    }

    /// Whether any canvas still holds draws it has not replayed, for a
    /// test.
    #[cfg(test)]
    pub(crate) fn has_pending_draws(&self) -> bool {
        self.entries.borrow().values().any(|entry| {
            matches!(entry, Entry::Canvas(CanvasEntry { canvas: Some(canvas), .. })
                if canvas.has_pending_draws())
        })
    }
}

fn no_such(kind: &str, id: u64) -> String {
    format!("bobcat-internal:host: no {kind} has id {id}")
}

/// The names of the members [`install`] registers, for a test listing what
/// a realm's `bobcat-internal:host` exports.
#[cfg(test)]
pub(crate) const MEMBERS: [&str; 66] = [
    "markTransferable",
    "setTransferReviver",
    "canvasAdopt",
    "canvasCreate",
    "canvasDrop",
    "canvasSetSize",
    "canvasPath",
    "canvasPathFromSvg",
    "canvasAddPath",
    "canvasSave",
    "canvasRestore",
    "canvasReset",
    "canvasScale",
    "canvasRotate",
    "canvasTranslate",
    "canvasTransform",
    "canvasSetTransform",
    "canvasGetTransform",
    "canvasResetTransform",
    "canvasGetNumber",
    "canvasSetNumber",
    "canvasGetKeyword",
    "canvasSetKeyword",
    "canvasGetSmoothing",
    "canvasSetSmoothing",
    "canvasSetColor",
    "canvasSetPaint",
    "canvasLinearGradient",
    "canvasRadialGradient",
    "canvasConicGradient",
    "canvasAddColorStop",
    "canvasCreatePattern",
    "canvasPatternSetTransform",
    "canvasClearRect",
    "canvasFillRect",
    "canvasStrokeRect",
    "canvasBeginPath",
    "canvasClosePath",
    "canvasMoveTo",
    "canvasLineTo",
    "canvasQuadraticCurveTo",
    "canvasBezierCurveTo",
    "canvasArc",
    "canvasArcTo",
    "canvasEllipse",
    "canvasRect",
    "canvasRoundRect",
    "canvasFill",
    "canvasStroke",
    "canvasClip",
    "canvasIsPointInPath",
    "canvasIsPointInStroke",
    "canvasSetLineDash",
    "canvasGetLineDash",
    "canvasSetFont",
    "canvasFillText",
    "canvasStrokeText",
    "canvasMeasureText",
    "canvasDrawImage",
    "canvasGetImageData",
    "canvasPutImageData",
    "canvasTransferToBitmap",
    "canvasBitmapFrom",
    "canvasBitmapFromPixels",
    "canvasBitmapWidth",
    "canvasBitmapHeight",
];

/// The member [`install`] registers beside [`MEMBERS`] in a realm with a
/// document, for a test.
#[cfg(test)]
pub(crate) const DOCUMENT_MEMBER: &str = "canvasTransferControl";

/// `members` and the canvas members, sorted as a module namespace lists its
/// exports and joined with commas, for a test.
#[cfg(test)]
pub(crate) fn host_members_with_canvas(members: &[&str]) -> String {
    let mut all: Vec<&str> = members.iter().chain(&MEMBERS).copied().collect();
    all.sort_unstable();
    all.join(",")
}
