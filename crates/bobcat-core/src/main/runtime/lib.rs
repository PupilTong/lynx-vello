//! The Lynx main-thread runtime over its owned `QuickJS` realm.
//!
//! # Who owns the document
//!
//! The realm does, and it says so: the boot module's first statement is
//! `export const document = new Document();`, and that constructor is what
//! builds the document — out of the [`DocumentIngredients`] the view task
//! staged before this realm was opened. No JavaScript in this realm runs
//! ahead of that statement, so every host member that follows has a document
//! to work on.
//!
//! The exported binding holds it for the realm's life. Nothing in the realm
//! releases it: there is no release member, no registry, and no answer a tree
//! member gives without a document. Because `bobcat-internal:host` resolves
//! from any module in the realm a card can reach `createDocument` too, and
//! the one refusal left in this area is what it gets: the ingredients are
//! spent, so a second construction fails that card's boot.
//!
//! Release is the view's task ending. Dropping the `LynxView` closes the
//! command channel, the task returns, and [`MainThreadRuntime`]'s fields drop
//! in declaration order — every field that holds a handle of this realm first,
//! which together are what frees it, and with it the host functions it held and
//! their clones of this realm's [`DocumentSlot`]; then the runtime's own `slot`
//! handle, which is when the `LynxDocument` drops. JavaScript goes first, then
//! the Rust object it named.

use std::cell::{RefCell, RefMut};
use std::fmt::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;

use dom::StylePool;
use quickjs_rust_bridge::{HostArgument, HostValue};
use smallvec::SmallVec;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::clock::ClockInstant;
use crate::esm::{
    BTS_MODULE_SPECIFIER, CONTEXT_MODULE_SOURCE, CONTEXT_MODULE_SPECIFIER,
    EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE, HOST_MODULE_SPECIFIER, TIMER_MODULE_SOURCE,
    TIMER_MODULE_SPECIFIER,
};
use crate::link::{ViewNotice, ViewOutbox};
use crate::main::tree::{LynxDocument, PageConfig, apply_attribute_style, new_document};
use crate::resource::StyleSheetSource;
use crate::script::ScriptError;
use crate::timers::{TimerState, install_timer_members, run_due_timers};
use crate::view::Viewport;

const BOOT_MODULE_SPECIFIER: &str = "bobcat:boot";
const ELEMENT_MODULE_SPECIFIER: &str = "bobcat:element";
const RUNTIME_MODULE_SPECIFIER: &str = "bobcat:runtime";
const EVENT_DISPATCH_EXPORT: &str = "__BobcatDispatchEvent";

/// Declarations one `__SetInlineStyles` record carries without touching the
/// heap. Compiled `ReactLynx` records are a handful of properties.
const INLINE_DECLARATIONS: usize = 16;

const ELEMENT_PAPI_SOURCE: &str = crate::esm::runtime_source!("element-papi");
const RUNTIME_MODULE_SOURCE: &str = crate::esm::runtime_source!("main-thread-runtime");

mod style_sheets;

const ENTRY_PREAMBLE: &str = r#"import {
  __Card__,
  lynx,
  console,
  SystemInfo,
  __globalProps,
  NativeModules,
  _AddEventListener,
  _ReportError,
  _SetSourceMapRelease,
  __OnLifecycleEvent,
  __LoadLepusChunk,
  __LoadStyleSheet,
  __AdoptStyleSheet,
} from "bobcat:runtime";
import {
  __CreatePage,
  __CreateElement,
  __CreateWrapperElement,
  __CreateText,
  __CreateImage,
  __CreateView,
  __CreateScrollView,
  __CreateRawText,
  __CreateList,
  __AppendElement,
  __InsertElementBefore,
  __RemoveElement,
  __ReplaceElement,
  __ReplaceElements,
  __SwapElement,
  __SetClasses,
  __SetID,
  __GetID,
  __GetTag,
  __GetChildren,
  __GetAttributeByName,
  __GetAttributeNames,
  __GetElementUniqueID,
  __SetDataset,
  __GetDataset,
  __AddDataset,
  __SetInlineStyles,
  __AddInlineStyle,
  __SetCSSId,
  __SetAttribute,
  __UpdateListCallbacks,
  __AddEvent,
  __GetEvent,
  __GetEvents,
  __SetEvents,
  __AddEventListener,
  __RemoveEventListener,
  __StopPropagation,
  __StopImmediatePropagation,
  __GetPageElement,
  __QuerySelector,
  __QuerySelectorAll,
  __FlushElementTree,
} from "bobcat:element";
//# allFunctionsCalledOnLoad
"#;

pub(crate) fn entry_module_source(source: &str) -> String {
    let mut module = String::with_capacity(ENTRY_PREAMBLE.len() + source.len());
    module.push_str(ENTRY_PREAMBLE);
    module.push_str(source);
    module
}

/// Why constructing or running the engine-owned main-thread runtime failed.
#[derive(Debug)]
pub(crate) struct MainThreadError {
    context: &'static str,
    source: ScriptError,
}

impl MainThreadError {
    fn from_engine(context: &'static str, source: ScriptError) -> Self {
        Self { context, source }
    }

    pub(crate) fn into_script_error(mut self) -> ScriptError {
        self.source.message = Arc::from(format!("{}: {}", self.context, self.source.message));
        self.source
    }
}

impl fmt::Display for MainThreadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for MainThreadError {}

/// How many removals go by between collections.
///
/// A removal is where a subtree's handles start dying — `ReactLynx` unmounts
/// with `__RemoveElement` and then deletes the snapshot's element list, which
/// drops the last reference to the detached root's handle and, through the
/// child sets under it, to every handle in the subtree — and nothing frees
/// the elements until those handles are finalized. A handle reachable only
/// from a registration it captured is a cycle, which only a collection
/// resolves. `QuickJS` collects on its own only at allocation pressure (its
/// threshold is 1.5× the live size after each collection), so a card that
/// detaches steadily while allocating little keeps dead subtrees for a long
/// time: measured, a 301-node subtree outlived 30 batches at 5 elements of
/// churn per batch, and an idle card never freed it. Counting removals is the
/// cheapest signal that correlates with that garbage; the count is a policy
/// knob, not a measurement.
const REMOVALS_PER_COLLECTION: u32 = 32;

/// Everything a document is built out of, staged by the view task before the
/// realm opens.
///
/// Nothing on the Rust side creates the document any more: the boot module
/// does, by constructing a `Document`, and the host member behind that
/// constructor builds one out of this. The view task's boot phase therefore
/// fetches sources without a document to mount them on, and stages them here
/// instead.
pub(crate) struct DocumentIngredients {
    /// The metrics the document is created at. A `Resize` that arrives
    /// before the document does updates this rather than a document.
    pub(crate) viewport: Viewport,
    pub(crate) config: PageConfig,
    /// The fonts and the default family, already validated against a context
    /// of their own — see
    /// [`adopt_text_context`](dom::Document::adopt_text_context). `None` is a
    /// view that named neither, which leaves the document's own lazy context
    /// alone.
    pub(crate) text_context: Option<dom::TextContext>,
    /// Every author sheet this view listed, fetched and in cascade order.
    pub(crate) sheets: Vec<StyleSheetSource>,
    pub(crate) style_pool: Option<Rc<StylePool>>,
    /// Image reports that arrived while there was no document to apply them
    /// to, replayed in order once there is one.
    pub(crate) pending_image_events: Vec<dom::ImageEvent>,
}

#[cfg(test)]
impl DocumentIngredients {
    /// The ingredients of a view that lists no sheets, fonts or pool — what
    /// a test stages when the document itself is not what it is about.
    pub(crate) fn for_test(viewport: Viewport, config: PageConfig) -> Self {
        Self {
            viewport,
            config,
            text_context: None,
            sheets: Vec::new(),
            style_pool: None,
            pending_image_events: Vec::new(),
        }
    }
}

/// Everything one realm is opened with, in one value.
///
/// One struct because all of it shares one lifetime: it is handed over exactly
/// once, as the realm opens, and nothing ever updates it.
/// [`LynxView::update_data`](crate::LynxView::update_data),
/// [`update_global_props`](crate::LynxView::update_global_props) and
/// [`reload`](crate::LynxView::reload) reach the realm through
/// `ToMain::PageUpdate` instead, and never touch any of this. The four strings
/// below become one-shot host members the realm alone reads; `source` and
/// `url` are what the runtime evaluates; and `background_entry` is spliced
/// into the BTS Worker's boot script by `WorkerFactory::install`.
#[derive(Default)]
pub(crate) struct RealmStartup {
    /// The entry module's text and the resolved URL it is named by, which the
    /// runtime evaluates once this realm is furnished.
    pub(crate) source: String,
    pub(crate) url: String,
    /// The BTS entry `bobcat:bts` imports, if the view named one.
    pub(crate) background_entry: Option<String>,
    /// The host's processor name, page data and global props, as the strings
    /// it passed in. `bobcat:runtime` parses the data and props as JSON and
    /// uses the processor name unchanged.
    pub(crate) initial_processor: String,
    pub(crate) init_data: Option<String>,
    pub(crate) global_props: Option<String>,
    /// The embedder's modules as one `<utf16Length>:<text>` record, two fields
    /// per module: its name, then its method names joined with commas. Empty
    /// for a view built with none. The realm reads it into the
    /// `{name: methods}` object it sends the BTS Worker.
    pub(crate) native_modules: String,
}

/// The realm's document and the ingredients it is built out of, plus the
/// publish seam its commits leave through.
///
/// Filling it is the realm's doing: `createDocument` builds the document out
/// of the staged ingredients when the boot module constructs its `Document`.
/// Nothing empties it again — the slot drops with the view's task, after the
/// realm that named the document has been freed.
struct DocumentSlot {
    /// What a `createDocument` builds from, taken by the first one that runs.
    ingredients: Option<DocumentIngredients>,
    document: Option<LynxDocument>,
    /// Removals since the last collection; see [`REMOVALS_PER_COLLECTION`].
    removals: u32,
    /// Where committed frames leave for the painting side.
    outbox: ViewOutbox,
}

/// What every caller of [`DocumentSlot::document_mut`] relies on, stated at
/// the one place that could observe it failing.
const DOCUMENT_EXISTS: &str = "the boot module creates the document before any card runs";

impl DocumentSlot {
    /// The realm's document.
    ///
    /// Panicking is what a missing document deserves here rather than an
    /// error every member would have to carry: no JavaScript runs in this
    /// realm before the boot module's first statement creates the document,
    /// and nothing empties the slot until the realm itself is freed.
    fn document_mut(&mut self) -> &mut LynxDocument {
        self.document.as_mut().expect(DOCUMENT_EXISTS)
    }

    /// Builds the realm's one document out of the staged ingredients.
    ///
    /// The ingredients are spent by the first call, which is the whole of the
    /// refusal a second one gets: a construction that fails rejects the boot
    /// module's `new Document()`, which fails the boot and ends the view, so
    /// nothing asks again.
    ///
    /// Every phase is caught, because a panic that crosses the bridge is
    /// erased into "the host function panicked" and this is the one host
    /// member that runs the whole document pipeline — the UA cascade, the
    /// author sheets, the early image reports — behind a single call.
    fn create_document(&mut self) -> Result<(), String> {
        let Some(ingredients) = self.ingredients.take() else {
            return Err("the realm already created its document".to_owned());
        };
        let DocumentIngredients {
            viewport,
            config,
            text_context,
            sheets,
            style_pool,
            pending_image_events,
        } = ingredients;
        let mut document = construction_phase("building the page", || {
            let mut document = new_document(viewport, config);
            if let Some(pool) = style_pool {
                document.set_style_pool(pool);
            }
            if let Some(context) = text_context {
                document.adopt_text_context(context);
            }
            document
        })?;
        construction_phase("mounting the author stylesheets", || {
            for sheet in sheets {
                match sheet {
                    StyleSheetSource::Preparsed(sheet) => {
                        crate::style::add_preparsed_style_sheet(&mut document, &sheet);
                    }
                    StyleSheetSource::Text(css) => {
                        crate::style::add_style_sheet_text(&mut document, &css);
                    }
                }
            }
        })?;
        construction_phase("replaying the image reports that arrived first", || {
            document.apply_image_events(&pending_image_events);
        })?;
        self.document = Some(document);
        Ok(())
    }

    /// Runs the whole pipeline and publishes the committed frame — the
    /// native half of `__FlushElementTree`, and the only place frames leave
    /// this thread.
    fn flush(&mut self) {
        let frame = self.document_mut().commit();
        self.outbox.publish_frame(frame);
        // The walk that just ran is the one place that knows which image
        // sources this frame needs; ask the painter to name them. Empty on
        // every commit that met no new image, which is almost all of them.
        let wanted = self.document_mut().take_wanted_images();
        if !wanted.is_empty() {
            self.outbox.notify(ViewNotice::RequestImages(wanted));
        }
    }

    /// Commits and publishes only when something is stale — the epilogue of
    /// every entry into the realm, which is what makes "we do not guarantee
    /// the tree is not flushed outside `__FlushElementTree`" true.
    fn commit_if_dirty(&mut self) {
        if self.document_mut().needs_render() {
            self.flush();
        }
    }

    /// The `@font-face` rules the mounted sheets declared, once each.
    ///
    /// Empty before `createDocument`: the staged sheets are not in a cascade
    /// yet, and the first entry after they are mounted asks again.
    fn take_font_face_requests(&mut self) -> Vec<dom::FontFaceRequest> {
        self.document
            .as_mut()
            .map(LynxDocument::take_font_face_requests)
            .unwrap_or_default()
    }

    /// Notes that a subtree left the tree.
    fn note_removal(&mut self) {
        self.removals = self.removals.saturating_add(1);
    }

    /// Whether enough removals have accumulated for a collection, resetting
    /// the count when they have.
    fn take_collection_due(&mut self) -> bool {
        if self.removals < REMOVALS_PER_COLLECTION {
            return false;
        }
        self.removals = 0;
        true
    }
}

/// Runs one phase of document construction, naming it if it panics.
///
/// Without this the realm is told "the host function panicked", which is the
/// bridge's answer for every host member and says nothing about which of the
/// document's several pipelines gave way.
fn construction_phase<T>(phase: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work)).map_err(|payload| {
        format!(
            "creating the document panicked while {phase}: {}",
            crate::threads::panic_message(payload.as_ref())
        )
    })
}

/// The private main-thread runtime used by the engine pipeline.
///
/// **The field order is the release, and it must stay in this order.** Fields
/// drop in declaration order, and two rules fix that order.
///
/// Everything that holds an `Rc` of this realm's JavaScript context is declared
/// before `slot`, so the realm is freed — and with it every host function and
/// its own clone of that `Rc` — before the `LynxDocument` those functions could
/// name. JavaScript first, then the Rust object it named.
///
/// `workers` follows the realm handles: its channels belong to this MTS
/// runtime and close when it is released. JS host functions reference it
/// weakly, so queued finalizers cannot extend the channels' lifetime.
pub(crate) struct MainThreadRuntime {
    engine: ScriptEngine,
    /// This realm's side of the workers it created, shared with the three
    /// host functions that drive them, and where a worker failure is reported
    /// from.
    workers: Rc<super::workers::WorkerOwner>,
    slot: Rc<RefCell<DocumentSlot>>,
    timers: Rc<TimerState>,
}

impl fmt::Debug for MainThreadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl MainThreadRuntime {
    /// Opens one view's realm, furnishes it, and installs the `Worker`
    /// bindings — handing back the one channel everything this view's workers
    /// say arrives on.
    ///
    /// A realm has its `Worker` members from the moment it exists: there is no
    /// state in which it is missing them, and so no order between furnishing
    /// the realm and installing them for a caller to get wrong. Which worker
    /// this realm creates is still the realm's own business — the boot module
    /// constructs the built-in background context, during the entry evaluation
    /// this call does not make.
    ///
    /// It takes the strings it installs out of the startup, leaving the entry
    /// for the caller to evaluate.
    pub(crate) fn new(
        js_runtime: &mut ScriptRuntime,
        ingredients: DocumentIngredients,
        outbox: ViewOutbox,
        workers: &super::workers::WorkerFactory,
        startup: &mut RealmStartup,
    ) -> Result<
        (
            Self,
            tokio::sync::mpsc::UnboundedReceiver<crate::background::WorkerEvent>,
        ),
        MainThreadError,
    > {
        let mut engine = js_runtime
            .create_realm()
            .map_err(|error| MainThreadError::from_engine("creating the script realm", error))?;
        let timers = Rc::new(TimerState::new());
        let frame_outbox = outbox.clone();
        crate::script_frames::install(&mut engine, js_runtime, move |pending| {
            frame_outbox.notify(crate::link::ViewNotice::ScriptFrameDemand {
                worker: None,
                pending,
            });
        })
        .map_err(|error| MainThreadError::from_engine("installing animation frames", error))?;
        engine.enable_module_loading();
        let slot = install_bobcat(
            &mut engine,
            js_runtime,
            ingredients,
            outbox.clone(),
            &timers,
        )?;
        style_sheets::install_styles(&mut engine, js_runtime, &slot, &outbox)?;
        install_startup_strings(&mut engine, js_runtime, startup)?;
        let (workers, incoming) = workers
            .install(
                &mut engine,
                js_runtime,
                outbox,
                &startup.url,
                startup.background_entry.take(),
            )
            .map_err(|error| MainThreadError::from_engine("installing Worker", error))?;
        Ok((
            Self {
                engine,
                workers,
                slot,
                timers,
            },
            incoming,
        ))
    }

    /// How many of this realm's workers are still running, which is how many
    /// `Terminate`s releasing it would send.
    #[cfg(test)]
    pub(crate) fn live_workers(&self) -> usize {
        self.workers.live_workers()
    }

    pub(crate) fn dispatch_worker_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        key: crate::background::WorkerKey,
        payload: crate::background::WorkerPayload,
    ) -> Result<(), MainThreadError> {
        use crate::background::WorkerPayload;
        let failed = matches!(payload, WorkerPayload::Failed(_));
        // A worker that closed itself, or whose script or realm failed, has
        // ended: this is where the realm learns it, and so where the right to
        // tell it to stop stops being worth keeping.
        if matches!(payload, WorkerPayload::Closed | WorkerPayload::Failed(_)) {
            self.workers.forget(key);
        }
        // `(key, kind, data, filename, lineno, colno)`: `data` is the value
        // the worker posted — primitive or structured clone — for a message,
        // and the error's own message for a failure.
        let (kind, data, location) = match payload {
            WorkerPayload::Message(data) => ("message", data, None),
            WorkerPayload::Closed => ("closed", HostValue::Undefined, None),
            WorkerPayload::Errored(error) | WorkerPayload::Failed(error) => {
                let kind = if failed { "failed" } else { "error" };
                let data = HostValue::String(error.message.to_string());
                let location = error.location.clone();
                self.workers.report_failure(error);
                (kind, data, location)
            }
        };
        let location = location.as_ref();
        let key = key.get().to_string();
        let called = self
            .engine
            .call_module_export(
                js_runtime,
                super::workers::MODULE,
                "__BobcatDispatchWorkerEvent",
                &[
                    HostArgument::String(&key),
                    HostArgument::String(kind),
                    data.as_argument(),
                    HostArgument::String(location.and_then(|l| l.source.as_deref()).unwrap_or("")),
                    HostArgument::Number(f64::from(location.and_then(|l| l.line).unwrap_or(0))),
                    HostArgument::Number(f64::from(location.and_then(|l| l.column).unwrap_or(0))),
                ],
            )
            .map_err(|error| MainThreadError::from_engine("delivering a worker event", error));
        let finished = self.finish_batch(js_runtime, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    /// Commits and publishes when anything is stale. Called by the page's
    /// epilogue, after every entry into the realm.
    pub(crate) fn commit_if_dirty(&mut self) {
        self.slot.borrow_mut().commit_if_dirty();
    }

    pub(crate) fn apply_page_update(
        &mut self,
        js: &mut ScriptRuntime,
        update: &crate::link::PageUpdate,
    ) -> Result<(), MainThreadError> {
        use crate::link::PageUpdate;

        let (export, arguments): (&str, &[HostArgument<'_>]) = match update {
            PageUpdate::Reload {
                data,
                processor_name,
            } => (
                "__BobcatReload",
                &[
                    HostArgument::String(data),
                    HostArgument::String(processor_name),
                ],
            ),
            PageUpdate::Data {
                data,
                processor_name,
                reset,
            } => (
                "__BobcatUpdateData",
                &[
                    HostArgument::String(data),
                    HostArgument::String(processor_name),
                    HostArgument::Boolean(*reset),
                ],
            ),
            PageUpdate::GlobalProps(data) => {
                ("__BobcatUpdateGlobalProps", &[HostArgument::String(data)])
            }
            PageUpdate::GlobalEvent { name, arguments } => (
                "__BobcatSendGlobalEvent",
                &[HostArgument::String(name), HostArgument::String(arguments)],
            ),
        };
        let called = self
            .engine
            .call_module_export(js, RUNTIME_MODULE_SPECIFIER, export, arguments)
            .map_err(|error| MainThreadError::from_engine("updating page data", error));
        let finished = self.finish_batch(js, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    /// Advances the animation timeline to the painting side's clock
    /// reading. Whether anything changed is the next commit's business.
    pub(crate) fn begin_frame(&mut self, now: f64) {
        let _ = self
            .slot
            .borrow_mut()
            .document_mut()
            .advance_animations(now);
    }

    pub(crate) fn vsync(
        &mut self,
        js: &mut ScriptRuntime,
        milliseconds: f64,
    ) -> Result<(), MainThreadError> {
        self.engine
            .call_module_export(
                js,
                RUNTIME_MODULE_SPECIFIER,
                "__BobcatBeginFrame",
                &[HostArgument::Number(milliseconds)],
            )
            .map(|_| ())
            .map_err(|error| MainThreadError::from_engine("delivering animation callbacks", error))
    }

    /// Writes the painting side's scroll offsets into the document and
    /// repaints: the commit this entry's epilogue makes bakes windows
    /// re-centered on them. This is the only way a user scroll reaches the
    /// document — between refills the offsets live on the painting side
    /// alone.
    pub(crate) fn refill_scroll_windows(&mut self, offsets: &[(dom::NodeId, dom::Vector2D<f32>)]) {
        let mut slot = self.slot.borrow_mut();
        let document = slot.document_mut();
        for (node, offset) in offsets {
            document.scroll_to(*node, *offset);
        }
        document.note_scroll_windows_stale();
    }

    /// Applies new device metrics.
    ///
    /// The document is where they belong once it exists; before that the
    /// view task holds them in the ingredients instead, so the document is
    /// created at the size the painter last named.
    pub(crate) fn apply_resize(&mut self, width: f32, height: f32, device_pixel_ratio: f32) {
        let mut slot = self.slot.borrow_mut();
        let document = slot.document_mut();
        let viewport = document.viewport_size();
        if viewport.width.to_bits() != width.to_bits()
            || viewport.height.to_bits() != height.to_bits()
        {
            document.set_viewport(width, height);
        }
        if document.device_pixel_ratio().to_bits() != device_pixel_ratio.to_bits() {
            document.set_device_pixel_ratio(device_pixel_ratio);
        }
    }

    pub(crate) fn apply_image_events(&mut self, events: &[dom::ImageEvent]) {
        self.slot
            .borrow_mut()
            .document_mut()
            .apply_image_events(events);
    }

    /// Runs `probe` against the realm's document — the observation seam for
    /// everything outside this thread.
    pub(crate) fn with_document<T>(&mut self, probe: impl FnOnce(&mut LynxDocument) -> T) -> T {
        let mut slot = self.slot.borrow_mut();
        probe(slot.document_mut())
    }

    /// Delivers one routed event the painting side decided: the type and
    /// the target crossed as plain data; the propagation path is computed
    /// here, where the document is, and the dispatch over it is the realm's.
    ///
    /// A target freed since the decision formed resolves to nothing rather
    /// than a path — a `NodeId` names one node for the life of the document,
    /// so the check is one lookup and can never hit a stranger.
    ///
    /// The path crosses as the standard's event path — the bubble steps, in
    /// target-first, root-last order, each with its shadow-retargeted target
    /// — encoded as two comma-joined decimal id strings, which is how
    /// `childElementIds` carries a list already: the boundary takes
    /// primitives and structured clones only, a clone can be minted by the
    /// realm alone, and a decimal id cannot contain the separator. One call
    /// carries the whole dispatch, and what the realm then runs over it —
    /// the two passes, the `global-bindEvent` pass after them, whether any
    /// listener exists at all — is the realm's business, because every
    /// registration lives there.
    ///
    /// The document is released before the call, which is what lets a
    /// listener mutate the tree.
    ///
    /// Returns whether the realm published the export, which is all the host
    /// can know: nothing here says whether anything ran.
    pub(crate) fn dispatch_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        detail_json: &str,
    ) -> Result<bool, MainThreadError> {
        let steps = {
            let mut slot = self.slot.borrow_mut();
            let document = slot.document_mut();
            if document.get(target).is_none() {
                return Ok(false);
            }
            document.event_steps(target, true, true)
        };
        let mut nodes = String::new();
        let mut targets = String::new();
        for step in steps.steps().iter().filter(|step| !step.capture) {
            if !nodes.is_empty() {
                nodes.push(',');
                targets.push(',');
            }
            write!(nodes, "{}", packed_node_id(step.node)).expect("writing to a String");
            write!(targets, "{}", packed_node_id(step.target)).expect("writing to a String");
        }

        let called = self
            .engine
            .call_module_export(
                js_runtime,
                ELEMENT_MODULE_SPECIFIER,
                EVENT_DISPATCH_EXPORT,
                &[
                    HostArgument::String(&nodes),
                    HostArgument::String(&targets),
                    HostArgument::String(name),
                    HostArgument::String(detail_json),
                ],
            )
            .map_err(|error| MainThreadError::from_engine("delivering an event", error))?;
        // Listeners remove elements too; the count they ran up is settled
        // here, at the end of the dispatch.
        self.finish_batch(js_runtime, true)?;
        Ok(called)
    }

    /// When the earliest armed timer comes due, if one is armed.
    pub(crate) fn next_timer_deadline(&mut self) -> Option<ClockInstant> {
        self.timers.next_deadline()
    }

    /// Runs every timer due now, in the order the standard fires them.
    ///
    /// Returns whatever their callbacks threw. A timer is its own task, so
    /// one that throws neither stops the ones behind it nor ends the realm —
    /// the same standing an event listener that throws already has.
    pub(crate) fn run_due_timers(&mut self, js_runtime: &mut ScriptRuntime) -> Vec<ScriptError> {
        let Some(mut failures) = run_due_timers(&mut self.engine, js_runtime, &self.timers) else {
            return Vec::new();
        };
        // Callbacks remove elements like any other realm entry point; the
        // count they ran up is settled here, at the end of the batch.
        if let Err(error) = self.finish_batch(js_runtime, true) {
            failures.push(error.into_script_error());
        }
        failures
    }

    pub(crate) fn run_main_thread_script(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        let entry_source = entry_module_source(source);
        self.engine
            .register_module_source(source_name, source_name, &entry_source)
            .map_err(|error| {
                MainThreadError::from_engine("registering the MTS entry module", error)
            })?;
        let entry_specifier = serde_json::to_string(source_name)
            .expect("serializing a Rust string as a JavaScript string cannot fail");
        let (viewport, enable_js_data_processor) = {
            let slot = self.slot.borrow();
            let ingredients = slot
                .ingredients
                .as_ref()
                .expect("boot creates the document");
            (
                ingredients.viewport,
                ingredients.config.enable_js_data_processor,
            )
        };
        let Viewport {
            width,
            height,
            device_pixel_ratio,
        } = viewport;
        let boot = format!(
            r#"import {{ lynx, __BobcatConnectBackground, __BobcatInitializeMTS, __BobcatProcessInitData, __BobcatRenderPage, __BobcatInitEntry }} from "{RUNTIME_MODULE_SPECIFIER}";
import {{ Document, __FlushElementTree }} from "{ELEMENT_MODULE_SPECIFIER}";
// Imported for its effect: it installs the timer globals, and a static
// import runs before the entry this module then loads.
import "{TIMER_MODULE_SPECIFIER}";

// The realm's document, created by this module's first statement and held by
// this exported binding for the realm's life. Nothing in the realm releases
// it: it goes when the realm does.
export const document = new Document();

__BobcatInitEntry({entry_specifier});
__BobcatInitializeMTS({{
  enableJSDataProcessor: {enable_js_data_processor},
  systemInfo: {{pixelRatio: {device_pixel_ratio}, pixelWidth: {width} * {device_pixel_ratio}, pixelHeight: {height} * {device_pixel_ratio}}},
}});
// React's entry clears lynx.__initData during initialization. The host's
// first-screen argument belongs to boot, independently of that mutable slot.
let data = lynx.__initData;

await import({entry_specifier});
const {{ Worker }} = await import("bobcat-internal");
data = __BobcatProcessInitData(data);
__BobcatConnectBackground(new Worker("{BTS_MODULE_SPECIFIER}", {{ name: "lynx-bg" }}), data);

// Queue the flush after jobs already scheduled by the lifecycle hooks.
__BobcatRenderPage(data);
await Promise.resolve().then(() => __FlushElementTree());
"#
        );
        self.evaluate_module(
            js_runtime,
            &boot,
            BOOT_MODULE_SPECIFIER,
            "booting the MTS entry",
        )
    }

    /// The next module an import in this realm is waiting for, if any.
    pub(crate) fn take_module_request(&mut self) -> Option<String> {
        self.engine.take_module_request()
    }

    /// The `@font-face` rules the sheets mounted so far declared and this
    /// realm has not reported before. Empty until a sheet arrives.
    pub(crate) fn take_font_face_requests(&mut self) -> Vec<dom::FontFaceRequest> {
        self.slot.borrow_mut().take_font_face_requests()
    }

    /// Files one loaded `@font-face` source under its declared family.
    pub(crate) fn register_font_face(&mut self, family: &str, data: dom::FontBlob) {
        self.slot
            .borrow_mut()
            .document_mut()
            .register_font_face(family, data);
    }

    pub(crate) fn main_module_finished(&mut self) -> Result<bool, MainThreadError> {
        self.engine
            .module_finished()
            .map_err(|error| MainThreadError::from_engine("booting the MTS entry", error))
    }

    /// Run the MTS module's asynchronous disposal through the ordinary ESM
    /// evaluator. JavaScript owns the BTS request, acknowledgement and stop.
    pub(crate) fn begin_dispose(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Result<(), MainThreadError> {
        self.engine
            .start_module(
                js_runtime,
                "import { __BobcatDispose } from 'bobcat:runtime'; await __BobcatDispose();",
                "bobcat:dispose",
            )
            .map_err(|error| MainThreadError::from_engine("disposing the MTS realm", error))
    }

    pub(crate) fn disposal_finished(&mut self) -> Result<bool, MainThreadError> {
        self.engine
            .module_finished()
            .map_err(|error| MainThreadError::from_engine("disposing the MTS realm", error))
    }

    pub(crate) fn complete_module(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        name: &str,
        source: Result<crate::resource::LoadedSource, crate::LynxViewError>,
    ) -> Result<(), MainThreadError> {
        use crate::resource::LoadedSource;
        let loaded = match source {
            Ok(LoadedSource::Entry { source, url }) => Ok((url, source)),
            Ok(LoadedSource::StyleSheet(_)) => {
                Err("a module request returned a stylesheet".to_owned())
            }
            Ok(LoadedSource::Font(_)) => Err("a module request returned a font".to_owned()),
            Err(error) => Err(format!("module '{name}': {error}").replace('\0', "\u{fffd}")),
        };
        self.engine
            .complete_module(
                js_runtime,
                name,
                loaded
                    .as_ref()
                    .map(|(url, source)| (url.as_str(), source.as_str()))
                    .map_err(String::as_str),
            )
            .map_err(|error| MainThreadError::from_engine("loading an imported module", error))
    }

    fn collect_garbage(&mut self, js_runtime: &mut ScriptRuntime) -> Result<(), MainThreadError> {
        self.slot.borrow_mut().removals = 0;
        self.engine
            .collect_garbage(js_runtime)
            .map_err(|error| MainThreadError::from_engine("collecting garbage", error))
    }

    /// Collects if enough held subtrees were removed since the last
    /// collection. Runs after every evaluation that succeeded; a failed one
    /// keeps its count for the next.
    fn finish_batch(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        succeeded: bool,
    ) -> Result<(), MainThreadError> {
        let due = succeeded && self.slot.borrow_mut().take_collection_due();
        if due {
            self.collect_garbage(js_runtime)
        } else {
            Ok(())
        }
    }

    pub(crate) fn evaluate_module(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        name: &str,
        phase: &'static str,
    ) -> Result<(), MainThreadError> {
        let result = self
            .engine
            .start_module(js_runtime, source, name)
            .map_err(|error| MainThreadError::from_engine(phase, error));
        let finished = self.finish_batch(js_runtime, result.is_ok());
        result.and(finished)
    }
}

/// Registers the source modules every realm on one runtime shares.
///
/// Their specifiers are fixed, so registering them per realm would refuse the
/// second realm on a runtime — a runtime holds one source per name and
/// compiles it into a module per realm, which is exactly what views share.
pub(crate) fn install_shared_modules(
    js_runtime: &mut ScriptRuntime,
) -> Result<(), MainThreadError> {
    js_runtime
        .register_module_source(super::workers::MODULE, super::workers::SOURCE)
        .map_err(|error| MainThreadError::from_engine("registering Worker", error))?;
    js_runtime
        .register_module_source(EVENT_TARGET_MODULE_SPECIFIER, EVENT_TARGET_SOURCE)
        .map_err(|error| {
            MainThreadError::from_engine("registering the EventTarget module", error)
        })?;
    js_runtime
        .register_module_source(CONTEXT_MODULE_SPECIFIER, CONTEXT_MODULE_SOURCE)
        .map_err(|error| MainThreadError::from_engine("registering the Context module", error))?;
    js_runtime
        .register_module_source(RUNTIME_MODULE_SPECIFIER, RUNTIME_MODULE_SOURCE)
        .map_err(|error| {
            MainThreadError::from_engine("registering the Bobcat runtime module", error)
        })?;
    js_runtime
        .register_module_source(ELEMENT_MODULE_SPECIFIER, ELEMENT_PAPI_SOURCE)
        .map_err(|error| {
            MainThreadError::from_engine("registering the Element PAPI module", error)
        })?;
    js_runtime
        .register_module_source(TIMER_MODULE_SPECIFIER, TIMER_MODULE_SOURCE)
        .map_err(|error| MainThreadError::from_engine("registering the timer module", error))
}

fn install_bobcat(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    ingredients: DocumentIngredients,
    outbox: ViewOutbox,
    timers: &Rc<TimerState>,
) -> Result<Rc<RefCell<DocumentSlot>>, MainThreadError> {
    for (name, is_error) in [("reportScriptError", true), ("logScriptMessage", false)] {
        let reporting = outbox.clone();
        install(engine, js_runtime, name, 2, move |arguments| {
            let level = string_argument(name, arguments, 0)?.to_owned();
            let message = string_argument(name, arguments, 1)?.to_owned();
            reporting.engine_event(if is_error {
                crate::EngineEvent::ScriptReported { level, message }
            } else {
                crate::EngineEvent::ConsoleMessage { level, message }
            });
            Ok(HostValue::Undefined)
        })?;
    }
    let events = outbox.clone();
    let handle = Rc::new(RefCell::new(DocumentSlot {
        ingredients: Some(ingredients),
        document: None,
        removals: 0,
        outbox,
    }));

    install_host_module(engine, js_runtime, &handle)?;
    install_event_members(engine, js_runtime, &events)?;
    install_timer_members(engine, js_runtime, timers)
        .map_err(|error| MainThreadError::from_engine("installing the timer members", error))?;

    Ok(handle)
}

fn install(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    name: &str,
    arity: u8,
    callback: impl FnMut(&[HostValue]) -> Result<HostValue, String> + 'static,
) -> Result<(), MainThreadError> {
    engine
        .register_host_module_function(
            js_runtime,
            HOST_MODULE_SPECIFIER,
            name,
            arity,
            Box::new(callback),
        )
        .map_err(|error| MainThreadError::from_engine("installing the host module", error))
}

/// Installs native host-module exports that parse their arguments, borrow the
/// slot, and run against the realm's document. Each `$parser` is one of the
/// argument helpers below, applied at the argument's position; `NAME` is the
/// diagnostic prefix every helper and validator stitches into its error.
///
/// The document is taken unconditionally: see [`DocumentSlot::document_mut`].
macro_rules! tree_members {
    ($engine:ident, $js_runtime:ident, $handle:ident; $(
        fn $name:ident($($arg:ident: $parser:ident),*) |$document:ident| $body:block
    )*) => {$({
        const NAME: &str = concat!("bobcat-internal:host.", stringify!($name));
        let tree = Rc::clone($handle);
        let arity = 0u8 $(+ { let _ = stringify!($arg); 1u8 })*;
        install($engine, $js_runtime, stringify!($name), arity, move |arguments| {
            #[allow(unused_mut, reason = "zero-argument members never advance it")]
            let mut index = 0usize;
            $(
                let $arg = $parser(NAME, arguments, index)?;
                index += 1;
            )*
            let _ = (arguments, index);
            let mut handle = borrow_slot(NAME, &tree)?;
            let $document = handle.document_mut();
            $body
        })?;
    })*};
}

fn install_host_module(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, js_runtime, handle;
        fn createPage() |document| {
            Ok(node_id_value(document.document_element().id()))
        }
        fn createElement(tag: string_argument) |document| {
            Ok(node_id_value(document.create_element(tag, ())))
        }
        fn parentNode(node: node_id_argument) |document| {
            let parent = document.get(node).and_then(dom::Node::parent_id);
            Ok(parent.map_or(HostValue::Null, node_id_value))
        }
        fn insertBefore(
            parent: node_id_argument,
            child: node_id_argument,
            reference: optional_node_id_argument
        ) |document| {
            validate_insert(document, NAME, parent, child, reference)?;
            document.insert_before(parent, child, reference);
            Ok(HostValue::Undefined)
        }
        fn swapElement(a: node_id_argument, b: node_id_argument) |document| {
            validate_swap(document, NAME, a, b)?;
            document.swap_element(a, b);
            Ok(HostValue::Undefined)
        }
    }

    // The two removals are written out rather than generated, because each
    // also counts toward the next collection: a detached subtree is freed
    // only once the handles naming it are finalized.
    let tree = Rc::clone(handle);
    install(engine, js_runtime, "removeElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.removeElement";
        let child = node_id_argument(NAME, arguments, 0)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_removable(document, NAME, child)?;
        document.remove_element(child);
        handle.note_removal();
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(engine, js_runtime, "replaceElement", 2, move |arguments| {
        const NAME: &str = "bobcat-internal:host.replaceElement";
        let new_element = node_id_argument(NAME, arguments, 0)?;
        let old_element = node_id_argument(NAME, arguments, 1)?;
        let mut handle = borrow_slot(NAME, &tree)?;
        let document = handle.document_mut();
        validate_removable(document, NAME, old_element)?;
        validate_live_element(document, NAME, new_element)?;
        if let Some(parent) = document.get(old_element).and_then(dom::Node::parent_id) {
            validate_insert(document, NAME, parent, new_element, Some(old_element))?;
            document.insert_before(parent, new_element, Some(old_element));
            document.remove_element(old_element);
            handle.note_removal();
        }
        Ok(HostValue::Undefined)
    })?;

    install_document_members(engine, js_runtime, handle)?;
    install_attribute_members(engine, js_runtime, handle)?;

    let tree = Rc::clone(handle);
    // The realm's handle for `node` has been collected, and a handle is the
    // one thing that holds an element: the node is freed now. Only the node —
    // its element children are unlinked and go on as detached roots, each
    // held by the handle that names it. Host-owned text children are freed
    // with the node because no realm handle names them. Every
    // listener the realm had on it is gone too, since those lived on the
    // handle — which the realm settles itself, before it calls this, by
    // closing whatever event names that handle alone held open.
    install(engine, js_runtime, "dropElement", 1, move |arguments| {
        const NAME: &str = "bobcat-internal:host.dropElement";
        let node = node_id_argument(NAME, arguments, 0)?;
        let mut tree = borrow_slot(NAME, &tree)?;
        let document = tree.document_mut();
        validate_removable(document, NAME, node)?;
        // A connected element's handle is held by its parent's, up to the
        // permanent page handle, so a connected element can never be the
        // subject of a drop; if one is, the graph and the tree disagree and
        // the realm must hear about it before the element is gone.
        if document.is_connected(node) {
            return Err(format!(
                "{NAME} was given a connected element: the element ownership \
                 graph and the tree disagree"
            ));
        }
        document.drop_element(node);
        Ok(HostValue::Undefined)
    })?;

    let tree = Rc::clone(handle);
    install(
        engine,
        js_runtime,
        "flushElementTree",
        0,
        move |_arguments| {
            borrow_slot("bobcat-internal:host.flushElementTree", &tree)?.flush();
            Ok(HostValue::Undefined)
        },
    )?;

    Ok(())
}

/// Installs the one member that begins the document's life.
///
/// It is the realm's, not the host's: `bobcat:element`'s `Document`
/// constructor calls it, and it resolves from any module in the realm —
/// `bobcat-internal:host` is realm-wide, as it is for `dropElement` — so a
/// card can reach it and be refused. There is no member at the other end:
/// nothing in the realm releases the document, which goes when the realm
/// does.
fn install_document_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    let tree = Rc::clone(handle);
    install(engine, js_runtime, "createDocument", 0, move |_arguments| {
        // The page id is not this member's answer: `createPage` is still what
        // hands the realm the permanent root, and it now has a document to
        // read it from.
        borrow_slot("bobcat-internal:host.createDocument", &tree)?.create_document()?;
        Ok(HostValue::Undefined)
    })?;

    Ok(())
}

/// Installs `initData`, `globalProps`, `initialProcessor` and
/// `nativeModuleTable`, handing the realm the original strings. Missing
/// initial data or props become `undefined`.
///
/// Each hands its string over once and keeps nothing. `bobcat:runtime` parses
/// the initial data and props, uses the processor name as a plain string, and
/// reads the module table as the record the realm decodes. All answer before
/// `createDocument` has run.
///
/// The four are *taken* out of the startup rather than copied out of it: each
/// is handed over once and never read again, so the move says what the
/// lifetime is. What is left behind is the entry, which the caller is about to
/// evaluate.
fn install_startup_strings(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    startup: &mut RealmStartup,
) -> Result<(), MainThreadError> {
    for (name, mut value) in [
        ("initData", startup.init_data.take()),
        ("globalProps", startup.global_props.take()),
        (
            "initialProcessor",
            Some(std::mem::take(&mut startup.initial_processor)),
        ),
        (
            "nativeModuleTable",
            Some(std::mem::take(&mut startup.native_modules)),
        ),
    ] {
        install(engine, js_runtime, name, 0, move |_arguments| {
            Ok(value.take().map_or(HostValue::Undefined, HostValue::String))
        })?;
    }
    Ok(())
}

/// Installs the two members the realm's event registrations speak to.
///
/// Neither touches the document, and neither carries a node: the realm owns
/// every registration and the whole walk over an event path, so all the host
/// is told is the *name* set the painting side routes against — and only its
/// global edges, the first registration for a name anywhere and the removal
/// of its last. A name already open hears nothing, so the traffic is edges,
/// never registrations, and the `Arc` each one allocates is rare enough to
/// be free.
fn install_event_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    outbox: &ViewOutbox,
) -> Result<(), MainThreadError> {
    for (member, available) in [("listenerNameOpened", true), ("listenerNameClosed", false)] {
        let outbox = outbox.clone();
        install(engine, js_runtime, member, 1, move |arguments| {
            let name = string_argument(member, arguments, 0)?;
            outbox.listener_edge(Arc::from(name), available);
            Ok(HostValue::Undefined)
        })?;
    }

    Ok(())
}

/// Installs the DOM-attribute and inline-style portion of the host module.
///
/// `id`, `class`, and `style` deliberately travel through the same narrow
/// string boundary as every other attribute. [`LynxDocument`] owns their
/// specialized DOM/style invalidation paths. `setInlineStyles` is the one
/// wider primitive: a whole record in one crossing, because a record is a
/// whole-block replacement and building it from empty is what the setter
/// means. Nothing in the realm — the Element PAPI included — receives a
/// document handle.
fn install_attribute_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, js_runtime, handle;
        fn setAttribute(
            node: node_id_argument,
            name: string_argument,
            value: string_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            document.set_attribute(node, name, value);
            apply_attribute_style(document, node, name, Some(value));
            Ok(HostValue::Undefined)
        }
        // Deliberately name-based: this PAPI receives record keys, custom
        // properties have no numeric id, and Stylo's internal PropertyId is
        // not a stable script ABI. A future numeric-key `__AddInlineStyle`
        // can translate its bundle id in JavaScript/the decoder-owned layer
        // before reaching this one primitive.
        fn setInlineStyles(node: node_id_argument, record: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            let declarations = split_style_record(NAME, record)?;
            document.set_inline_style_declarations(node, declarations);
            Ok(HostValue::Undefined)
        }
        fn setInlineStyleProperty(
            node: node_id_argument,
            name: string_argument,
            value: string_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            document.set_inline_style_property(node, name, value);
            Ok(HostValue::Undefined)
        }
        fn supportsStyleProperty(name: string_argument) |document| {
            Ok(HostValue::Boolean(document.supports_style_property(name)))
        }
        fn queryElementIds(
            root: node_id_argument,
            selector: string_argument,
            first_only: flag_argument,
            include_root: flag_argument
        ) |document| {
            validate_live_element(document, NAME, root)?;
            // SelectorQuery is Lynx's inclusive query scope, so it asks for
            // the root to be considered; the Element PAPI's `__QuerySelector`
            // is `Element.querySelector`'s, which is root-exclusive, and asks
            // for it to be skipped. Matching the root itself still uses the
            // same standard selector engine as the cascade.
            let matches_root =
                include_root && document.matches(root, selector).map_err(|e| e.to_string())?;
            let mut ids = if matches_root { vec![root] } else { Vec::new() };
            if !first_only || ids.is_empty() {
                if first_only {
                    ids.extend(document.query_selector(root, selector).map_err(|e| e.to_string())?);
                } else {
                    ids.extend(document.query_selector_all(root, selector).map_err(|e| e.to_string())?);
                }
            }
            Ok(HostValue::String(ids.into_iter().map(|id| id.to_bits().to_string()).collect::<Vec<_>>().join(",")))
        }
        fn removeAttribute(node: node_id_argument, name: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            document.remove_attribute(node, name);
            apply_attribute_style(document, node, name, None);
            Ok(HostValue::Undefined)
        }
        fn getAttribute(node: node_id_argument, name: string_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let value = element.attribute(name).map(str::to_owned);
            Ok(value.map_or(HostValue::Null, HostValue::String))
        }
        fn tagName(node: node_id_argument) |document| {
            let tag = validate_live_element(document, NAME, node)?
                .tag_name()
                .ok_or_else(|| {
                    "bobcat-internal:host.tagName requires a live element tag".to_owned()
                })?;
            Ok(HostValue::String(tag.to_owned()))
        }
        fn attributeNames(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut record = String::new();
            for (name, _) in element.attributes() {
                write_record_field(&mut record, name);
            }
            Ok(HostValue::String(record))
        }
        fn childElementIds(node: node_id_argument) |document| {
            let element = validate_live_element(document, NAME, node)?;
            let mut ids = String::new();
            for child in element.children().filter(|child| child.is_element()) {
                if !ids.is_empty() {
                    ids.push(',');
                }
                write!(&mut ids, "{}", child.id().to_bits())
                    .expect("writing to a String cannot fail");
            }
            Ok(HostValue::String(ids))
        }
    }

    Ok(())
}

/// Splits a `__SetInlineStyles` record payload into its declarations.
///
/// The payload is a flat sequence of `<units>:<text>` fields, two per
/// declaration — the hyphenated property name, then the value. `<units>` is
/// the text's length in UTF-16 code units, which is exactly what JavaScript's
/// `String.prototype.length` reports, so the writing side needs no scan and
/// no escaping.
///
/// Length-prefixing rather than delimiting is the point: a declaration value
/// is arbitrary author text, and any separator this could have used — a
/// semicolon, a NUL, a private-use code point — is a character some value may
/// legitimately contain. A length says where the next field starts without
/// asking what is inside this one.
fn split_style_record<'a>(
    function: &str,
    payload: &'a str,
) -> Result<SmallVec<[(&'a str, &'a str); INLINE_DECLARATIONS]>, String> {
    let mut declarations = SmallVec::new();
    let mut rest = payload;
    while !rest.is_empty() {
        let (property, after_property) = take_record_field(function, rest)?;
        let (value, after_value) = take_record_field(function, after_property)?;
        declarations.push((property, value));
        rest = after_value;
    }
    Ok(declarations)
}

/// Reads one `<units>:<text>` field, returning it and what follows.
fn take_record_field<'a>(function: &str, rest: &'a str) -> Result<(&'a str, &'a str), String> {
    let malformed = || format!("{function} received a malformed style record");
    let separator = rest.find(':').ok_or_else(malformed)?;
    let units: usize = rest[..separator].parse().map_err(|_| malformed())?;
    let body = &rest[separator + 1..];

    let mut counted = 0usize;
    let mut end = 0usize;
    for (offset, character) in body.char_indices() {
        if counted == units {
            end = offset;
            break;
        }
        counted += character.len_utf16();
        end = offset + character.len_utf8();
    }
    // Short of the count means the payload was truncated; past it means the
    // count landed inside a surrogate pair. Neither can come from the writer.
    if counted != units {
        return Err(malformed());
    }
    Ok((&body[..end], &body[end..]))
}

/// Appends one `<units>:<text>` field, [`take_record_field`]'s inverse; the
/// count is in UTF-16 code units because `String.prototype.slice` consumes it.
pub(crate) fn write_record_field(record: &mut String, text: &str) {
    let units: usize = text.chars().map(char::len_utf16).sum();
    write!(record, "{units}:").expect("writing to a String cannot fail");
    record.push_str(text);
}

fn borrow_slot<'a>(
    function: &str,
    tree: &'a Rc<RefCell<DocumentSlot>>,
) -> Result<RefMut<'a, DocumentSlot>, String> {
    tree.try_borrow_mut()
        .map_err(|_| format!("{function} cannot re-enter the element tree"))
}

fn validate_live_element<'a>(
    document: &'a LynxDocument,
    function: &str,
    node: dom::NodeId,
) -> Result<&'a dom::Node<()>, String> {
    let node = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if node.is_element() {
        Ok(node)
    } else {
        Err(format!("{function} requires a live element id"))
    }
}

fn validate_removable(
    document: &LynxDocument,
    function: &str,
    node: dom::NodeId,
) -> Result<(), String> {
    let live = document
        .get(node)
        .ok_or_else(|| format!("{function} received a stale element id"))?;
    if live.is_document() || live.is_shadow_root() || node == document.document_element().id() {
        Err(format!(
            "{function} cannot remove the document, page root, or a shadow root"
        ))
    } else {
        Ok(())
    }
}

fn validate_insert(
    document: &LynxDocument,
    function: &str,
    parent: dom::NodeId,
    child: dom::NodeId,
    reference: Option<dom::NodeId>,
) -> Result<(), String> {
    validate_live_element(document, function, parent)?;
    validate_removable(document, function, child)?;
    if parent == child || document.is_ancestor(child, parent) {
        return Err(format!("{function} cannot create an element-tree cycle"));
    }
    if reference == Some(child) {
        return Err(format!(
            "{function} requires the reference and child to differ"
        ));
    }
    if let Some(reference) = reference {
        let reference_parent = document
            .get(reference)
            .ok_or_else(|| format!("{function} received a stale reference id"))?
            .parent_id();
        if reference_parent != Some(parent) {
            return Err(format!(
                "{function} requires the reference to be a child of the parent"
            ));
        }
    }
    Ok(())
}

fn validate_swap(
    document: &LynxDocument,
    function: &str,
    a: dom::NodeId,
    b: dom::NodeId,
) -> Result<(), String> {
    if a == b {
        return Err(format!("{function} requires distinct elements"));
    }
    validate_removable(document, function, a)?;
    validate_removable(document, function, b)?;
    if document.get(a).and_then(dom::Node::parent_id).is_none()
        || document.get(b).and_then(dom::Node::parent_id).is_none()
    {
        return Err(format!("{function} requires attached elements"));
    }
    if document.is_ancestor(a, b) || document.is_ancestor(b, a) {
        return Err(format!("{function} cannot swap an ancestor and descendant"));
    }
    Ok(())
}

fn argument(arguments: &[HostValue], index: usize) -> &HostValue {
    arguments.get(index).unwrap_or(&HostValue::Undefined)
}

/// The largest integer an `f64` represents exactly. A packed `NodeId` is built
/// to stay under it, so a handle survives the script boundary unchanged.
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// A `NodeId` crossing into script *is* the element's Lynx `unique_id`: the DOM
/// issues it, and `__GetElementUniqueID` hands back the same number the
/// creating PAPI returned.
///
/// The handle carries both the arena key and the generation that key was at, so
/// it crosses packed into one integer. The generation is what makes a stale
/// handle safe: script holds these for as long as it likes, and a number it
/// stashed can outlive the element it named (a released element is freed once
/// it is detached), so such an id must resolve to nothing rather than to
/// whatever took its place.
#[allow(
    clippy::cast_precision_loss,
    reason = "a packed handle is built to stay inside f64's exact-integer range"
)]
fn packed_node_id(node: dom::NodeId) -> f64 {
    node.to_bits() as f64
}

/// The same handle as a value a host callback returns, rather than as an
/// argument the runtime lends into a call.
fn node_id_value(node: dom::NodeId) -> HostValue {
    HostValue::Number(packed_node_id(node))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bounds and integer checks above make the value a representable handle"
)]
fn node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<dom::NodeId, String> {
    let HostValue::Number(value) = *argument(arguments, index) else {
        return Err(format!("{function} expects a number for argument {index}"));
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= MAX_EXACT_INTEGER {
        return Err(format!(
            "{function} expects a non-negative integer node id for argument {index}"
        ));
    }
    dom::NodeId::from_bits(value as u64)
        .ok_or_else(|| format!("{function} got a number that is no element id: {value}"))
}

fn optional_node_id_argument(
    function: &str,
    arguments: &[HostValue],
    index: usize,
) -> Result<Option<dom::NodeId>, String> {
    match *argument(arguments, index) {
        HostValue::Undefined | HostValue::Null => Ok(None),
        _ => node_id_argument(function, arguments, index).map(Some),
    }
}

/// A flag the realm spells as `0` or `1`, which is every boolean the host
/// module takes.
fn flag_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Number(0.0) => Ok(false),
        HostValue::Number(1.0) => Ok(true),
        _ => Err(format!("{function} expects 0 or 1 for argument {index}")),
    }
}

/// A timer id, which the realm only ever passes back after the host handed
/// it one.
fn string_argument<'a>(
    function: &str,
    arguments: &'a [HostValue],
    index: usize,
) -> Result<&'a str, String> {
    match argument(arguments, index) {
        HostValue::String(value) => Ok(value),
        HostValue::Undefined | HostValue::Null => Ok(""),
        _ => Err(format!("{function} expects a string for argument {index}")),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod web_text_replication;

#[cfg(test)]
mod worker_tests;
