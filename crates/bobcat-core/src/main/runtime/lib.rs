//! The Lynx main-thread runtime over its owned `QuickJS` realm.
//!
//! # Who owns the document
//!
//! The realm does, and it says so: the boot module's first statement after its
//! configuration literal is `export const document = new Document(config);`,
//! and that constructor is what
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
use tokio::sync::watch;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use crate::clock::ClockInstant;
use crate::esm::{
    BTS_MODULE_SPECIFIER, ELEMENT_MODULE_SPECIFIER, HOST_MODULE_SPECIFIER,
    RUNTIME_MODULE_SPECIFIER, TIMER_MODULE_SPECIFIER, WORKER_CLASS_MODULE_SPECIFIER,
};
use crate::link::{InputEventPayload, ViewNotice, ViewOutbox};
use crate::main::tree::{ImageOutcomes, LynxDocument, PageConfig, new_document};
use crate::realm::{RealmCore, context_of, open_realm};
use crate::resource::LoadedSource;
use crate::script::ScriptError;
use crate::timers::run_due_timers;
use crate::view::{LynxViewError, ScreenMetrics, StartupSource, Viewport};

const BOOT_MODULE_SPECIFIER: &str = "bobcat:boot";
const EVENT_DISPATCH_EXPORT: &str = "__BobcatDispatchEvent";

/// What an element's own image source settling is called. Both are web-core's
/// names, which are the browser's (`XImage/ImageEvents.ts`), and both are
/// non-bubbling.
const LOAD_EVENT: &str = "load";
const ERROR_EVENT: &str = "error";

/// What one dispatch's `detail` is made of, as it crosses the boundary: a
/// discriminator and the numbers that kind spends. The object itself is built
/// in the realm, because its shape is JavaScript's.
///
/// One kind per detail *shape* rather than per event name. The realm reads the
/// discriminator alone — nothing there branches on which event is being
/// delivered — which is what keeps the one dispatch export general.
enum EventDetail<'a> {
    /// Every routed input event: the device position, the wheel delta an event
    /// may not have, and the four numbers per point the touch events carry.
    Input(&'a InputEventPayload),
    /// An `<image>`'s `load`: the bitmap's intrinsic size, in px.
    Size { width: f64, height: f64 },
    /// An `<image>`'s `error`: the realm's `{}`, and no numbers at all.
    Empty,
}

/// The discriminators [`EventDetail`] crosses as. Mirrored by the realm's own
/// `DETAIL_*` constants in `element-papi.ts`.
const DETAIL_INPUT: f64 = 0.0;
const DETAIL_SIZE: f64 = 1.0;
const DETAIL_EMPTY: f64 = 2.0;

impl EventDetail<'_> {
    const fn kind(&self) -> f64 {
        match self {
            Self::Input(_) => DETAIL_INPUT,
            Self::Size { .. } => DETAIL_SIZE,
            Self::Empty => DETAIL_EMPTY,
        }
    }

    /// Appends the numbers this kind spends, in the order the realm reads
    /// them.
    fn push_numbers<'a>(&'a self, arguments: &mut SmallVec<[HostArgument<'a>; 10]>) {
        match *self {
            Self::Input(payload) => {
                arguments.push(HostArgument::Number(f64::from(payload.position.x)));
                arguments.push(HostArgument::Number(f64::from(payload.position.y)));
                // `undefined` rather than a number for every event without a
                // wheel delta, which is what makes the two `detail` keys
                // absent there rather than `NaN`.
                for delta in [
                    payload.wheel.map(|delta| delta.x),
                    payload.wheel.map(|delta| delta.y),
                ] {
                    arguments.push(match delta {
                        Some(value) => HostArgument::Number(f64::from(value)),
                        None => HostArgument::Undefined,
                    });
                }
                for point in &payload.touches {
                    arguments.push(HostArgument::Number(f64::from(point.identifier)));
                    arguments.push(HostArgument::Number(f64::from(point.position.x)));
                    arguments.push(HostArgument::Number(f64::from(point.position.y)));
                    arguments.push(HostArgument::Number(f64::from(point.flags)));
                }
            }
            Self::Size { width, height } => {
                arguments.push(HostArgument::Number(width));
                arguments.push(HostArgument::Number(height));
            }
            Self::Empty => {}
        }
    }
}

/// Declarations one `__SetInlineStyles` record carries without touching the
/// heap. Compiled `ReactLynx` records are a handful of properties.
const INLINE_DECLARATIONS: usize = 16;

mod style_sheets;

/// What the MTS entry is given, which is [`crate::esm::MTS_CHUNK_PREAMBLE`] — the same
/// list a lazy container's `main-thread` body is compiled against. Built from
/// it rather than written twice, so the two lists cannot drift. web-core's
/// wrapper also carries a `//# allFunctionsCalledOnLoad` line, a V8
/// eager-compilation hint that `QuickJS`'s parser ignores, so it is not
/// reproduced here.
///
/// The preamble does not name the entry: [`MainThreadRuntime::complete_entry`]
/// hands the response URL to `__BobcatInitEntry` before the body runs, so the
/// statement is not compiled into every module this preamble is prepended to.
const ENTRY_PREAMBLE: &str = concat!(crate::esm::mts_chunk_preamble!(), "\n");

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

/// The Rust side of what a document is built out of: everything the realm is
/// not told and does not decide.
///
/// The boot module is what creates the document, by constructing a `Document`
/// over the page configuration written into it; the host member behind that
/// constructor takes the rest from here. The view's own resources therefore
/// never become JavaScript values — the realm names its metrics, its fonts and
/// its style pool nowhere.
pub(crate) struct DocumentIngredients {
    /// The metrics the document is created at, as the embedder named them to
    /// `create_lynx_view`. They are what it works at until a painter binds;
    /// a painter that bound first supersedes them at the construction.
    ///
    /// A view's metrics and nothing else. What `SystemInfo` reports is the
    /// screen, which the embedder names separately in
    /// [`ViewSources::screen`](crate::ViewSources::screen).
    pub(crate) viewport: Viewport,
    /// The page configuration the view was built with. The boot module is
    /// written with its four switches as literals, and hands them back to the
    /// `createDocument` that builds the document as four boolean arguments.
    /// What the document is actually built with is therefore the realm's
    /// copy, not this one.
    pub(crate) config: PageConfig,
    /// The fonts and the default family, already validated against a context
    /// of their own — see
    /// [`adopt_text_context`](dom::Document::adopt_text_context). `None` is a
    /// view that named neither, which leaves the document's own lazy context
    /// alone.
    pub(crate) text_context: Option<dom::TextContext>,
    pub(crate) style_pool: Option<Rc<StylePool>>,
}

/// A metrics watch that already holds `viewport`, so the realm built over it
/// is bound from the start.
///
/// The tests that build a realm in place play the painting side as well as
/// the view's, and an unbound `__FlushElementTree` would park on the binding
/// that side never makes. The sender is dropped with the call: a bound slot
/// only ever reads the value.
#[cfg(test)]
pub(crate) fn bound_metrics(viewport: Viewport) -> watch::Receiver<Option<Viewport>> {
    watch::channel(Some(viewport)).1
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
            style_pool: None,
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
/// below `background_entry` become one-shot host members the realm alone reads,
/// `entry` is written into the boot module's source, `background_entry` is
/// spliced into the BTS Worker's boot script by `WorkerFactory::install`, and
/// `sheets` go to the document slot, which the first `__FlushElementTree`
/// settles them out of. The entry's source is not here: its answer is a task
/// of the view's owner, which completes the module boot imports the entry as.
pub(crate) struct RealmStartup {
    /// The answers to the author stylesheet requests `create_lynx_view` made,
    /// in the order the view listed them, which is their cascade order. The
    /// first `__FlushElementTree` waits for each and mounts it before the
    /// document enters the style pipeline; nothing waits for them earlier.
    pub(crate) sheets: Vec<StartupSource>,
    /// The screen the realm's `SystemInfo` reports, as the embedder named it
    /// in [`ViewSources::screen`](crate::ViewSources::screen).
    pub(crate) screen: ScreenMetrics,
    /// The view's MTS entry, [`ViewSources::entry`](crate::ViewSources::entry)
    /// as `create_lynx_view` resolved it: always an absolute URL, in its
    /// WHATWG serialization. Boot imports the entry by this string.
    pub(crate) entry: String,
    /// The BTS entry `bobcat:bts` imports, if the view named one: always an
    /// absolute URL `create_lynx_view` resolved, like [`Self::entry`].
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

/// Hand-written rather than derived, because there is no default screen: every
/// embedder names one. What opens a realm from `RealmStartup::default()` is
/// this crate's own tests and benchmarks, and none of them reads `SystemInfo`.
impl Default for RealmStartup {
    fn default() -> Self {
        Self {
            sheets: Vec::new(),
            screen: ScreenMetrics {
                pixel_ratio: 1.0,
                pixel_width: 0.0,
                pixel_height: 0.0,
            },
            // No entry: the seams that boot a realm opened from this name one
            // themselves.
            entry: String::new(),
            background_entry: None,
            initial_processor: String::new(),
            init_data: None,
            global_props: None,
            native_modules: String::new(),
        }
    }
}

/// The realm's document and the ingredients it is built out of, plus the
/// publish seam its commits leave through.
///
/// Filling it is the realm's doing: `createDocument` builds the document out
/// of the configuration the boot module hands it and the Rust-side ingredients
/// below, and the first `__FlushElementTree` mounts the view's author sheets
/// on it, in listed order, before the document is styled for the first time.
/// Nothing empties it again — the slot drops with the view's task, after the
/// realm that named the document has been freed.
///
/// It is also where a view learns its painter's metrics, and so where the
/// binding is decided: the document is created at the create-time viewport
/// and works at it until a painter writes the watch, and a flush before that
/// holds its frame and parks.
struct DocumentSlot {
    /// What a `createDocument` builds from, taken by the first one that runs.
    ingredients: Option<DocumentIngredients>,
    /// The answers to this view's author stylesheet requests that have not
    /// been mounted yet, in the order the view listed them.
    ///
    /// Order of *use* rather than of completion: [`Self::flush`] mounts them
    /// one at a time and in this order, whatever order the fetcher answered
    /// them in, so the cascade order between listed sheets is the listed
    /// order. Non-empty is what keeps [`Self::commit_if_dirty`] from running
    /// the pipeline, so no frame is committed without them.
    sheets: Vec<StartupSource>,
    document: Option<LynxDocument>,
    /// The device metrics an attached painter names, `None` until one binds.
    ///
    /// Read at the start of [`Self::commit_if_dirty`] and [`Self::flush`],
    /// and polled directly by the wait an unbound flush parks on — never
    /// routed through a command, because no job runs while one is parked.
    metrics: watch::Receiver<Option<Viewport>>,
    /// Whether a painter has ever named its metrics for this view.
    ///
    /// Only the first binding is waited for: a painter that detaches leaves
    /// this set and the last metrics in place, so a later flush publishes
    /// rather than parking.
    bound: bool,
    /// The newest frame committed before the binding, which is not published
    /// yet.
    ///
    /// A frame is held rather than published because a painter composes at
    /// its own size: it has no way to tell that the frame it adopted was
    /// committed for the metrics it has just replaced. The binding is what
    /// releases it — as it is, if the metrics match, or recomputed if they do
    /// not — and a newer commit published once bound drops it, so it is
    /// never published after a frame that superseded it.
    held: Option<Arc<dom::CommittedFrame>>,
    /// The `load`s and `error`s the document's images owe, from both
    /// producers: the `image` component, which settles a `src` inside the
    /// `__SetAttribute` that wrote it, and [`MainThreadRuntime::apply_image_events`],
    /// which settles one from the painting side's report. Held here rather
    /// than inside the document because the component is the far end of it and
    /// reaches nothing else; the runtime drains it once per entry.
    image_outcomes: ImageOutcomes,
    /// Removals since the last collection; see [`REMOVALS_PER_COLLECTION`].
    removals: u32,
    /// Where committed frames leave for the painting side.
    outbox: ViewOutbox,
}

/// What every caller of [`DocumentSlot::document_mut`] relies on, stated at
/// the one place that could observe it failing.
const DOCUMENT_EXISTS: &str = "the boot module creates the document before any card runs";

impl DocumentSlot {
    /// The slot a realm opens with: no document, the inputs the one
    /// `createDocument` will build it from, and the seat it publishes
    /// through.
    fn new(
        ingredients: DocumentIngredients,
        sheets: Vec<StartupSource>,
        metrics: watch::Receiver<Option<Viewport>>,
        outbox: ViewOutbox,
    ) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self {
            ingredients: Some(ingredients),
            sheets,
            document: None,
            metrics,
            bound: false,
            held: None,
            image_outcomes: ImageOutcomes::default(),
            removals: 0,
            outbox,
        }))
    }

    /// The realm's document.
    ///
    /// Panicking is what a missing document deserves here rather than an
    /// error every member would have to carry: no JavaScript runs in this
    /// realm before the boot module's first statement creates the document,
    /// and nothing empties the slot until the realm itself is freed.
    fn document_mut(&mut self) -> &mut LynxDocument {
        self.document.as_mut().expect(DOCUMENT_EXISTS)
    }

    /// Builds the realm's one document out of the configuration the boot
    /// module hands it and the Rust-side ingredients the view was created
    /// with.
    ///
    /// The ingredients are spent by the first call, which is the whole of the
    /// refusal a second one gets: a construction that fails rejects the boot
    /// module's `new Document(config)`, which fails the boot and ends the view,
    /// so nothing asks again.
    ///
    /// It never waits. The view's author sheets are not mounted here: the
    /// first [`Self::flush`] mounts them, which is the first point at which
    /// the document is styled.
    ///
    /// The construction is caught, because a panic that crosses the bridge is
    /// erased into "the host function panicked" and this is the one host
    /// member that runs the UA cascade behind a single call.
    fn create_document(&mut self, config: PageConfig) -> Result<(), String> {
        let Some(ingredients) = self.ingredients.take() else {
            return Err("the realm already created its document".to_owned());
        };
        let DocumentIngredients {
            viewport,
            // The realm's copy is what the document is built with: the boot
            // module was written with this one and handed it back.
            config: _,
            text_context,
            style_pool,
        } = ingredients;
        // The create-time viewport is what the document works at until a
        // painter binds; a painter that bound before the boot module ran its
        // first statement has already named the real one.
        let viewport = match *self.metrics.borrow_and_update() {
            Some(metrics) => {
                self.bound = true;
                metrics
            }
            None => viewport,
        };
        let outcomes = self.image_outcomes.clone();
        let document = construction_phase("building the page", || {
            let mut document = new_document(viewport, config, outcomes);
            if let Some(pool) = style_pool {
                document.set_style_pool(pool);
            }
            if let Some(context) = text_context {
                document.adopt_text_context(context);
            }
            document
        })?;
        self.document = Some(document);
        Ok(())
    }

    /// Adopts whatever metrics an attached painter has named, and records
    /// that one has.
    ///
    /// The first call that finds a value is the binding: from then on this
    /// document lays out at the painter's size, and a flush publishes rather
    /// than parking. `borrow_and_update` marks the value seen for this
    /// receiver alone, which is why the `consume_metrics` task owns a clone
    /// of its own.
    fn adopt_metrics(&mut self) {
        let Some(metrics) = *self.metrics.borrow_and_update() else {
            return;
        };
        self.bound = true;
        let document = self.document_mut();
        let viewport = document.viewport_size();
        if viewport.width.to_bits() != metrics.width.to_bits()
            || viewport.height.to_bits() != metrics.height.to_bits()
        {
            document.set_viewport(metrics.width, metrics.height);
        }
        if document.device_pixel_ratio().to_bits() != metrics.device_pixel_ratio.to_bits() {
            document.set_device_pixel_ratio(metrics.device_pixel_ratio);
        }
    }

    /// Runs the whole pipeline and publishes the committed frame — the
    /// native half of `__FlushElementTree`.
    ///
    /// Two waits can come first, and each **parks the job it runs in**,
    /// exactly as `adoptStyleSheet` parks on its response: the engine
    /// thread's tasks go on running, no other job does, and the view's own
    /// token is the biased first arm so a release ends the wait.
    ///
    /// The first is the view's listed author sheets, which the first flush
    /// settles before anything is styled: one wait per sheet whose answer has
    /// not arrived, in listed order, each mounted as it is read. A sheet that
    /// failed to load, or that the fetcher answered with something else, is
    /// an error naming its URL, which `__FlushElementTree` throws — boot's own
    /// flush fails the boot with it. A frame is never committed without the
    /// sheets, because a frame styled without them would be published and
    /// then restyled.
    ///
    /// The second is the binding. Before any painter has bound, the frame
    /// committed here is held rather than published, because a painter
    /// composes at its own size and cannot tell that the frame it adopted
    /// predates the metrics it named, and the job parks on the metrics watch.
    /// Waking with different metrics discards the frame and commits again,
    /// which is the resize path. The sheets come first, so their IO and the
    /// painter's construction overlap and the frame held for the binding
    /// already carries them.
    ///
    /// Only the first binding is waited for. A painter that detaches leaves
    /// the last metrics behind, so every flush after it publishes at once.
    fn flush(&mut self, thread: &crate::jobs::JsThreadHandle) -> Result<(), String> {
        // The sheets first, then the metrics: a painter that binds while a
        // sheet is still in flight is adopted before the commit below, which
        // is then already at its size rather than held and recomputed.
        self.settle_sheets(thread)?;
        self.adopt_metrics();
        let frame = self.document_mut().commit();
        if self.bound {
            // A frame held from before the binding is older than this one, and
            // publishing it after this one would put the stale frame back.
            self.held = None;
            self.outbox.publish_frame(frame);
            self.request_wanted_images();
            return Ok(());
        }
        self.held = Some(frame);
        // Asked for before the wait rather than after it: the view's own
        // fetcher serves images whether or not a painter is attached, so
        // their IO overlaps the binding instead of starting behind it.
        self.request_wanted_images();
        let mut metrics = self.metrics.clone();
        let token = self.outbox.token().clone();
        thread.wait(async move {
            tokio::select! {
                biased;
                () = token.cancelled() => Err("view was released".to_owned()),
                named = metrics.wait_for(Option::is_some) => named
                    .map(|_| ())
                    .map_err(|_| "the view's seat is gone".to_owned()),
            }
        })?;
        self.adopt_metrics();
        if self.document_mut().needs_render() {
            self.held = Some(self.document_mut().commit());
        }
        if let Some(frame) = self.held.take() {
            self.outbox.publish_frame(frame);
        }
        self.request_wanted_images();
        Ok(())
    }

    /// Mounts every listed author sheet not mounted yet, in listed order,
    /// waiting inside the job for each answer that has not arrived.
    ///
    /// A sheet is taken off the front of the list before it is read, so a
    /// failure consumes that sheet alone: the error is thrown to the caller
    /// of `__FlushElementTree`, and a later flush goes on with the sheets
    /// behind it.
    fn settle_sheets(&mut self, thread: &crate::jobs::JsThreadHandle) -> Result<(), String> {
        if self.sheets.is_empty() {
            return Ok(());
        }
        let token = self.outbox.token().clone();
        while !self.sheets.is_empty() {
            let StartupSource { url, answer } = self.sheets.remove(0);
            style_sheets::settle_style_sheet(self.document_mut(), thread, &token, &url, answer)?;
        }
        Ok(())
    }

    /// Commits when anything is stale, and publishes it if a painter has
    /// bound — the epilogue of every entry into the realm, which is what
    /// makes "we do not guarantee the tree is not flushed outside
    /// `__FlushElementTree`" true.
    ///
    /// It never waits: the epilogue runs after every entry, and parking here
    /// would stop the group on any of them. So while any listed author sheet
    /// is still outstanding it does nothing at all: a commit without the
    /// sheets would publish an unstyled frame, and waiting for them is the
    /// first [`Self::flush`]'s. Nothing is lost by skipping — boot's own
    /// `__FlushElementTree` is what commits the first frame, and it settles
    /// the sheets before it does. What it commits before the binding is held
    /// for whichever flush or binding publishes next.
    fn commit_if_dirty(&mut self) {
        if !self.sheets.is_empty() {
            return;
        }
        self.adopt_metrics();
        if self.document_mut().needs_render() {
            let frame = self.document_mut().commit();
            if self.bound {
                // Superseded, as in `flush`: the held frame is older.
                self.held = None;
                self.outbox.publish_frame(frame);
            } else {
                self.held = Some(frame);
            }
            // The walk asked for its images either way: the view's own
            // fetcher serves them whether or not a painter is attached.
            self.request_wanted_images();
        } else if self.bound
            && let Some(frame) = self.held.take()
        {
            // Bound at the metrics this frame was already committed for, so
            // there is nothing to recompute and it goes out as it is.
            self.outbox.publish_frame(frame);
        }
    }

    /// Asks the host for the image sources the last walk discovered.
    ///
    /// The walk that just ran is the one place that knows which sources a
    /// frame needs. Empty on every commit that met no new image, which is
    /// almost all of them.
    fn request_wanted_images(&mut self) {
        let wanted = self.document_mut().take_wanted_images();
        if !wanted.is_empty() {
            self.outbox.notify(ViewNotice::RequestImages(wanted));
        }
    }

    /// The `@font-face` rules the mounted sheets declared, once each.
    ///
    /// Empty before `createDocument`: there is no cascade yet, and the
    /// epilogue of the entry that mounts a sheet asks again.
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
/// name. JavaScript first, then the Rust object it named. `core` is first: the
/// realm, then the timer and future tables the realm's core members wrote to,
/// neither of which runs JavaScript when it drops.
///
/// `workers` follows the realm handles: its channels belong to this MTS
/// runtime and close when it is released. JS host functions reference it
/// weakly, so queued finalizers cannot extend the channels' lifetime.
pub(crate) struct MainThreadRuntime {
    /// The realm and the members every realm has, which
    /// [`crate::realm::open_realm`] installed.
    core: RealmCore,
    /// This realm's side of the workers it created, shared with the three
    /// host functions that drive them, and where a worker failure is reported
    /// from.
    workers: Rc<super::workers::WorkerOwner>,
    slot: Rc<RefCell<DocumentSlot>>,
    /// The newest reading of the view's timeline this side has been handed —
    /// a `BeginFrame`'s `now` or a vsync's, both in milliseconds off the same
    /// epoch, the view's construction.
    ///
    /// The timeline is read on the painting side, where the frame clock is, so
    /// a routed event arrives carrying its own reading and this is never used
    /// for one. What it stamps is an event this side originates — an image's
    /// `load` or `error`, which settles between frames — with the last moment
    /// the two sides agreed on, and with the time origin before the first
    /// frame. One clock, one epoch; nothing here takes a second reading of its
    /// own.
    timeline_milliseconds: f64,
    /// The screen this realm's `SystemInfo` reports, as the embedder named
    /// it. Held from construction because boot writes the three numbers into
    /// its own module source, which runs after the realm exists.
    screen: ScreenMetrics,
    /// The page configuration boot writes into its own module source as four
    /// boolean literals, held for the same reason as [`Self::screen`].
    config: PageConfig,
    /// The URL the view named its MTS entry by, which boot writes into its
    /// own module source as the specifier it imports the entry by. Held for
    /// the same reason as [`Self::screen`], and read again by
    /// [`Self::complete_entry`] and [`Self::entry_module_name`].
    entry: String,
}

impl fmt::Debug for MainThreadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MainThreadRuntime")
            .finish_non_exhaustive()
    }
}

impl MainThreadRuntime {
    /// Opens one view's realm through [`crate::realm::open_realm`], which
    /// installs the core every realm has, and installs this realm's own host
    /// members — the document and host module, the stylesheet members, the
    /// startup strings and the `Worker` bindings — handing back the one
    /// channel everything this view's workers say arrives on.
    ///
    /// A realm has its `Worker` members from the moment it exists: there is no
    /// state in which it is missing them, and so no order between furnishing
    /// the realm and installing them for a caller to get wrong. Which worker
    /// this realm creates is still the realm's own business — the boot module
    /// constructs the built-in background context, during the entry evaluation
    /// this call does not make.
    ///
    /// Opening spends the startup: the strings become one-shot members read
    /// later by the boot module, the screen and the page configuration are
    /// kept for the boot module's own source, and the author sheets' answers
    /// go to the document slot for the first `__FlushElementTree` to settle.
    /// Nothing here waits for any of them, and the entry is not part of it,
    /// which is what lets a realm open while its own sources are still in
    /// flight.
    pub(crate) fn new(
        js_runtime: &mut ScriptRuntime,
        ingredients: DocumentIngredients,
        // The painter's metrics, as the view's seat publishes them. Link
        // state rather than a document input: it outlives the one
        // construction the ingredients are for, and an unbound flush parks on
        // it.
        metrics: watch::Receiver<Option<Viewport>>,
        outbox: ViewOutbox,
        workers: &super::workers::WorkerFactory,
        // What `adoptStyleSheet`, a `Future.wait`, a `require` and an unbound
        // `__FlushElementTree` park on: the engine thread this realm's
        // entries are jobs of.
        thread: crate::jobs::JsThreadHandle,
        mut startup: RealmStartup,
    ) -> Result<
        (
            Self,
            tokio::sync::mpsc::UnboundedReceiver<crate::background::WorkerEvent>,
        ),
        MainThreadError,
    > {
        let config = ingredients.config;
        let sheets = std::mem::take(&mut startup.sheets);
        let slot = DocumentSlot::new(ingredients, sheets, metrics, outbox.clone());
        // The view's own token: what this realm's core members park on ends
        // with the view.
        let host = outbox.host_outbox(outbox.token().clone());
        let (core, (workers, incoming)) = open_realm(
            js_runtime,
            &host,
            thread.clone(),
            None,
            |engine, js_runtime| {
                install_bobcat(engine, js_runtime, &slot, &outbox, thread.clone())
                    .map_err(MainThreadError::into_script_error)?;
                style_sheets::install_styles(engine, js_runtime, &slot, &outbox, thread)
                    .map_err(MainThreadError::into_script_error)?;
                install_startup_strings(engine, js_runtime, startup_strings(&mut startup))
                    .map_err(MainThreadError::into_script_error)?;
                workers
                    .install(engine, js_runtime, outbox, startup.background_entry.take())
                    .map_err(|error| context_of("installing Worker", error))
            },
        )
        .map_err(|error| MainThreadError::from_engine("opening the MTS realm", error))?;
        Ok((
            Self {
                core,
                workers,
                slot,
                timeline_milliseconds: 0.0,
                screen: startup.screen,
                config,
                entry: startup.entry,
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
            .core
            .engine
            .call_module_export(
                js_runtime,
                WORKER_CLASS_MODULE_SPECIFIER,
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

    /// Adopts the painter's metrics, commits when anything is stale, and
    /// publishes once a painter has bound. Called by the page's epilogue,
    /// after every entry into the realm, and never waits.
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
            .core
            .engine
            .call_module_export(js, RUNTIME_MODULE_SPECIFIER, export, arguments)
            .map_err(|error| MainThreadError::from_engine("updating page data", error));
        let finished = self.finish_batch(js, called.is_ok());
        called.map(|_| ()).and(finished)
    }

    /// Advances the animation timeline to the painting side's clock
    /// reading. Whether anything changed is the next commit's business.
    pub(crate) fn begin_frame(&mut self, now: f64) {
        // Seconds here, as the animation timeline wants them; milliseconds is
        // what an event's `timestamp` is in.
        self.timeline_milliseconds = now * 1000.0;
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
        self.timeline_milliseconds = milliseconds;
        self.core
            .engine
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

    /// Applies the painting side's image reports, queueing the `load`s and
    /// `error`s they settle for this entry's epilogue to dispatch.
    ///
    /// Queued rather than dispatched here for one reason only — so that both
    /// producers answer the same way. The other one is the `image` component,
    /// which settles a `src` from inside the `__SetAttribute` that wrote it
    /// and may not re-enter the realm at all.
    pub(crate) fn apply_image_events(&mut self, events: &[dom::ImageEvent]) {
        let mut slot = self.slot.borrow_mut();
        let outcomes = slot.document_mut().apply_image_events(events);
        slot.image_outcomes.extend(outcomes);
    }

    /// Whether either producer has left an image event owing — the queue
    /// [`Self::dispatch_image_outcomes`] drains, asked once per entry.
    ///
    /// One `is_empty`, because nearly every entry's answer is no: an entry
    /// during which no source settled posts nothing. A `load` handler that
    /// writes a `src` this document has already seen settle gets its outcome
    /// at the bind, so a delivery entry can leave the queue non-empty for an
    /// entry of its own.
    pub(crate) fn has_image_outcomes(&self) -> bool {
        !self.slot.borrow().image_outcomes.is_empty()
    }

    /// Dispatches everything [`Self::apply_image_events`] and the `image`
    /// component have queued, in the order they settled.
    ///
    /// Called by the entry the page posts for the batch and by nothing else:
    /// the events are tasks, not part of the entry that settled the source.
    /// Each is one non-bubbling dispatch at the element whose own `src`
    /// settled, which is web-core's shape for both events
    /// (`commonEventInitConfiguration.ts`: `bubbles: false`). An element freed
    /// between the outcome forming and this call resolves to nothing, exactly
    /// as a routed input event at a freed target does.
    ///
    /// Returns what the listeners threw. A `load` handler that throws is
    /// nonfatal, the standing every event listener has here.
    pub(crate) fn dispatch_image_outcomes(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Vec<MainThreadError> {
        let outcomes = self.slot.borrow().image_outcomes.take();
        let timestamp = self.timeline_milliseconds;
        let mut failures = Vec::new();
        for outcome in outcomes {
            // [`EventDetail::Empty`] is the realm's `{}`, which is web-core's
            // `error` detail exactly; a `load` carries the bitmap's
            // *intrinsic* size, web-core's `naturalWidth`/`naturalHeight`,
            // not the box it drew into.
            let (target, name, detail) = match outcome {
                dom::ImageOutcome::Loaded {
                    node,
                    width,
                    height,
                } => (
                    node,
                    LOAD_EVENT,
                    EventDetail::Size {
                        width: f64::from(width),
                        height: f64::from(height),
                    },
                ),
                dom::ImageOutcome::Failed { node } => (node, ERROR_EVENT, EventDetail::Empty),
            };
            if let Err(error) = self.dispatch(js_runtime, target, name, false, timestamp, &detail) {
                failures.push(error);
            }
        }
        failures
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
    /// Every routed input event bubbles — the painting side routes what a
    /// gesture produced, and Lynx has no non-bubbling input event — so this
    /// is [`Self::dispatch`] with the flag set and the payload's numbers for
    /// a detail. The events that do not bubble are an `<image>`'s `load` and
    /// `error`, which [`Self::dispatch_image_outcomes`] delivers.
    pub(crate) fn dispatch_input_event(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        payload: &InputEventPayload,
    ) -> Result<bool, MainThreadError> {
        self.dispatch(
            js_runtime,
            target,
            name,
            true,
            payload.timestamp,
            &EventDetail::Input(payload),
        )
    }

    /// One whole dispatch, whatever produced it: one call into the realm,
    /// carrying the path, the flag that shapes the walk, the timestamp and
    /// the numbers the `detail` is made of.
    ///
    /// A target freed since the event formed resolves to nothing rather than
    /// a path — a `NodeId` names one node for the life of the document, so
    /// the check is one lookup and can never hit a stranger.
    ///
    /// The path crosses as the standard's event path — every step, in
    /// target-first, root-last order, each with its shadow-retargeted target
    /// — encoded as two comma-joined decimal id strings, which is how
    /// `childElementIds` carries a list already: the boundary takes
    /// primitives and structured clones only, a clone can be minted by the
    /// realm alone, and a decimal id cannot contain the separator. One call
    /// carries the whole dispatch, and what the realm then runs over it —
    /// the passes, whether any listener exists at all — is the realm's
    /// business, because every registration lives there.
    ///
    /// `bubbles` crosses as itself rather than as a shortened path, because
    /// the passes narrow differently: a non-bubbling event still captures down
    /// the whole path, binds on its at-target steps alone, and runs no
    /// `global-bindEvent` pass at all — web-core's `common_event_handler`
    /// (`web-core/src/main_thread/client/element_apis/event_apis.rs:413-432`),
    /// which is handed the same one flag off the DOM event
    /// (`WASMJSBinding.ts:254-258`). Sending a narrowed path instead would
    /// lose the capture pass.
    ///
    /// Everything else crosses as numbers, and the realm builds the objects:
    /// the `timestamp`, then [`EventDetail`]'s discriminator and whatever
    /// numbers that kind spends. No JSON is formatted here and none is parsed
    /// there.
    ///
    /// The document is released before the call, which is what lets a
    /// listener mutate the tree.
    ///
    /// Returns whether the realm published the export, which is all the host
    /// can know: nothing here says whether anything ran.
    fn dispatch(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        bubbles: bool,
        timestamp: f64,
        detail: &EventDetail<'_>,
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

        // Inline for the ten an input event without touch points spends: six
        // of header, then its four detail numbers. A touch event spills once,
        // and its points are the only thing that ever grows this.
        let mut arguments: SmallVec<[HostArgument<'_>; 10]> = SmallVec::new();
        arguments.push(HostArgument::String(&nodes));
        arguments.push(HostArgument::String(&targets));
        arguments.push(HostArgument::String(name));
        arguments.push(HostArgument::Boolean(bubbles));
        arguments.push(HostArgument::Number(timestamp));
        arguments.push(HostArgument::Number(detail.kind()));
        detail.push_numbers(&mut arguments);

        let called = self
            .core
            .engine
            .call_module_export(
                js_runtime,
                ELEMENT_MODULE_SPECIFIER,
                EVENT_DISPATCH_EXPORT,
                &arguments,
            )
            .map_err(|error| MainThreadError::from_engine("delivering an event", error))?;
        // Listeners remove elements too; the count they ran up is settled
        // here, at the end of the dispatch.
        self.finish_batch(js_runtime, true)?;
        Ok(called)
    }

    /// [`Self::dispatch_input_event`] for a test that is about the dispatch
    /// rather than about the payload: the event happened at `position`, with
    /// no wheel delta, no touch points and the time origin for a timestamp.
    #[cfg(test)]
    pub(crate) fn dispatch_for_test(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        target: dom::NodeId,
        name: &str,
        position: dom::Point2D<f32>,
    ) -> Result<bool, MainThreadError> {
        self.dispatch_input_event(
            js_runtime,
            target,
            name,
            &InputEventPayload {
                position,
                ..InputEventPayload::default()
            },
        )
    }

    /// Whether the commits since the last delivery left any
    /// `content-visibility: auto` skipping change owing — css-contain-2
    /// §4.4's queue, asked once per entry.
    ///
    /// One `is_empty` on a `Vec`, because nearly every entry's answer is no:
    /// a page whose `auto` boxes did not change state posts nothing.
    pub(crate) fn has_pending_content_visibility_changes(&self) -> bool {
        self.slot
            .borrow()
            .document
            .as_ref()
            .is_some_and(LynxDocument::has_pending_content_visibility_changes)
    }

    /// Fires one `contentvisibilityautostatechange`
    /// ([css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event))
    /// per queued change, in frame order.
    ///
    /// The whole dispatch is `dom`'s: it owns the path, the walk and the
    /// listeners, because the listeners are the engine's own components
    /// (`dom::CustomElement` definitions — `<image>` today, `<list>` when it
    /// exists) rather than anything in a realm. **No realm is entered**,
    /// which is why this takes no `ScriptRuntime`: the event has no script
    /// form, no Lynx event name, and nothing about it is published to the
    /// painting or the background side. What this method adds is *when*: the
    /// spec dispatches the event "by posting a task at the time when the
    /// state change occurs", and the entry the page posts for it is that
    /// task.
    ///
    /// A handler may mutate the tree — the changes are taken before the walk
    /// starts, each step re-checks its node, and whatever a handler wrote is
    /// committed by this entry's own epilogue.
    pub(crate) fn dispatch_content_visibility_changes(&mut self) {
        self.slot
            .borrow_mut()
            .document_mut()
            .dispatch_content_visibility_changes();
    }

    /// When the earliest armed timer comes due, if one is armed.
    pub(crate) fn next_timer_deadline(&mut self) -> Option<ClockInstant> {
        self.core.timers.next_deadline()
    }

    /// Runs every timer due now, in the order the standard fires them.
    ///
    /// Returns whatever their callbacks threw. A timer is its own task, so
    /// one that throws neither stops the ones behind it nor ends the realm —
    /// the same standing an event listener that throws already has.
    pub(crate) fn run_due_timers(&mut self, js_runtime: &mut ScriptRuntime) -> Vec<ScriptError> {
        let Some(mut failures) =
            run_due_timers(&mut self.core.engine, js_runtime, &self.core.timers)
        else {
            return Vec::new();
        };
        // Callbacks remove elements like any other realm entry point; the
        // count they ran up is settled here, at the end of the batch.
        if let Err(error) = self.finish_batch(js_runtime, true) {
            failures.push(error.into_script_error());
        }
        failures
    }

    /// Evaluates `bobcat:boot`, which is this realm's whole startup: it
    /// creates the document, imports the entry, connects the BTS Worker,
    /// renders, and flushes.
    ///
    /// Nothing is waited for before this runs. The entry reaches the realm
    /// through an ordinary `import` of the URL the view named it by, a module
    /// a task of the view's owner completes from the answer
    /// `create_lynx_view` already asked for, and the author stylesheets are
    /// mounted by boot's own `__FlushElementTree`, which waits for each before
    /// the document is styled. So this returns with a document in place
    /// however long the entry takes, and boot's own completion is the promise
    /// `main_module_finished` reads. The only two things boot waits on are
    /// both inside that flush: the listed sheets, and a painter's binding.
    ///
    /// How much of boot has run when this returns depends on the entry alone:
    /// an entry already completed is found in the realm's registry and boot
    /// runs through to its own flush here; one still outstanding leaves the
    /// import pending, and the rest of boot runs in the job that completes it.
    ///
    /// The only literals written into it are the screen's three numbers, the
    /// page configuration's four switches and the entry's URL — facts Rust
    /// owns, written as primitives rather than as JSON the realm would parse
    /// and hand back. The URL is written as a JSON string literal, which is
    /// the one quoting that is also a JavaScript string literal.
    /// `SystemInfo` describes the screen the page is shown on, which this
    /// view's viewport is not; the switches are what the realm builds its
    /// document with and what `enableJSDataProcessor` tells the MTS runtime.
    pub(crate) fn run_boot_module(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Result<(), MainThreadError> {
        let ScreenMetrics {
            pixel_ratio,
            pixel_width,
            pixel_height,
        } = self.screen;
        let PageConfig {
            default_display_linear,
            default_overflow_visible,
            enable_css_selector,
            enable_js_data_processor,
        } = self.config;
        let entry = serde_json::to_string(&self.entry).expect("a string serializes");
        let boot = format!(
            r#"import {{ lynx, __BobcatConnectBackground, __BobcatInitializeMTS, __BobcatProcessInitData, __BobcatRenderPage }} from "{RUNTIME_MODULE_SPECIFIER}";
import {{ Document, __FlushElementTree }} from "{ELEMENT_MODULE_SPECIFIER}";
// Imported for its effect: it installs the timer globals, and a static
// import runs before the entry this module then loads.
import "{TIMER_MODULE_SPECIFIER}";

const config = {{
  defaultDisplayLinear: {default_display_linear},
  defaultOverflowVisible: {default_overflow_visible},
  enableCssSelector: {enable_css_selector},
  enableJSDataProcessor: {enable_js_data_processor},
}};

// The realm's document, created out of that configuration and held by this
// exported binding for the realm's life. Nothing in the realm releases it: it
// goes when the realm does. The view's own resources — its metrics, its fonts,
// its style pool and its author stylesheets — stay on the host side; the
// first flush below mounts the author sheets, in listed order, before the
// document is styled.
export const document = new Document(config);

__BobcatInitializeMTS({{
  enableJSDataProcessor: config.enableJSDataProcessor,
  systemInfo: {{pixelRatio: {pixel_ratio}, pixelWidth: {pixel_width}, pixelHeight: {pixel_height}}},
}});
// React's entry clears lynx.__initData during initialization. The host's
// first-screen argument belongs to boot, independently of that mutable slot.
let data = lynx.__initData;

// The entry, by the URL the view named it by. A task of the view's owner
// completes this module from the answer the view asked for before this realm
// opened, answered from the response URL the fetcher gave, so this import
// asks the fetcher for nothing, and names that URL as `__Card__` before the
// entry's body runs.
await import({entry});
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

    /// Boots a realm over `source` as its entry, without a fetcher behind it —
    /// the seam this crate's own tests and benchmarks drive boot through.
    ///
    /// `source_name` is both the URL the entry is requested by and the one it
    /// is answered from; it replaces whatever entry the realm was opened with.
    /// The production boot runs up to its `import` of the entry, and the entry
    /// is then completed with the source in hand, as the view's own entry task
    /// would with a fetcher's answer, which runs the rest of boot: the same
    /// module, the same members and the same order a view boots in.
    ///
    /// The result is boot's: an error if it failed, whether before the import
    /// or in the entry, and `Ok` once it settled or while it still waits for a
    /// resource or a timer.
    pub(crate) fn run_main_thread_script(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        source_name.clone_into(&mut self.entry);
        self.run_boot_module(js_runtime)?;
        // The one request boot left is its entry, which the view's owner
        // never sends to a fetcher; it is answered below.
        let requested = self.take_module_request();
        debug_assert_eq!(requested, self.entry_module_name().ok());
        self.complete_entry(
            js_runtime,
            source_name,
            Ok(LoadedSource::Module {
                source: source.to_owned(),
                url: source_name.to_owned(),
            }),
        )?;
        let finished = self.main_module_finished().map(|_| ());
        let collected = self.finish_batch(js_runtime, finished.is_ok());
        finished.and(collected)
    }

    /// Boots a realm over `source` as its entry the way a view whose author
    /// sheets the fetcher answered before its first flush boots: each sheet is
    /// handed to the document slot as an answer already in hand, in the order
    /// given, which is the listed order, and boot's own `__FlushElementTree`
    /// mounts them before it styles the document. `source_name` is both the
    /// entry's request URL and its response URL, as in
    /// [`Self::run_main_thread_script`].
    ///
    /// The seam for this crate's own tests of cards that come with a sheet.
    #[cfg(test)]
    pub(crate) fn run_main_thread_script_over_sheets(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        sheets: Vec<(&str, LoadedSource)>,
        source: &str,
        source_name: &str,
    ) -> Result<(), MainThreadError> {
        {
            let mut slot = self.slot.borrow_mut();
            let token = slot.outbox.token().clone();
            slot.sheets = sheets
                .into_iter()
                .map(|(url, sheet)| {
                    let (completion, answer) =
                        crate::resource::SourceCompletion::new(token.clone());
                    completion.complete(Ok(sheet));
                    StartupSource {
                        url: url.to_owned(),
                        answer,
                    }
                })
                .collect();
        }
        source_name.clone_into(&mut self.entry);
        self.run_boot_module(js_runtime)?;
        // The one request boot left is its entry, which the view's owner
        // never sends to a fetcher.
        assert_eq!(self.take_module_request(), self.entry_module_name().ok());
        self.complete_entry(
            js_runtime,
            source_name,
            Ok(LoadedSource::Module {
                source: source.to_owned(),
                url: source_name.to_owned(),
            }),
        )
    }

    /// The next module an import in this realm is waiting for, if any.
    pub(crate) fn take_module_request(&mut self) -> Option<String> {
        self.core.engine.take_module_request()
    }

    /// The name boot's `import` of the entry asks the realm for: the entry's
    /// URL, normalized the way the module loader normalizes a specifier
    /// imported from `bobcat:boot`. For this entry the normalization is the
    /// identity: `create_lynx_view` already resolved it to an absolute URL's
    /// serialization, which the loader serializes to itself. A name that is
    /// not an absolute URL, which only a seam that skips `create_lynx_view`
    /// can put here, is refused with the loader's own message, which is also
    /// what rejects boot's `import`.
    pub(crate) fn entry_module_name(&self) -> Result<String, String> {
        super::quickjs::normalize_module_url(BOOT_MODULE_SPECIFIER, &self.entry)
    }

    /// Completes the module boot imports the entry as, from the answer to the
    /// entry request `create_lynx_view` made.
    ///
    /// The module is the one [`Self::entry_module_name`] names — the name
    /// boot's `import` asks for, the request URL, which is also the name its
    /// errors and stack frames carry — completed like any other import: a
    /// script answer is the entry with the entry preamble prepended, answered
    /// from the fetcher's response URL. That URL is the base its own relative
    /// imports resolve against and its `import.meta.url`. A load that failed,
    /// and an answer that is not a script, complete the module with an error
    /// naming the URL the view asked for and the reason, which rejects boot's
    /// `import` and so fails the boot with that message.
    ///
    /// **The entry is named here.** Before a script answer is completed, the
    /// response URL — the fetcher's, so a redirect is already applied — is
    /// handed to `bobcat:runtime`'s `__BobcatInitEntry`, which makes it
    /// `__Card__`: this page's own container URL for the body, for every chunk
    /// and stylesheet it names by `__Card__`, and for the base URL a
    /// `new Worker` specifier resolves against. A chunk is never named: it
    /// would overwrite `__Card__` with its own URL.
    ///
    /// Boot has run first: `bobcat:runtime` is one of its static imports, so
    /// it is instantiated by the time this is called, and boot's `import` of
    /// the entry is the pending request this completion resumes. The view's
    /// entry task is queued behind `open_realm`, which runs boot, and
    /// [`Self::run_main_thread_script`] and its variant with sheets keep
    /// the same order.
    pub(crate) fn complete_entry(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        requested: &str,
        answered: Result<LoadedSource, LynxViewError>,
    ) -> Result<(), MainThreadError> {
        let booting = |error| MainThreadError::from_engine("booting the MTS entry", error);
        let name = self.entry_module_name().map_err(|message| {
            booting(ScriptError {
                kind: crate::script::ScriptErrorKind::ModuleLoad,
                phase: crate::script::ScriptErrorPhase::ExecuteModule,
                message: format!("the MTS entry {requested}: {message}").into(),
                location: None,
            })
        })?;
        let loaded = match answered {
            Ok(LoadedSource::Module { source, url }) => Ok((url, entry_module_source(&source))),
            Ok(_) => Err(format!("the MTS entry {requested} is not a script")),
            Err(error) => Err(format!("loading the MTS entry {requested}: {error}")),
        };
        // Without a checkpoint of its own: the completion below drains the
        // queue for both, so naming the entry wakes no sibling realm between
        // them.
        if let Ok((url, _)) = &loaded {
            self.core
                .engine
                .call_module_export_before_operation(
                    js_runtime,
                    RUNTIME_MODULE_SPECIFIER,
                    "__BobcatInitEntry",
                    &[HostArgument::String(url)],
                )
                .map_err(booting)?;
        }
        self.core
            .engine
            .complete_module(
                js_runtime,
                &name,
                loaded
                    .as_ref()
                    .map(|(url, source)| (url.as_str(), source.as_str()))
                    .map_err(String::as_str),
            )
            .map_err(booting)
    }

    /// The futures this realm asked to settle asynchronously — a `.then` on a
    /// `Future` — since the last entry. Each is a task for the view's owner.
    pub(crate) fn take_future_settles(&mut self) -> Vec<(u32, crate::future::HostFuture)> {
        self.core.futures.take_settle_requests()
    }

    /// Hands one settled future back to the realm that asked for it.
    pub(crate) fn deliver_future(
        &mut self,
        js_runtime: &mut ScriptRuntime,
        id: u32,
        outcome: crate::future::Outcome,
    ) -> Result<(), MainThreadError> {
        crate::future::deliver(&mut self.core.engine, js_runtime, id, outcome)
            .map_err(|error| MainThreadError::from_engine("settling a future", error))
    }

    /// The `@font-face` rules the sheets mounted so far declared and this
    /// realm has not reported before. Empty until a sheet is mounted.
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
        self.core
            .engine
            .module_finished()
            .map_err(|error| MainThreadError::from_engine("booting the MTS entry", error))
    }

    /// Run the MTS module's asynchronous disposal through the ordinary ESM
    /// evaluator. JavaScript owns the BTS request, acknowledgement and stop.
    pub(crate) fn begin_dispose(
        &mut self,
        js_runtime: &mut ScriptRuntime,
    ) -> Result<(), MainThreadError> {
        self.core
            .engine
            .start_module(
                js_runtime,
                "import { __BobcatDispose } from 'bobcat:runtime'; await __BobcatDispose();",
                "bobcat:dispose",
            )
            .map_err(|error| MainThreadError::from_engine("disposing the MTS realm", error))
    }

    pub(crate) fn disposal_finished(&mut self) -> Result<bool, MainThreadError> {
        self.core
            .engine
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
            Ok(LoadedSource::Module { source, url }) => Ok((url, source)),
            Ok(LoadedSource::StyleSheet(_)) => {
                Err("a module request returned a stylesheet".to_owned())
            }
            Ok(LoadedSource::Font(_)) => Err("a module request returned a font".to_owned()),
            Ok(LoadedSource::Fetched) => Err("a module request returned a plain fetch".to_owned()),
            Err(error) => Err(format!("module '{name}': {error}")),
        };
        self.core
            .engine
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
        self.core
            .engine
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
            .core
            .engine
            .start_module(js_runtime, source, name)
            .map_err(|error| MainThreadError::from_engine(phase, error));
        let finished = self.finish_batch(js_runtime, result.is_ok());
        result.and(finished)
    }
}

fn install_bobcat(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    handle: &Rc<RefCell<DocumentSlot>>,
    outbox: &ViewOutbox,
    thread: crate::jobs::JsThreadHandle,
) -> Result<(), MainThreadError> {
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
    install_host_module(engine, js_runtime, handle, thread)?;
    install_event_members(engine, js_runtime, outbox)
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
    // `flushElementTree`'s, for the waits it makes on the listed author sheets
    // and before a painter has bound.
    thread: crate::jobs::JsThreadHandle,
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
    install_readback_members(engine, js_runtime, handle)?;

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
            borrow_slot("bobcat-internal:host.flushElementTree", &tree)?.flush(&thread)?;
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
    const NAME: &str = "bobcat-internal:host.createDocument";
    let tree = Rc::clone(handle);
    install(engine, js_runtime, "createDocument", 4, move |arguments| {
        // The four switches, in `PageConfig`'s order, as the boot module
        // wrote them. Read rather than passed through because Rust itself
        // needs the fields: the UA cascade and the document's own switches are
        // built out of them. A missing or non-boolean one fails the
        // construction, which fails the boot.
        let config = PageConfig {
            default_display_linear: boolean_argument(NAME, arguments, 0)?,
            default_overflow_visible: boolean_argument(NAME, arguments, 1)?,
            enable_css_selector: boolean_argument(NAME, arguments, 2)?,
            enable_js_data_processor: boolean_argument(NAME, arguments, 3)?,
        };
        // The page id is not this member's answer: `createPage` is still what
        // hands the realm the permanent root, and it now has a document to
        // read it from.
        borrow_slot(NAME, &tree)?.create_document(config)?;
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
/// reads the module table as the record the realm decodes. All four answer
/// before a document exists.
///
/// The strings are *moved* in rather than copied: each is handed over once and
/// never read again, so the move says what the lifetime is.
fn install_startup_strings(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    strings: [(&'static str, Option<String>); 4],
) -> Result<(), MainThreadError> {
    for (name, mut value) in strings {
        install(engine, js_runtime, name, 0, move |_arguments| {
            Ok(value.take().map_or(HostValue::Undefined, HostValue::String))
        })?;
    }
    Ok(())
}

/// The four strings this realm answers once, in the order they are
/// installed, taken out of the startup that is being spent.
fn startup_strings(startup: &mut RealmStartup) -> [(&'static str, Option<String>); 4] {
    [
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
    ]
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

/// Installs the two members that read geometry and style back out of the
/// document.
///
/// Neither runs a pipeline step. Both report what the last completed pass
/// left behind, and the realm decides when the next one runs by calling
/// `__FlushElementTree` — measuring must not be able to move layout out from
/// under the job that measures, and a card that wants current numbers says
/// so. Both still go through [`validate_live_element`], so a freed element
/// is a script error rather than a zero rect or an empty style.
fn install_readback_members(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    handle: &Rc<RefCell<DocumentSlot>>,
) -> Result<(), MainThreadError> {
    tree_members! { engine, js_runtime, handle;
        fn callElementMethod(node: node_id_argument, method: string_argument) |document| {
            validate_live_element(document, NAME, node)?;
            // One method, dispatched by name because the PAPI is generic:
            // anything else is "no such method" for the realm to turn into
            // the shared table's code 3. `params` is not carried — nothing
            // reads one, and the first method that does brings it.
            if method != "boundingClientRect" {
                return Ok(HostValue::Null);
            }
            // A box-less element answers zeros rather than nothing, which is
            // what both references report for one.
            let rect = document
                .bounding_client_rect(node)
                .unwrap_or_else(dom::Rect::zero);
            Ok(HostValue::String(format!(
                "{},{},{},{}",
                rect.origin.x, rect.origin.y, rect.size.width, rect.size.height
            )))
        }
        fn getComputedStyleMap(
            node: node_id_argument,
            properties: string_argument,
            resolved: flag_argument
        ) |document| {
            validate_live_element(document, NAME, node)?;
            // An empty payload is "every property", so it must not become the
            // one-element filter `"".split(',')` would produce.
            let filter: Vec<&str> = if properties.is_empty() {
                Vec::new()
            } else {
                properties.split(',').collect()
            };
            let mut record = String::new();
            for (name, value) in document.computed_style_entries(node, &filter, resolved) {
                write_record_field(&mut record, &name);
                write_record_field(&mut record, &value);
            }
            Ok(HostValue::String(record))
        }
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
///
/// An attribute a tag reflects into CSS — `text-maxline`, `span-count`,
/// `src` — is reflected by that tag's own component, in the
/// `attribute_changed_callback` [`LynxDocument`] raises and drains inside the
/// write below. This layer only performs the DOM mutation: it neither knows
/// which names mean something nor which tag they mean it on.
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

/// A flag the realm spells as `0` or `1`, which is every boolean a tree
/// member takes.
fn flag_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Number(0.0) => Ok(false),
        HostValue::Number(1.0) => Ok(true),
        _ => Err(format!("{function} expects 0 or 1 for argument {index}")),
    }
}

/// A JavaScript boolean, which is what the boot module hands `createDocument`
/// its four switches as.
fn boolean_argument(function: &str, arguments: &[HostValue], index: usize) -> Result<bool, String> {
    match *argument(arguments, index) {
        HostValue::Boolean(value) => Ok(value),
        _ => Err(format!("{function} expects a boolean for argument {index}")),
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
