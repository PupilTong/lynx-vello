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
use crate::view::WorkerFonts;

pub(crate) enum Entry {
    Canvas(CanvasEntry),
    Path(Path),
    Gradient(Gradient),
    Pattern(Pattern),
    Bitmap(Image),
}

pub(crate) struct CanvasEntry {
    width: u32,
    height: u32,
    canvas: Option<Box<OffscreenCanvas>>,
    drawn: bool,
    presenter: Option<CanvasPresenter>,
}

const _: () = {
    const fn send<T: Send>() {}
    send::<Entry>();
};

trait Moved: Send {
    fn into_entry(self: Box<Self>) -> Entry;
}

impl Moved for Entry {
    fn into_entry(self: Box<Self>) -> Entry {
        *self
    }
}

#[derive(Default)]
pub(crate) struct CanvasHandOver {
    last: AtomicU64,
    entries: Mutex<FxHashMap<u64, Box<dyn Moved>>>,
}

impl CanvasHandOver {
    fn lock(&self) -> std::sync::MutexGuard<'_, FxHashMap<u64, Box<dyn Moved>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn store(&self, entry: Box<dyn Moved>) -> u64 {
        let token = self.last.fetch_add(1, Ordering::Relaxed) + 1;
        self.lock().insert(token, entry);
        token
    }

    fn take(&self, token: u64) -> Option<Entry> {
        self.lock().remove(&token).map(Moved::into_entry)
    }

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

    fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        if let Some(canvas) = &mut self.canvas {
            canvas.set_size(width, height);
        }
    }
}

#[derive(Clone)]
pub(crate) enum CanvasHost {
    Worker(Arc<WorkerFonts>),
    Document(Weak<dyn CanvasDocument>),
}

pub(crate) trait CanvasDocument {
    fn with_text_context(&self, shape: &mut dyn FnMut(&mut TextContext)) -> bool;

    fn transfer_control_to_offscreen(
        &self,
        node: NodeId,
    ) -> Result<Result<Transferred, InvalidState>, String>;
}

#[derive(Clone)]
pub(crate) struct ReadWait {
    #[cfg(target_arch = "wasm32")]
    token: CancellationToken,
    #[cfg(target_arch = "wasm32")]
    thread: JsThreadHandle,
}

impl ReadWait {
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

    #[cfg_attr(
        not(target_arch = "wasm32"),
        expect(clippy::unused_self, reason = "on wasm32 the wait is the realm's")
    )]
    fn wait(&self, read: ImageDataRead) -> Result<Vec<u8>, CanvasError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            read.wait()
        }
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

pub(crate) struct CanvasTable {
    entries: RefCell<FxHashMap<u64, Entry>>,
    next: Cell<u64>,
    drawn: RefCell<Vec<u64>>,
    fonts: Fonts,
    reads: ReadWait,
    hand_over: Arc<CanvasHandOver>,
    replay: RefCell<Option<ReplayUser>>,
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
                CanvasHost::Worker(fonts) => Fonts::Worker(Arc::clone(fonts), RefCell::new(None)),
                CanvasHost::Document(document) => Fonts::Document(Weak::clone(document)),
            },
            reads,
            hand_over,
            replay: RefCell::new(None),
            #[cfg(test)]
            settled: Cell::new(0),
        }
    }

    pub(crate) fn settle_drawn(&self) {
        let mut drawn = self.drawn.borrow_mut();
        if drawn.is_empty() {
            return;
        }
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

    fn insert(&self, entry: Entry) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        if matches!(entry, Entry::Canvas(_)) {
            self.replay.borrow_mut().get_or_insert_with(ReplayUser::new);
        }
        self.entries.borrow_mut().insert(id, entry);
        id
    }

    pub(crate) fn remove(&self, id: u64) -> Option<Entry> {
        let mut entries = self.entries.borrow_mut();
        let entry = entries.remove(&id);
        if matches!(entry, Some(Entry::Canvas(_))) {
            self.canvas_removed(&entries);
        }
        entry
    }

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

    fn canvas_removed(&self, entries: &FxHashMap<u64, Entry>) {
        if !entries
            .values()
            .any(|entry| matches!(entry, Entry::Canvas(_)))
        {
            self.replay.borrow_mut().take();
        }
    }

    #[cfg(test)]
    pub(crate) fn keeps_replay_state(&self) -> bool {
        self.replay.borrow().is_some()
    }

    fn entries(&self) -> RefMut<'_, FxHashMap<u64, Entry>> {
        self.entries.borrow_mut()
    }

    fn context<R>(&self, id: u64, call: impl FnOnce(&mut Context2d<'_>) -> R) -> Result<R, String> {
        match self.entries().get_mut(&id) {
            Some(Entry::Canvas(entry)) => Ok(call(&mut entry.canvas().context_2d())),
            _ => Err(no_such("canvas", id)),
        }
    }

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

    fn draw<R>(&self, id: u64, call: impl FnOnce(&mut Context2d<'_>) -> R) -> Result<R, String> {
        self.draw_canvas(id, |canvas| call(&mut canvas.context_2d()))
    }

    fn note_drawn(&self, id: u64, entry: &mut CanvasEntry) {
        if !entry.drawn {
            entry.drawn = true;
            self.drawn.borrow_mut().push(id);
        }
    }

    fn image(&self, id: u64) -> Result<Image, String> {
        match self.entries().get_mut(&id) {
            Some(Entry::Canvas(entry)) => Ok(entry.canvas().image()),
            Some(Entry::Bitmap(image)) => Ok(image.clone()),
            _ => Err(no_such("canvas or image bitmap", id)),
        }
    }

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

    #[cfg(test)]
    pub(crate) fn has_pending_draws(&self) -> bool {
        self.entries.borrow().values().any(|entry| {
            matches!(entry, Entry::Canvas(CanvasEntry { canvas: Some(canvas), .. })
                if canvas.has_pending_draws())
        })
    }
}

fn no_such(kind: &str, id: u64) -> String {
    format!("no {kind} has id {id}")
}

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

#[cfg(test)]
pub(crate) const DOCUMENT_MEMBER: &str = "canvasTransferControl";

#[cfg(test)]
pub(crate) fn host_members_with_canvas(members: &[&str]) -> String {
    let mut all: Vec<&str> = members.iter().chain(&MEMBERS).copied().collect();
    all.sort_unstable();
    all.join(",")
}
