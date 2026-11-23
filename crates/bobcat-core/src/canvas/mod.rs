//! The Canvas 2D API's host state: one [`CanvasTable`] per realm, holding
//! the Canvas 2D objects (`terminus::link::html::canvas`) — canvases, paths,
//! gradients, patterns and image bitmaps — that the realm's JavaScript names
//! by id through the canvas members of `bobcat-internal:host`.
//!
//! The table is installed with the core every realm has
//! ([`crate::realm::open_realm`], and each `ShadowRealm` child its own), so
//! every realm kind has the members. They are [`members::install`]'s; the
//! engine's built-in modules and an embedder's native-module ES module
//! (`lynx:<name>`, [`crate::NativeModule::esm`]) call them, nothing checks
//! the caller, and no realm has a canvas class or global.
//! `crates/bobcat-core/docs/canvas.md` is the topic.
//!
//! - **Ids** are handed out from 1 and never reused, so a stale id reaches nothing rather than
//!   another object's entry. `canvasDrop` removes an entry; nothing else does while the realm
//!   lives, and the table goes with the realm.
//! - **Entries** are owned `Send` values ([`Entry`]), so an entry can leave this table for another
//!   realm's: `canvasDetach` puts a canvas or image bitmap in the view's [`CanvasHandOver`] under a
//!   token, and `canvasAttach` files it in the table of the realm that names the token.
//! - **The glyph outline cache** a canvas replay uses is its thread's, shared by every canvas
//!   encoded there (`terminus`'s `render::canvas`). The table holds a [`ReplayUser`] while it holds
//!   a canvas, so the cache is dropped once no realm on the thread holds one: a group's view realms
//!   and their `ShadowRealm` children share `bobcat-main`, and its BTS and every `Worker` share
//!   `bobcat-workers`.
//! - **The per-entry step** is [`CanvasTable::settle_drawn`]: the owner's epilogue runs it after
//!   every entry. Each canvas drawn during the entry replays what it recorded, so a recording
//!   nothing reads stays one entry long, and one bound to a placeholder `<canvas>` that changed is
//!   committed to it.

mod members;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell, RefMut};
use std::rc::Weak;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use rustc_hash::FxHashMap;
use terminus::link::html::canvas::{
    CanvasError, Context2d, Gradient, Image, ImageDataRead, InvalidState, OffscreenCanvas, Path,
    Pattern, ReplayUser,
};
use terminus::link::{NodeId, TextContext};
use tokio_util::sync::CancellationToken;

pub(crate) use self::members::install;
use crate::jobs::JsThreadHandle;
use crate::paint::canvas_pictures::{CanvasPresenter, Transferred};

/// One object a realm's JavaScript names by id.
pub(crate) enum Entry {
    Canvas(CanvasEntry),
    Path(Path),
    Gradient(Gradient),
    Pattern(Pattern),
    /// An image bitmap's pixels.
    Bitmap(Image),
}

/// A canvas: its size, its bitmap with the context state, made at the first
/// call that needs it, and the placeholder `<canvas>` it commits to, if it
/// came from one.
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
};

/// An entry in a [`CanvasHandOver`], behind a trait object whose `Send` is
/// stated in its type: a type holding the store is then proven `Send`
/// without the compiler walking an entry's GPU types, which nest deeper than
/// its default recursion limit lets it go.
trait Moved: Send {
    fn into_entry(self: Box<Self>) -> Entry;
}

impl Moved for Entry {
    fn into_entry(self: Box<Self>) -> Entry {
        *self
    }
}

/// The canvas and image bitmap entries one view's realms hand to each
/// other, by token: `canvasDetach` stores an entry taken out of the calling
/// realm's table, and `canvasAttach` takes it out again into the calling
/// realm's table.
///
/// One per view: the view's `ViewOutbox` makes it, and every `HostOutbox`
/// the view hands out — the MTS realm's, its `ShadowRealm` children's, the
/// BTS's and each `Worker`'s — carries the same `Arc`. Tokens start at 1
/// and are never reused. What is never attached is dropped when the last of
/// those outboxes goes, which is when the view and its workers have ended.
#[derive(Default)]
pub(crate) struct CanvasHandOver {
    /// The last token handed out.
    last: AtomicU64,
    entries: Mutex<FxHashMap<u64, Box<dyn Moved>>>,
}

impl CanvasHandOver {
    fn lock(&self) -> std::sync::MutexGuard<'_, FxHashMap<u64, Box<dyn Moved>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `entry` under a new token, and answers the token.
    fn store(&self, entry: Box<dyn Moved>) -> u64 {
        let token = self.last.fetch_add(1, Ordering::Relaxed) + 1;
        self.lock().insert(token, entry);
        token
    }

    /// Takes the entry under `token` out of the store.
    fn take(&self, token: u64) -> Option<Entry> {
        self.lock().remove(&token).map(Moved::into_entry)
    }

    /// How many entries wait in the store, for a test.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
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

    /// The canvas a placeholder's transfer files. Its bitmap is
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
    /// None: a worker realm's. Text shapes with the platform's fonts, in a
    /// context the realm makes at its first text call, as a worker on the
    /// web does not see the document's fonts.
    Worker,
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
    System(RefCell<Option<Box<TextContext>>>),
    Document(Weak<dyn CanvasDocument>),
}

/// Every canvas object one realm's JavaScript holds, by id.
pub(crate) struct CanvasTable {
    entries: RefCell<FxHashMap<u64, Entry>>,
    next: Cell<u64>,
    /// The canvases drawn since the last [`Self::settle_drawn`].
    drawn: RefCell<Vec<u64>>,
    fonts: Fonts,
    reads: ReadWait,
    /// The view's store that `canvasDetach` and `canvasAttach` go through.
    hand_over: Arc<CanvasHandOver>,
    /// Held while `entries` holds a canvas: the thread's canvas replay
    /// state is kept for it.
    replay: RefCell<Option<ReplayUser>>,
    /// How many canvases [`Self::settle_drawn`] has flushed or committed,
    /// for a test.
    #[cfg(test)]
    pub(crate) settled: Cell<usize>,
}

impl CanvasTable {
    pub(crate) fn new(host: &CanvasHost, reads: ReadWait, hand_over: Arc<CanvasHandOver>) -> Self {
        Self {
            entries: RefCell::default(),
            next: Cell::new(1),
            drawn: RefCell::default(),
            fonts: match host {
                CanvasHost::Worker => Fonts::System(RefCell::new(None)),
                CanvasHost::Document(document) => Fonts::Document(Weak::clone(document)),
            },
            reads,
            hand_over,
            replay: RefCell::new(None),
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
        if matches!(entry, Entry::Canvas(_)) {
            self.replay.borrow_mut().get_or_insert_with(ReplayUser::new);
        }
        self.entries.borrow_mut().insert(id, entry);
        id
    }

    /// Takes the entry under `id` out of the table.
    pub(crate) fn remove(&self, id: u64) -> Option<Entry> {
        let mut entries = self.entries.borrow_mut();
        let entry = entries.remove(&id);
        if matches!(entry, Some(Entry::Canvas(_))) {
            self.canvas_removed(&entries);
        }
        entry
    }

    /// `canvasDetach(id)`: takes the canvas or image bitmap under `id` out
    /// of this table into the view's store, and answers its token. A canvas
    /// goes as it is — bitmap, context state, draws not yet replayed, its
    /// placeholder's presenter — and leaves the drawn list; the replay state
    /// is let go once the last canvas has left.
    fn detach(&self, id: u64) -> Result<u64, String> {
        let mut entries = self.entries();
        if !matches!(entries.get(&id), Some(Entry::Canvas(_) | Entry::Bitmap(_))) {
            return Err(no_such("canvas or image bitmap", id));
        }
        let Some(mut entry) = entries.remove(&id) else {
            return Err(no_such("canvas or image bitmap", id));
        };
        if let Entry::Canvas(canvas) = &mut entry {
            canvas.drawn = false;
            self.drawn.borrow_mut().retain(|drawn| *drawn != id);
            self.canvas_removed(&entries);
        }
        drop(entries);
        Ok(self.hand_over.store(Box::new(entry)))
    }

    /// `canvasAttach(token)`: files the entry the view's store holds under
    /// `token` in this table, and answers its new id, width and height;
    /// `None` when the store holds nothing under `token`. A canvas that
    /// arrives with draws not yet replayed, or with a bitmap changed since
    /// its last commit, is listed as drawn, so this realm's epilogue
    /// replays or commits it.
    fn attach(&self, token: u64) -> Option<[f64; 3]> {
        let entry = self.hand_over.take(token)?;
        let (size, owed) = match &entry {
            Entry::Canvas(canvas) => (
                [canvas.width, canvas.height],
                canvas
                    .canvas
                    .as_ref()
                    .is_some_and(|bitmap| bitmap.has_changes() || bitmap.has_pending_draws()),
            ),
            Entry::Bitmap(image) => ([image.width(), image.height()], false),
            Entry::Path(_) | Entry::Gradient(_) | Entry::Pattern(_) => ([0, 0], false),
        };
        let id = self.insert(entry);
        if owed && let Some(Entry::Canvas(canvas)) = self.entries().get_mut(&id) {
            self.note_drawn(id, canvas);
        }
        #[expect(clippy::cast_precision_loss, reason = "ids stay below 2^53")]
        Some([id as f64, f64::from(size[0]), f64::from(size[1])])
    }

    /// Lets go of the thread's canvas replay state once `entries`, which a
    /// canvas has just left, holds none; a bitmap or a pattern encodes no
    /// draw of its own.
    fn canvas_removed(&self, entries: &FxHashMap<u64, Entry>) {
        if !entries
            .values()
            .any(|entry| matches!(entry, Entry::Canvas(_)))
        {
            self.replay.borrow_mut().take();
        }
    }

    /// Whether the table keeps the thread's canvas replay state, for a
    /// test.
    #[cfg(test)]
    pub(crate) fn keeps_replay_state(&self) -> bool {
        self.replay.borrow().is_some()
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

    /// The pixels of the canvas or bitmap under `id`, for `canvasDrawImage`,
    /// `canvasCreatePattern` and `canvasBitmapFrom`.
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
            Fonts::System(context) => {
                let mut context = context.borrow_mut();
                let context = context.get_or_insert_with(|| Box::new(TextContext::new()));
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

/// The message of a lookup that found nothing of `kind` under `id`; the
/// member that looked prefixes its own name.
fn no_such(kind: &str, id: u64) -> String {
    format!("no {kind} has id {id}")
}

/// The names of the members [`install`] registers, for a test listing what
/// a realm's `bobcat-internal:host` exports.
#[cfg(test)]
pub(crate) const MEMBERS: [&str; 65] = [
    "canvasCreate",
    "canvasDrop",
    "canvasSetSize",
    "canvasDetach",
    "canvasAttach",
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
