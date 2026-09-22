//! What a view's tasks owe each other, driven with the test standing in for
//! both ends of the link.
//!
//! Real `QuickJS`, a real [`JsThread`] and the real [`serve_view`], with no
//! group thread, no painter and no GPU: the test answers every source request
//! by hand and reads what the view published. That is the whole seam these
//! pins need, because what they are about is which task ran, which job ran, in
//! what order, and how many entries into the realm it took.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::background::{WorkerCommand, WorkerMessage, WorkerStart};
use crate::jobs::{JsThread, JsThreadHandle};
use crate::link::{DetachedView, InputEventPayload, PageUpdate, ViewNotice, detached_outbox};
use crate::main::WorkerFactory;
use crate::main::runtime::{bound_metrics, install_shared_modules};
use crate::main::tree::PageConfig;
use crate::resource::{SourceCompletion, SourceRequest};
use crate::view::{NoWakeup, StartupSource, StartupSources};

/// How many times the harness lets every ready task run before it gives up on
/// something happening. A hang detector rather than a schedule: everything
/// here is on one thread and cooperative, so a step that has not happened in
/// this many turns is a step that never will.
const TURNS: usize = 512;

/// Runs one test body as the `main` task of a [`JsThread`], which is the shape
/// `bobcat-main` itself runs: the body's own turns are scheduler turns, and the
/// jobs its views queue run between them.
fn on_a_js_thread<F, B>(body: B)
where
    F: std::future::Future<Output = ()> + 'static,
    B: FnOnce(JsThreadHandle) -> F,
{
    let thread = JsThread::new();
    let body = body(thread.handle());
    thread.run(body);
}

/// Opens one page's realm the way [`serve_view`] does: as one job of that
/// page's, awaited, with its entry already answered.
async fn open_realm(page: &Rc<Page>, entry: &str, url: &str) {
    let startup = RealmStartup {
        startup: answered_startup(entry, url, &page.lifetime.token().clone()),
        ..RealmStartup::default()
    };
    crate::lifetime::run_job(page, move |page| {
        page.open_realm(ingredients(), startup);
        Some(())
    })
    .await;
}

/// The startup of a view that lists no stylesheets and whose entry the
/// fetcher answered before the realm opened, which is what every pin here
/// that is not about the loading itself wants.
fn answered_startup(entry: &str, url: &str, token: &CancellationToken) -> StartupSources {
    let (completion, answer) = SourceCompletion::new(token.clone());
    completion.complete(Ok(LoadedSource::Entry {
        source: entry.to_owned(),
        url: url.to_owned(),
    }));
    StartupSources {
        sheets: Vec::new(),
        entry: StartupSource {
            url: url.to_owned(),
            answer,
        },
    }
}

/// One group's shared runtime, with the test holding the worker thread's end
/// of the factory so a `Start` is observable and no worker ever boots.
fn group(thread: &JsThreadHandle) -> (Rc<GroupContext>, mpsc::UnboundedReceiver<WorkerCommand>) {
    let mut js = ScriptRuntime::new().expect("a QuickJS runtime");
    install_shared_modules(&mut js).expect("the shared modules register");
    let (workers, commands) = mpsc::unbounded_channel();
    let context = GroupContext {
        js: Rc::new(RefCell::new(js)),
        style_pool: None,
        requester: Arc::new(NoWakeup),
        workers: WorkerFactory::new(workers),
        thread: thread.clone(),
    };
    (Rc::new(context), commands)
}

/// What every page here is created at, and what the painter these tests play
/// binds it at unless the test says otherwise.
const CREATE_VIEWPORT: Viewport = Viewport::new(320.0, 240.0);

/// The metrics a published frame was committed at, as bits: these are values
/// copied across a channel rather than computed, so exact equality is the
/// question and `to_bits` is how it is asked.
fn committed_at(frame: &dom::CommittedFrame) -> (u32, u32, u32) {
    (
        frame.viewport().width.to_bits(),
        frame.viewport().height.to_bits(),
        frame.device_pixel_ratio().to_bits(),
    )
}

/// The same three numbers of a viewport, to compare one against.
fn metrics_of(viewport: Viewport) -> (u32, u32, u32) {
    (
        viewport.width.to_bits(),
        viewport.height.to_bits(),
        viewport.device_pixel_ratio.to_bits(),
    )
}

fn ingredients() -> DocumentIngredients {
    DocumentIngredients::for_test(CREATE_VIEWPORT, PageConfig::default())
}

/// One view served by the real owner, with the test on the host's end of its
/// link.
struct Harness {
    workers: mpsc::UnboundedReceiver<WorkerCommand>,
    background: Option<WorkerStart>,
    commands: mpsc::UnboundedSender<ToMain>,
    /// The painter's end of the view's metrics watch. Bound at
    /// [`CREATE_VIEWPORT`] unless the test asked for a view nothing has
    /// bound, which is what [`Harness::unbound`] is for.
    metrics: watch::Sender<Option<Viewport>>,
    view: DetachedView,
    events: Vec<EngineEvent>,
    sources: Vec<(SourceRequest, SourceCompletion)>,
    preloads: Vec<SourceRequest>,
    /// The owner's handle, so a step that never happened because the owner
    /// trapped is reported as that panic rather than as a deadline.
    owner: task::JoinHandle<()>,
}

impl Harness {
    fn new(context: Rc<GroupContext>, workers: mpsc::UnboundedReceiver<WorkerCommand>) -> Self {
        Self::serving(context, workers, ViewSources::new("app:///main.js"))
    }

    /// A view no painter has bound, which is where its first
    /// `__FlushElementTree` parks. The test binds it by writing
    /// [`Harness::metrics`].
    fn unbound(context: Rc<GroupContext>, workers: mpsc::UnboundedReceiver<WorkerCommand>) -> Self {
        Self::binding(context, workers, ViewSources::new("app:///main.js"), None)
    }

    /// A second view in the same group. The group has one worker channel and
    /// the first harness holds it, so this one watches a channel of its own:
    /// the pins that build two views are about which view's *own* work runs,
    /// and neither of them boots a BTS.
    fn sibling(context: Rc<GroupContext>, sources: ViewSources) -> Self {
        Self::serving(context, mpsc::unbounded_channel().1, sources)
    }

    fn serving(
        context: Rc<GroupContext>,
        workers: mpsc::UnboundedReceiver<WorkerCommand>,
        sources: ViewSources,
    ) -> Self {
        Self::binding(context, workers, sources, Some(CREATE_VIEWPORT))
    }

    /// `bound` is what the view's metrics watch starts at: `Some` for a view
    /// a painter is already watching — which is what every pin that is not
    /// about the binding wants — and `None` for one nothing has bound.
    fn binding(
        context: Rc<GroupContext>,
        workers: mpsc::UnboundedReceiver<WorkerCommand>,
        mut sources: ViewSources,
        bound: Option<Viewport>,
    ) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let (commands, incoming) = mpsc::unbounded_channel();
        let (metrics, metric_receiver) = watch::channel(bound);
        // What `create_lynx_view` does on the embedder's thread, which this
        // test is: the startup sources are requested before the view's task
        // exists, so they are outstanding from the first turn and the
        // sheets are answered in whatever order the test likes.
        let mut outstanding = Vec::new();
        let request = |request: SourceRequest, outstanding: &mut Vec<_>| {
            let (completion, answer) = SourceCompletion::new(view.token.clone());
            outstanding.push((request, completion));
            answer
        };
        let sheets = std::mem::take(&mut sources.style_sheets)
            .into_iter()
            .map(|url| StartupSource {
                answer: request(SourceRequest::StyleSheet(url.clone()), &mut outstanding),
                url,
            })
            .collect();
        let entry_url = std::mem::take(&mut sources.entry);
        let entry = StartupSource {
            answer: request(SourceRequest::Entry(entry_url.clone()), &mut outstanding),
            url: entry_url,
        };
        let attached = AttachedView {
            viewport: CREATE_VIEWPORT,
            sources,
            text_context: None,
            startup: StartupSources { sheets, entry },
            native_modules: String::new(),
            commands: incoming,
            metrics: metric_receiver,
            cancel: view.token.clone(),
        };
        let owner = task::spawn_local(serve_view(context, attached, outbox));
        Self {
            workers,
            background: None,
            commands,
            metrics,
            view,
            events: Vec::new(),
            sources: outstanding,
            preloads: Vec::new(),
            owner,
        }
    }

    /// Lets every ready task of the view run, collecting whatever it said.
    async fn turn(&mut self) {
        task::yield_now().await;
        if self.view.token.is_cancelled()
            && let Some(background) = self.background.as_mut()
        {
            while let Ok(message) = background.messages.try_recv() {
                if is_dispose(&message) {
                    acknowledge_disposal(background);
                }
            }
        }
        while let Ok(notice) = self.view.notices.try_recv() {
            match notice {
                ViewNotice::Engine(event) => self.events.push(event),
                ViewNotice::RequestSource {
                    request,
                    completion,
                } => self.sources.push((request, completion)),
                ViewNotice::RequestImages(_)
                | ViewNotice::WorkerCreated { .. }
                | ViewNotice::NativeModuleCall { .. }
                | ViewNotice::ScriptFrameDemand { .. } => {}
                ViewNotice::PreloadSource(request) => self.preloads.push(request),
            }
        }
    }

    /// Turns until `ready` answers, or fails naming what never happened.
    async fn until(&mut self, what: &str, mut ready: impl FnMut(&mut Self) -> bool) {
        for _ in 0..TURNS {
            if ready(self) {
                return;
            }
            self.turn().await;
        }
        // An owner that trapped is why nothing happened, and its payload says
        // more than the deadline this loop ran out of. Awaiting a handle that
        // has already finished answers at once.
        if self.owner.is_finished() {
            match (&mut self.owner).await {
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                _ => {}
            }
        }
        panic!("{what}");
    }

    /// Answers one outstanding source request, whichever it is.
    fn answer(&mut self, url: &str, source: &str) {
        let (_, completion) = self.sources.pop().expect("a source request is outstanding");
        completion.complete(Ok(LoadedSource::Entry {
            source: source.to_owned(),
            url: url.to_owned(),
        }));
    }

    /// Answers one outstanding stylesheet request with author CSS.
    fn answer_style_sheet(&mut self, css: &str) {
        let position = self
            .sources
            .iter()
            .position(|(request, _)| matches!(request, SourceRequest::StyleSheet(_)))
            .expect("a stylesheet request is outstanding");
        let (_, completion) = self.sources.remove(position);
        completion.complete(Ok(LoadedSource::StyleSheet(
            crate::resource::StyleSheetSource::Text(css.to_owned()),
        )));
    }

    /// Whether this view has asked for a stylesheet and not been answered.
    fn wants_a_style_sheet(&self) -> bool {
        self.sources
            .iter()
            .any(|(request, _)| matches!(request, SourceRequest::StyleSheet(_)))
    }

    fn finished(&self) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptFinished))
    }

    /// Answers one outstanding source request with a failure.
    fn refuse(&mut self) {
        let (_, completion) = self.sources.pop().expect("a source request is outstanding");
        completion.complete(Err(unanswered_source().into()));
    }

    /// Fails one outstanding stylesheet request, leaving the entry alone.
    fn refuse_style_sheet(&mut self) {
        let position = self
            .sources
            .iter()
            .position(|(request, _)| matches!(request, SourceRequest::StyleSheet(_)))
            .expect("a stylesheet request is outstanding");
        let (_, completion) = self.sources.remove(position);
        completion.complete(Err(unanswered_source().into()));
    }

    /// Whether the entry request is still unanswered.
    fn wants_its_entry(&self) -> bool {
        self.sources
            .iter()
            .any(|(request, _)| matches!(request, SourceRequest::Entry(_)))
    }

    /// The startup failure this view reported, if it has.
    fn startup_failure(&self) -> Option<String> {
        self.events.iter().find_map(|event| match event {
            EngineEvent::StartupFailed(error) => Some(error.to_string()),
            _ => None,
        })
    }

    /// Boots the view over `entry` and returns the commit its boot published.
    async fn boot(&mut self, entry: &str) -> u64 {
        self.until("the view never asked for its entry", |harness| {
            !harness.sources.is_empty()
        })
        .await;
        self.answer("app:///main.js", entry);
        self.until("MTS never rendered", |h| {
            h.view.published.commit().is_some()
        })
        .await;
        self.background = Some(self.background_worker());
        self.until("the entry never finished", |harness| {
            harness
                .events
                .iter()
                .any(|event| matches!(event, EngineEvent::ScriptFinished))
        })
        .await;
        self.view
            .published
            .commit()
            .expect("boot published a frame")
    }

    /// The `Start` the boot module's BTS `Worker` sent, which nothing on this
    /// test's side ever boots.
    fn background_worker(&mut self) -> WorkerStart {
        if let Some(background) = self.background.take() {
            return background;
        }
        let Some(WorkerCommand::Start(start)) = self.workers.try_recv().ok() else {
            panic!("boot creates the BTS worker")
        };
        start
    }
}

fn is_dispose(message: &WorkerMessage) -> bool {
    matches!(message, WorkerMessage::Post(data)
    if crate::background::wire_matches(
        data,
        r#"(m) => m?.bobcat === "runtime" && m.method === "dispose""#,
    ))
}

fn acknowledge_disposal(background: &WorkerStart) {
    background
        .events
        .send(crate::background::WorkerEvent {
            key: background.key,
            payload: crate::background::WorkerPayload::Message(crate::background::wire_value(
                r#"{bobcat:"runtime",method:"disposed"}"#,
            )),
        })
        .unwrap();
}

async fn answer_disposal(background: &mut WorkerStart) {
    while let Some(message) = background.messages.recv().await {
        if is_dispose(&message) {
            assert!(
                !background.token.is_cancelled(),
                "BTS remains live until its acknowledgement"
            );
            acknowledge_disposal(background);
            return;
        }
        assert!(
            !matches!(message, WorkerMessage::Terminate),
            "Worker terminated before dispose"
        );
    }
    panic!("Worker channel closed before dispose");
}

/// One page over the token that ends it, with the test holding the owner's
/// tail rather than a task running it.
///
/// The pins below that use this are about the owner's own steps — the end it
/// runs after its wait — and nothing is queued for another task in either of
/// them, so the cancel is the only wake there is and running that tail inline
/// is running it where it would have run anyway. A pin about which task the
/// scheduler picks belongs on [`Harness`] instead.
struct OwnedPage {
    page: Rc<Page>,
    view: DetachedView,
    /// The view's end signal, which here the test is the embedder of.
    token: CancellationToken,
    /// Held, not sent on: the command consumer this page spawned is parked on
    /// the other end, and dropping this would close the channel and end the
    /// view by a path neither pin here is about.
    _commands: mpsc::UnboundedSender<ToMain>,
    /// The painter's end of the metrics watch, bound at [`CREATE_VIEWPORT`]
    /// from the start: none of these pins is about the binding, and an
    /// unbound page's first flush would park.
    _metrics: watch::Sender<Option<Viewport>>,
}

impl OwnedPage {
    fn new(context: Rc<GroupContext>) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let token = view.token.clone();
        let (commands, incoming) = mpsc::unbounded_channel();
        let (metrics, metric_receiver) = watch::channel(Some(CREATE_VIEWPORT));
        let page = Page::new(context, outbox, metric_receiver, token.clone());
        page.spawn(consume_commands(Rc::clone(&page), incoming));
        Self {
            page,
            view,
            token,
            _commands: commands,
            _metrics: metrics,
        }
    }

    /// Opens the realm over `entry` and turns until its first frame is
    /// published.
    async fn boot(&mut self, entry: &str) -> u64 {
        open_realm(&self.page, entry, "app:///main.js").await;
        for _ in 0..TURNS {
            if self.view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        self.view.published.commit().expect("the page booted")
    }

    /// Every lifecycle event this page has published so far.
    fn events(&mut self) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        while let Ok(notice) = self.view.notices.try_recv() {
            if let ViewNotice::Engine(event) = notice {
                events.push(event);
            }
        }
        events
    }
}

/// A page that renders one box, which is enough for a resize to have
/// something to lay out again.
const ONE_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  globalThis.box = box;
};
";

/// A page whose entry adopts a stylesheet as it evaluates, which is the one
/// synchronous wait a realm can make.
const ADOPTING_BOX: &str = r"
__AdoptStyleSheet(__LoadStyleSheet('CSS', '__Card__'));
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __AppendElement(page, __CreateView(0));
};
";

/// The same page, with an `updatePage` that writes what the host sent onto
/// the box, so a `PageUpdate` command genuinely dirties the document.
const ONE_BOX_WITH_UPDATE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  globalThis.box = box;
};
globalThis.updatePage = function (data) {
  __SetAttribute(globalThis.box, 'data-value', String(data.value));
};
";

/// One host data update carrying `value`, for the pins that need a command
/// that genuinely changes the document.
fn update_command(value: u8) -> ToMain {
    ToMain::PageUpdate(PageUpdate::Data {
        data: format!(r#"{{"value":{value}}}"#),
        processor_name: String::new(),
        reset: false,
    })
}

/// The same page, with a timer far enough out that nothing will ever fire it,
/// so the deadline a live realm publishes is there to be withdrawn.
const ONE_BOX_WITH_TIMER: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  setTimeout(() => {}, 3600000);
};
";

/// The same page, with a listener on its box so a dispatch into it genuinely
/// enters JavaScript.
const LISTENING_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  __AddEventListener(box, 'tap', () => {});
};
";

/// A page that is nothing but its root, with a JavaScript listener on it for
/// the very event the engine fires Rust-side.
///
/// That listener is the pin for the ruling: `contentvisibilityautostatechange`
/// is delivered to engine components and to nothing else, so a realm
/// registration for it — on an ancestor of every row, in the phase a bubbling
/// event would reach — must never run. It would mark the page element if it
/// did.
const LISTENING_PAGE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;width:200px;height:240px;align-items:flex-start');
  __AddEventListener(page, 'contentvisibilityautostatechange', () => {
    __SetAttribute(page, 'data-js-heard', 'yes');
  }, {});
};
";

/// How many rows [`build_scrolling_rows`] lays out, how tall each is, and how
/// tall the scrollport holding them is — all whole CSS pixels, which is what
/// lets [`admitted_rows`] derive the answer without a float in sight.
const ROWS: usize = 20;
const ROW_HEIGHT: usize = 20;
const SCROLLPORT: usize = 100;

/// The rows the encode window admits at `offset`, derived from the spec of
/// the margin rather than from the implementation: one scrollport past the
/// committed offset in each direction, clamped to the scroll range. Touching
/// the band counts, since the cull test admits whatever it cannot rule out.
fn admitted_rows(offset: usize) -> Vec<usize> {
    let max_offset = ROWS * ROW_HEIGHT - SCROLLPORT;
    let top = offset.saturating_sub(SCROLLPORT);
    let bottom = SCROLLPORT + (offset + SCROLLPORT).min(max_offset);
    (0..ROWS)
        .filter(|&row| {
            let y = ROW_HEIGHT * row;
            y <= bottom && y + ROW_HEIGHT >= top
        })
        .collect()
}

/// Where the components below record what they heard.
type Log = Arc<std::sync::Mutex<Vec<String>>>;

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

/// An engine component that records every event it hears as
/// `who:phase:target`, naming the target by its `id` attribute, and — on the
/// row — writes to the tree from inside the delivery.
struct Recorder {
    who: &'static str,
    log: Log,
}

impl dom::CustomElement<()> for Recorder {
    fn handle_event(
        &self,
        document: &mut dom::Document<()>,
        element: dom::NodeId,
        event: &mut dom::event::ElementEvent,
    ) {
        let dom::event::ElementEventKind::ContentVisibilityAutoStateChange { skipped } =
            event.kind()
        else {
            panic!("an engine event this page never fires: {:?}", event.kind())
        };
        let target = document
            .get(event.target())
            .and_then(|node| node.attribute("id"))
            .expect("every target here is a live row")
            .to_owned();
        let phase = match event.phase() {
            dom::event::EventPhase::Capturing => "capture",
            dom::event::EventPhase::AtTarget => "at-target",
            dom::event::EventPhase::Bubbling => "bubble",
        };
        let state = if skipped { "skipped" } else { "shown" };
        self.log
            .lock()
            .expect("the log is never poisoned")
            .push(format!("{}:{phase}:{target}:{state}", self.who));
        // A handler mutates the tree it was called about: whatever this
        // leaves dirty is the delivery entry's own commit, never the one that
        // decided the change.
        document.set_inline_style_property(element, "opacity", "0.5");
    }
}

/// The three deliveries one row's change owes, in order.
fn whole_path(row: usize, skipped: bool) -> Vec<String> {
    let state = if skipped { "skipped" } else { "shown" };
    vec![
        format!("scroller:capture:row{row}:{state}"),
        format!("row:at-target:row{row}:{state}"),
        format!("scroller:bubble:row{row}:{state}"),
    ]
}

/// Installs the two component definitions and builds the scroller and its
/// rows under the card's page element, as one probe: `Document::define`
/// requires a definition to precede every element with its tag, and the tree
/// is built in the same entry so it does.
///
/// A probe rather than JavaScript because an engine component is not
/// script-reachable at all — there is no PAPI for one, and that is the point.
/// Returns the scroller, which is what a `Refill` has to name.
async fn build_scrolling_rows(page: &Rc<Page>, log: &Log) -> dom::NodeId {
    let (answer, built) = std::sync::mpsc::channel();
    let log = Arc::clone(log);
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        document.define(
            "x-scroller",
            Box::new(Recorder {
                who: "scroller",
                log: Arc::clone(&log),
            }),
        );
        document.define("x-row", Box::new(Recorder { who: "row", log }));

        let root = document.document_element().id();
        let scroller = document.create_element("x-scroller", ());
        document.set_inline_style(
            scroller,
            "display:flex;flex-direction:column;overflow:hidden;\
             width:200px;height:100px;align-items:flex-start",
        );
        document.append_child(root, scroller);
        for index in 0..ROWS {
            let row = document.create_element("x-row", ());
            document.set_inline_style(
                row,
                "display:flex;width:200px;height:20px;flex-shrink:0;\
                 content-visibility:auto;contain-intrinsic-size:200px 20px",
            );
            document.set_id_attribute(row, Some(&format!("row{index}")));
            document.append_child(scroller, row);
        }
        let _ = answer.send(scroller);
    }))])
    .await;
    built.try_recv().expect("the probe ran")
}

/// Whether the realm's listener for the same event name ever ran.
async fn js_heard(page: &Rc<Page>) -> bool {
    let (answer, heard) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let _ = answer.send(
            document
                .document_element()
                .attribute("data-js-heard")
                .is_some(),
        );
    }))])
    .await;
    heard.try_recv().expect("the probe ran")
}

/// css-contain-2 §4.4: the commit that first determines relevance fires a
/// `contentvisibilityautostatechange` at every `auto` element whose skipping
/// state changed — which on a first determination is the on-screen ones
/// alone, since an undetermined box already skips.
///
/// The delivery is an entry of its own, because the spec dispatches the event
/// "by posting a task at the time when the state change occurs": the commit's
/// own entry queues it and runs no handler, so the epilogue count moves twice
/// for one burst.
#[test]
fn a_commit_delivers_its_state_changes_in_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(LISTENING_PAGE).await;
        let log: Log = Arc::default();
        let settled = owned.page.epilogue_count();

        build_scrolling_rows(&owned.page, &log).await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the burst that built the rows was one entry, and the deliveries \
             its commit decided were another",
        );

        let expected: Vec<String> = admitted_rows(0)
            .into_iter()
            .flat_map(|row| whole_path(row, false))
            .collect();
        assert_eq!(
            take(&log),
            expected,
            "every revealed row, in frame order, over the whole path: the \
             scroller inbound, the row itself, the scroller outbound",
        );
        assert!(
            !js_heard(&owned.page).await,
            "the realm's own listener for the same name never runs: this \
             event is fired Rust-side and reaches engine components alone",
        );
    });
}

/// A refill past the encode window re-determines every row, and the two
/// directions are one batch: one entry behind the refill's own, one delivery
/// per row that changed, each exactly once.
#[test]
fn a_refill_delivers_both_directions_in_the_entry_after_its_commit() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(LISTENING_PAGE).await;
        let log: Log = Arc::default();
        let scroller = build_scrolling_rows(&owned.page, &log).await;
        let before = admitted_rows(0);
        assert_eq!(take(&log).len(), before.len() * 3);
        let settled = owned.page.epilogue_count();

        owned
            .page
            .apply(vec![ToMain::Refill {
                offsets: vec![(scroller, dom::Vector2D::new(0.0, 150.0))],
            }])
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the refill was one entry and the deliveries its commit decided \
             were another",
        );

        let after = admitted_rows(150);
        let expected: Vec<String> = (0..ROWS)
            .filter(|row| before.contains(row) != after.contains(row))
            .flat_map(|row| whole_path(row, !after.contains(&row)))
            .collect();
        assert!(
            expected.iter().any(|entry| entry.ends_with("skipped"))
                && expected.iter().any(|entry| entry.ends_with("shown")),
            "the case is only worth anything if both directions happened: {expected:?}",
        );
        assert_eq!(take(&log), expected);
        assert!(!js_heard(&owned.page).await);
    });
}

/// The host's page data rides from the view's sources to its realm as the
/// text it was given, and is parsed there before the entry loads: the entry
/// sees the global props as it evaluates, and `processData` gets the init
/// data. Each side checks its own, so a swap anywhere on the way fails boot.
#[test]
fn page_data_reaches_the_realm_it_was_given_to() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                init_data: Some(r#"{"boxes": 2}"#.to_owned()),
                global_props: Some(r#"{"theme": "dark"}"#.to_owned()),
                ..ViewSources::new("app:///main.js")
            },
        );
        harness
            .boot(
                r"
                if (__globalProps.theme !== 'dark') throw new Error('global props');
                globalThis.processData = function (data) {
                  if (data.boxes !== 2) throw new Error('init data');
                  return data;
                };
                ",
            )
            .await;
    });
}

/// A host's whole round of input is one entry into the realm, so it is one
/// commit and one acknowledgement — not one of each per command.
///
/// The metrics used to be part of such a burst. They are a watch now, so the
/// burst is made of page updates instead; what a metrics change coalesces
/// into is pinned by
/// [`metrics_that_move_before_the_task_wakes_are_one_commit`] below.
#[test]
fn a_burst_of_commands_is_one_commit_and_one_acknowledgement() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX_WITH_UPDATE).await;

        // Every one of these is queued before the consumer wakes, and every
        // one of them genuinely writes a different attribute value, so an
        // entry apiece would be a commit apiece.
        for step in 0..5u8 {
            harness
                .commands
                .send(update_command(step))
                .expect("the view is still serving");
        }
        harness
            .commands
            .send(ToMain::BeginFrame { now: 0.0, seq: 7 })
            .expect("the view is still serving");

        harness
            .until("the burst was never acknowledged", |harness| {
                harness.view.published.begin_frame_serviced() == 7
            })
            .await;
        assert_eq!(
            harness.view.published.commit(),
            Some(booted + 1),
            "the whole burst committed once"
        );
    });
}

/// Nothing is published and MTS boot does not finish until a painter binds:
/// boot's own `__FlushElementTree` commits at the create-time viewport, holds
/// that frame, and parks the job it runs in on the metrics watch.
///
/// Binding at the size the view was created at is the cheap path — the held
/// frame goes out as it is, with no second commit.
#[test]
fn an_unbound_view_holds_its_first_frame_and_publishes_it_on_the_binding() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::unbound(context, workers);
        harness
            .until("the view never asked for its entry", |harness| {
                !harness.sources.is_empty()
            })
            .await;
        harness.answer("app:///main.js", ONE_BOX);

        // Every task of the view goes on running while the flush is parked,
        // so this is as far as an unbound view ever gets.
        for _ in 0..64 {
            harness.turn().await;
        }
        assert_eq!(
            harness.view.published.commit(),
            None,
            "the frame boot committed is held rather than published"
        );
        assert!(!harness.finished(), "and MTS boot has not finished");

        harness.metrics.send_replace(Some(CREATE_VIEWPORT));
        harness
            .until("the binding never published the held frame", |harness| {
                harness.view.published.commit().is_some()
            })
            .await;
        let frame = harness
            .view
            .published
            .frame()
            .expect("a frame is published");
        assert_eq!(committed_at(&frame), metrics_of(CREATE_VIEWPORT));
        harness
            .until("boot never finished", |harness| harness.finished())
            .await;
    });
}

/// Binding at metrics the view was not created at discards the held frame and
/// commits again, which is the resize path: the first frame a painter ever
/// sees is at the painter's own size.
#[test]
fn binding_at_other_metrics_recommits_the_held_frame() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::unbound(context, workers);
        harness
            .until("the view never asked for its entry", |harness| {
                !harness.sources.is_empty()
            })
            .await;
        harness.answer("app:///main.js", ONE_BOX);
        for _ in 0..64 {
            harness.turn().await;
        }
        assert_eq!(harness.view.published.commit(), None);

        let painter = Viewport::new(200.0, 100.0).with_device_pixel_ratio(2.0);
        harness.metrics.send_replace(Some(painter));
        harness
            .until("the binding never published a frame", |harness| {
                harness.view.published.commit().is_some()
            })
            .await;

        let frame = harness
            .view
            .published
            .frame()
            .expect("a frame is published");
        assert_eq!(
            committed_at(&frame),
            metrics_of(painter),
            "the held frame was discarded and recomputed at the painter's size"
        );
        harness
            .until("boot never finished", |harness| harness.finished())
            .await;
    });
}

/// The view's own token is the parked flush's biased first arm, so a release
/// ends the wait: the flush throws, boot fails with it, and the view ends
/// without ever reporting that it started.
#[test]
fn a_view_released_while_its_first_flush_is_parked_never_finishes() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::unbound(context, workers);
        harness
            .until("the view never asked for its entry", |harness| {
                !harness.sources.is_empty()
            })
            .await;
        harness.answer("app:///main.js", ONE_BOX);
        for _ in 0..64 {
            harness.turn().await;
        }
        // Taken before the release so the disposal exchange has somewhere to
        // answer; boot creates the BTS Worker well before its flush.
        harness.background = Some(harness.background_worker());
        assert!(!harness.finished());

        harness.view.token.cancel();
        harness
            .until("the view never ended", |harness| {
                harness.owner.is_finished()
            })
            .await;

        assert_eq!(
            harness.view.published.commit(),
            None,
            "the held frame went with the view"
        );
        assert!(
            !harness.finished(),
            "and a view that ended mid-flush reports no boot"
        );
    });
}

/// The painter's metrics are a watch, not a command: five moves before the
/// consuming task wakes are one value and so one commit.
///
/// That is the whole of what the new mechanism guarantees here. A watch keeps
/// the latest rather than a queue, so how many writes a settle stands for is
/// not something a caller can count on — only that the document ends up at
/// the last of them, in one commit if nothing else entered the realm between.
#[test]
fn metrics_that_move_before_the_task_wakes_are_one_commit() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX).await;

        for step in 0..5u8 {
            harness.metrics.send_replace(Some(
                Viewport::new(320.0 - f32::from(step), 240.0 + f32::from(step))
                    .with_device_pixel_ratio(1.0),
            ));
        }

        harness
            .until("the metrics never reached the document", |harness| {
                harness.view.published.commit() == Some(booted + 1)
            })
            .await;
        let frame = harness
            .view
            .published
            .frame()
            .expect("a frame is published");
        assert_eq!(
            committed_at(&frame),
            metrics_of(Viewport::new(316.0, 244.0)),
            "and it was committed at the last of the five"
        );

        // Nothing else is owed: a second settle would be a second commit.
        for _ in 0..8 {
            harness.turn().await;
        }
        assert_eq!(harness.view.published.commit(), Some(booted + 1));
    });
}

/// A module completion is a task of its own, so what its continuation changed
/// is committed without a command to carry it.
#[test]
fn data_updates_are_visible_to_the_next_command_and_commit_without_an_explicit_flush() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        let booted = owned.boot(&format!(
            "{ONE_BOX}\nglobalThis.updatePage = data => __SetAttribute(box, 'data-value', String(data.value));"
        )).await;
        owned
            .page
            .apply(vec![
                ToMain::PageUpdate(crate::link::PageUpdate::Data {
                    data: r#"{"value":7}"#.into(),
                    processor_name: String::new(),
                    reset: false,
                }),
                ToMain::Probe(Box::new(|document| {
                    let node = document.document_element().children().next().unwrap();
                    assert_eq!(node.attribute("data-value"), Some("7"));
                })),
            ])
            .await;
        assert_eq!(owned.view.published.commit(), Some(booted + 1));
        assert!(!owned.page.ended());
    });
}

/// One image with a `bindload` that records what it was handed, and an
/// `updatePage` that writes whichever source the host names.
///
/// The handler is a worklet because that is the only `__AddEvent` kind this
/// realm runs — a string handler is published to the background thread, which
/// no test here has.
const LOADING_IMAGE: &str = r"
globalThis.runWorklet = (value, params) => value.body(params[0]);
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const image = __CreateImage(0);
  __AppendElement(page, image);
  globalThis.image = image;
  globalThis.loads = 0;
  __AddEvent(image, 'bindEvent', 'load', {
    type: 'worklet',
    value: {
      body: (event) => {
        globalThis.loads += 1;
        __SetAttribute(image, 'data-loaded', event.detail.width + 'x' + event.detail.height);
        __SetAttribute(image, 'data-loads', String(globalThis.loads));
      },
    },
  });
};
globalThis.updatePage = data => __SetAttribute(image, 'src', String(data.src));
";

/// The image's own attributes, read off the document.
///
/// A probe is an entry of the page's own, so this is what every other
/// observation here is: one command, applied and awaited.
async fn image_attributes(page: &Rc<Page>, name: &'static str) -> Option<String> {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let root = document.document_element().id();
        let image = document.get(root).expect("the page is live").child_ids()[0];
        let _ = answer.send(
            document
                .get(image)
                .and_then(|node| node.attribute(name))
                .map(str::to_owned),
        );
    }))])
    .await;
    read.try_recv().expect("the probe ran")
}

/// Both producers of an image event, end to end through the real page: a
/// report from the painting side, and a `src` that settles at the bind
/// because this document has already seen that URL.
///
/// Either way the delivery is an entry of its own, the way a browser fires an
/// `<img>`'s `load` from a task even for a cached URL, so the epilogue count
/// moves twice for one report: the entry that settled the source, and the
/// entry that delivered what it settled.
#[test]
fn an_image_load_reaches_its_listener_in_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        let booted = owned.boot(LOADING_IMAGE).await;

        // Neither source is on the element yet, so the second report settles
        // the registry alone — nothing owes an event for it, and the entry
        // that applied it posts none.
        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![ToMain::ImageEvents(vec![dom::ImageEvent::Loaded {
                source: Arc::from("app:///b.png"),
                width: 10,
                height: 5,
            }])])
            .await;
        assert_eq!(owned.page.epilogue_count(), settled + 1);
        assert_eq!(image_attributes(&owned.page, "data-loads").await, None);

        // A source that has not loaded, then the report that settles it.
        owned
            .page
            .apply(vec![ToMain::PageUpdate(PageUpdate::Data {
                data: r#"{"src":"app:///a.png"}"#.into(),
                processor_name: String::new(),
                reset: false,
            })])
            .await;
        assert_eq!(image_attributes(&owned.page, "data-loads").await, None);
        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![ToMain::ImageEvents(vec![dom::ImageEvent::Loaded {
                source: Arc::from("app:///a.png"),
                width: 40,
                height: 20,
            }])])
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the report was one entry, and the `load` it settled was another",
        );
        assert_eq!(
            image_attributes(&owned.page, "data-loaded").await,
            Some("40x20".to_owned())
        );

        // The other producer: binding a URL this document has already settled
        // answers at the bind, inside the `__SetAttribute` that wrote it, and
        // is delivered by an entry of its own rather than from inside it.
        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![ToMain::PageUpdate(PageUpdate::Data {
                data: r#"{"src":"app:///b.png"}"#.into(),
                processor_name: String::new(),
                reset: false,
            })])
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the `__SetAttribute` was one entry, and its own answer another",
        );
        assert_eq!(
            image_attributes(&owned.page, "data-loaded").await,
            Some("10x5".to_owned()),
        );
        assert_eq!(
            image_attributes(&owned.page, "data-loads").await,
            Some("2".to_owned()),
            "one event per source that settled, and no repeat of the first",
        );
        assert!(owned.view.published.commit() > Some(booted));
    });
}

/// An event a listener queues during the drain is a batch of its own, and
/// owes an entry of its own: the latch is cleared before the walk, so the
/// delivery entry's epilogue posts one more for what its own handler bound.
#[test]
fn an_image_event_queued_by_a_listener_is_delivered_by_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        // The handler writes the *other* settled source, so delivering the
        // first load queues a second one from inside the drain.
        owned
            .boot(&LOADING_IMAGE.replace(
                "__SetAttribute(image, 'data-loads', String(globalThis.loads));",
                "__SetAttribute(image, 'data-loads', String(globalThis.loads));
        if (globalThis.loads === 1) __SetAttribute(image, 'src', 'app:///b.png');",
            ))
            .await;
        owned
            .page
            .apply(vec![ToMain::ImageEvents(vec![dom::ImageEvent::Loaded {
                source: Arc::from("app:///b.png"),
                width: 10,
                height: 5,
            }])])
            .await;

        owned
            .page
            .apply(vec![ToMain::PageUpdate(PageUpdate::Data {
                data: r#"{"src":"app:///a.png"}"#.into(),
                processor_name: String::new(),
                reset: false,
            })])
            .await;
        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![ToMain::ImageEvents(vec![dom::ImageEvent::Loaded {
                source: Arc::from("app:///a.png"),
                width: 40,
                height: 20,
            }])])
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 3,
            "the report, the `load` it settled, and the `load` that \
             delivery's own handler bound",
        );
        assert_eq!(
            image_attributes(&owned.page, "data-loads").await,
            Some("2".to_owned()),
        );
        assert_eq!(
            image_attributes(&owned.page, "data-loaded").await,
            Some("10x5".to_owned()),
            "the entry that followed delivered what the listener queued",
        );
    });
}

#[test]
fn a_module_completion_commits_with_no_command_behind_it() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness
            .boot(&format!(
                "{ONE_BOX}\nimport('app:///dep.js').then(() => __SetAttribute(globalThis.box, 'loaded', 'yes'));"
            ))
            .await;
        harness
            .until("the entry's import was never requested", |harness| {
                harness
                    .sources
                    .iter()
                    .any(|(request, _)| matches!(request, SourceRequest::Module(url) if url == "app:///dep.js"))
            })
            .await;
        assert_eq!(
            harness.view.published.commit(),
            Some(booted),
            "nothing has committed since boot"
        );

        harness.answer("app:///dep.js", "export const value = 1;");
        harness
            .until("the import's continuation never committed", |harness| {
                harness.view.published.commit() == Some(booted + 1)
            })
            .await;
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, EngineEvent::ScriptFinished))
                .count(),
            1,
            "and it is not a second boot"
        );
    });
}

/// An import that cannot be completed is fatal to whatever awaited it: it is
/// reported once, and the end reaches every task of the view — the owner
/// returns, which closes the command channel, and the workers the view
/// created are told to stop. A command that arrives behind the end is dropped
/// whole, `BeginFrame` included.
#[test]
fn a_fatal_module_failure_ends_every_task_of_the_view() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness
            .boot(&format!("{ONE_BOX}\nimport('app:///dep.js');"))
            .await;
        let mut background = harness.background_worker();
        assert!(
            matches!(background.messages.try_recv(), Ok(WorkerMessage::Post(_))),
            "boot supplies BTS initial data through its first Worker message"
        );
        harness
            .until("the entry's import was never requested", |harness| {
                !harness.sources.is_empty()
            })
            .await;

        harness.refuse();
        harness
            .until("the failed import was never reported", |harness| {
                harness
                    .events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::ScriptRunError(_)))
            })
            .await;
        // Exactly one command behind the end, which has nowhere to go: the end
        // acknowledged whatever was pending, and what arrives after it is
        // dropped rather than served. A send the closing has already overtaken
        // says the same thing.
        let _ = harness
            .commands
            .send(ToMain::BeginFrame { now: 0.0, seq: 9 });
        answer_disposal(&mut background).await;
        harness
            .until("the view's owner never returned", |harness| {
                harness.owner.is_finished()
            })
            .await;

        assert_ne!(
            harness.view.published.begin_frame_serviced(),
            9,
            "the command behind the end was never served"
        );
        assert_eq!(
            harness.view.published.commit(),
            Some(booted),
            "and nothing committed after the failure"
        );
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, EngineEvent::ScriptRunError(_)))
                .count(),
            1,
            "one failure is reported once"
        );
        assert!(
            !harness
                .events
                .iter()
                .skip_while(|event| !matches!(event, EngineEvent::ScriptRunError(_)))
                .any(|event| matches!(event, EngineEvent::ScriptFinished)),
            "and nothing claims the entry finished after it"
        );
        assert!(
            matches!(background.messages.try_recv(), Ok(WorkerMessage::Terminate)),
            "the realm going takes the workers it created with it"
        );
    });
}

/// A sibling's task ending announces a checkpoint nobody ran: the job queue
/// it left is this page's too, so the page settles what its own realm owes
/// rather than staying parked.
///
/// Built over the page directly rather than over [`serve_view`], because what
/// it counts — that the epilogue ran at all — is not something a page with
/// nothing to publish says out loud.
#[test]
fn a_siblings_checkpoint_makes_a_parked_page_settle() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let page = Page::new(
            Rc::clone(&context),
            outbox,
            bound_metrics(CREATE_VIEWPORT),
            view.token.clone(),
        );
        open_realm(&page, ONE_BOX, "app:///main.js").await;
        for _ in 0..TURNS {
            if view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        assert!(view.published.commit().is_some(), "the page booted");
        assert_eq!(
            page.task_count(),
            2,
            "a live realm waits on its workers, and on its clock — its deadline and the \
             runtime's checkpoints are one task"
        );

        // Parked: nothing of this page's own is running, and its own entries
        // are not what the clock task's checkpoint arm is watching for.
        for _ in 0..8 {
            task::yield_now().await;
        }
        let settled = page.epilogue_count();

        // Exactly what `finish_view` does when a sibling's task ends.
        context.js.borrow().mark_checkpoint();
        for _ in 0..TURNS {
            if page.epilogue_count() > settled {
                break;
            }
            task::yield_now().await;
        }
        assert_eq!(
            page.epilogue_count(),
            settled + 1,
            "the bump nobody's entry produced is the one that wakes this page"
        );
    });
}

/// A page's own entries do not wake its clock task: the generation it records
/// at the end of every entry is what tells its own bumps from a sibling's.
#[test]
fn a_pages_own_entries_never_wake_its_clock_task() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let page = Page::new(
            Rc::clone(&context),
            outbox,
            bound_metrics(CREATE_VIEWPORT),
            view.token.clone(),
        );
        // The listener is what makes the dispatch below a real entry into
        // JavaScript rather than a walk that meets nobody.
        open_realm(&page, LISTENING_BOX, "app:///main.js").await;
        for _ in 0..TURNS {
            if view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        // A boot that failed says why, which is a better report than the
        // assertion below and than every step after it.
        while let Ok(notice) = view.notices.try_recv() {
            if let ViewNotice::Engine(EngineEvent::StartupFailed(error)) = notice {
                panic!("the page never booted: {error}");
            }
        }
        assert!(view.published.commit().is_some(), "the page booted");

        // The probe is how the test learns which node to aim the dispatch at.
        let (target, listening) = std::sync::mpsc::channel();
        page.apply(vec![ToMain::Probe(Box::new(move |document| {
            let page = document.document_element().id();
            let box_id = document.get(page).expect("the page is live").child_ids()[0];
            let _ = target.send(box_id);
        }))])
        .await;
        let target = listening.try_recv().expect("the probe ran");
        for _ in 0..64 {
            task::yield_now().await;
        }
        let settled = page.epilogue_count();

        // One entry of this page's own, which enters JavaScript and so bumps
        // the generation every realm on the runtime shares.
        page.apply(vec![ToMain::DispatchEvent {
            target,
            name: "tap",
            payload: InputEventPayload::default(),
        }])
        .await;
        for _ in 0..64 {
            task::yield_now().await;
        }
        assert_eq!(
            page.epilogue_count(),
            settled + 1,
            "the entry ran one epilogue, and its own checkpoint woke nothing"
        );
    });
}

/// A panic in one task of a view ends the view during the unwind, before any
/// other task of it runs: the guard the page wraps every task in is what does
/// it, and the owner — which is woken by the same cancellation and therefore
/// polled behind the sibling — is what reports the payload.
///
/// The seam spawns the panicking task first and the recorder behind it, so
/// what the recorder saw is what a sibling polled between the unwind and the
/// owner's turn would have seen.
#[test]
fn a_task_that_traps_ends_the_view_before_a_sibling_runs() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness.boot(ONE_BOX).await;

        let (latch, seen) = std::sync::mpsc::channel();
        harness
            .commands
            .send(ToMain::Trap(latch))
            .expect("the view is still serving");
        harness
            .until("the view's owner never returned", |harness| {
                harness.owner.is_finished()
            })
            .await;
        harness.turn().await;

        assert_eq!(
            seen.try_recv(),
            Ok(true),
            "the sibling polled during the unwind found a view that had already ended"
        );
        let reports: Vec<_> = harness
            .events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ScriptRunError(error) => Some(error.message.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(reports.len(), 1, "one panic is reported once: {reports:?}");
        assert!(
            reports[0].contains("the Lynx main thread panicked")
                && reports[0].contains("a task of the view trapped"),
            "the report carries the payload: {}",
            reports[0]
        );
    });
}

/// The end acknowledges whatever `BeginFrame` was applied and not yet
/// answered, and withdraws the deadline the realm had armed — both of them on
/// the owner's own turn, because a release cancels the token from another
/// thread and nothing on this one has run since.
///
/// A painter blocked on that sequence number is waiting for a frame that will
/// never come, so releasing it is the last thing the view owes it.
#[test]
fn the_end_acknowledges_the_begin_frame_a_painter_is_blocked_on() {
    on_a_js_thread(|thread| async move {
        let (context, mut workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX_WITH_TIMER).await;
        assert!(
            owned.page.armed_deadline().is_some(),
            "the booted realm armed a timer"
        );
        owned.page.arm_begin_frame_for_test(11);

        // The embedder's release, with nothing else touched: the command
        // channel stays open, so the owner's wait is the token alone.
        let Some(WorkerCommand::Start(mut background)) = workers.recv().await else {
            panic!("BTS starts")
        };
        owned.token.cancel();
        tokio::join!(owned.page.run_owner(), answer_disposal(&mut background));

        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            11,
            "the end acknowledged the pending BeginFrame"
        );
        assert!(
            owned.page.armed_deadline().is_none(),
            "and withdrew the deadline the realm had armed"
        );
    });
}

/// A burst queued behind the embedder's release is discarded rather than
/// applied: the consumer reads the token once per burst, at the wake
/// boundary, and a token already cancelled there ends the view instead of
/// serving what is queued.
///
/// The order this pins is the order production has, which is why it runs over
/// the real [`serve_view`] rather than the owner's tail alone: the command is
/// sent first, so it wakes the consumer, and the cancel that follows wakes the
/// owner *behind* it. The consumer is what runs first, so nothing here can
/// rest on the owner being woken first.
///
/// The channel is left open, which is also the shape a fatal event's cancel in
/// `LynxView::pump` leaves.
#[test]
fn a_burst_queued_behind_a_release_is_never_applied() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX_WITH_UPDATE).await;

        // Genuinely dirtying: the same command applied on its own is what the
        // burst pin above counts a commit for.
        harness
            .commands
            .send(update_command(1))
            .expect("the view is still serving");
        harness.view.token.cancel();
        harness
            .until("the view never ended", |harness| {
                harness.owner.is_finished()
            })
            .await;

        assert_eq!(
            harness.view.published.commit(),
            Some(booted),
            "the queued command never reached the realm"
        );
    });
}

/// A panic is reported whatever the view was told before it. The report-once
/// latch a startup failure spends is not the one a panic goes through: the
/// lifetime holds a latch of its own for the payload-bearing report, so a view
/// that failed and then trapped says both.
#[test]
fn a_view_that_already_failed_still_reports_a_task_that_traps() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .page
            .fail(EngineEvent::StartupFailed(unanswered_source().into()));
        owned
            .page
            .spawn(async { panic!("a task of the view trapped") });
        // Let the panicking task run: the owner aborts what has not been
        // polled, so a task that never ran is a task that never trapped.
        for _ in 0..8 {
            task::yield_now().await;
        }

        owned.page.run_owner().await;

        let events = owned.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::StartupFailed(_)))
                .count(),
            1,
            "the startup failure is reported once"
        );
        let reports: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ScriptRunError(error) => Some(error.message.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            reports.len(),
            1,
            "and the panic behind it is reported once too: {reports:?}"
        );
        assert!(
            reports[0].contains("a task of the view trapped"),
            "carrying the payload: {}",
            reports[0]
        );
    });
}

#[test]
fn script_finished_is_published_once_without_any_bts_acknowledgement() {
    // Boot is the MTS entry's: the module evaluated and its first flush
    // committed. The BTS Worker here is taken and never booted, so it says
    // nothing at all — and the view is ready anyway, exactly once.
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut sources = ViewSources::new("app:///main.js");
        sources.background_entry = Some("app:///background.js".into());
        let mut harness = Harness::serving(context, workers, sources);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer("app:///main.js", "__CreatePage();");
        harness
            .until("MTS did not render", |h| {
                h.view.published.commit().is_some()
            })
            .await;
        let mut background = harness.background_worker();
        harness
            .until("MTS boot did not finish on its own", |h| {
                h.events
                    .iter()
                    .any(|e| matches!(e, EngineEvent::ScriptFinished))
            })
            .await;
        // And nothing later adds a second one.
        for _ in 0..4 {
            harness.turn().await;
        }
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|e| matches!(e, EngineEvent::ScriptFinished))
                .count(),
            1
        );
        harness.view.token.cancel();
        answer_disposal(&mut background).await;
        harness.owner.await.unwrap();
    });
}

#[test]
fn disposal_finishes_behind_a_job_heavy_worker_event() {
    // A checkpoint runs the job queue dry however long it is. When it did
    // not, the jobs one worker event queued were finished out of the *next*
    // event's entry — and the reply disposal was waiting for was spent
    // there instead of reaching JavaScript, so the view never ended.
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer(
            "app:///main.js",
            r"__CreatePage();
lynx.getJSContext().addEventListener('flood', () => {
    for (let i = 0; i < 4096; i++) Promise.resolve().then(() => {});
});",
        );
        harness
            .until("MTS did not render", |h| {
                h.view.published.commit().is_some()
            })
            .await;
        // Held here rather than in `harness.background`, so the harness's own
        // turns do not acknowledge disposal before the flood is queued behind
        // the `dispose` post.
        let mut background = harness.background_worker();
        harness
            .until("MTS boot did not finish", |h| {
                h.events
                    .iter()
                    .any(|e| matches!(e, EngineEvent::ScriptFinished))
            })
            .await;

        harness.view.token.cancel();
        {
            let background = &mut background.messages;
            let mut posted = false;
            harness
                .until("the BTS was never asked to dispose", |_| {
                    while let Ok(message) = background.try_recv() {
                        posted |= is_dispose(&message);
                    }
                    posted
                })
                .await;
        }
        // One event whose listener floods the job queue, then the reply the
        // owner is waiting for. Both are due in this order on the one worker
        // event stream.
        background
            .events
            .send(crate::background::WorkerEvent {
                key: background.key,
                payload: crate::background::WorkerPayload::Message(crate::background::wire_value(
                    r#"{type:"flood",data:null,origin:"JSContext"}"#,
                )),
            })
            .unwrap();
        acknowledge_disposal(&background);
        harness
            .until("the owner never ended", |h| h.owner.is_finished())
            .await;
        harness.owner.await.unwrap();
    });
}

#[test]
fn a_bts_worker_that_fails_reports_worker_failed_and_leaves_boot_alone() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut sources = ViewSources::new("app:///main.js");
        sources.background_entry = Some("app:///background.js".into());
        let mut harness = Harness::serving(context, workers, sources);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer("app:///main.js", "__CreatePage();");
        harness
            .until("MTS did not render", |h| {
                h.view.published.commit().is_some()
            })
            .await;
        let background = harness.background_worker();
        // Boot is already settled when the failure arrives: it is the MTS
        // entry's own, and the BTS Worker is no part of it.
        harness
            .until("MTS boot did not finish", |h| {
                h.events
                    .iter()
                    .any(|e| matches!(e, EngineEvent::ScriptFinished))
            })
            .await;
        background
            .events
            .send(crate::background::WorkerEvent {
                key: background.key,
                payload: crate::background::WorkerPayload::Failed(crate::script::ScriptError {
                    kind: crate::script::ScriptErrorKind::Exception,
                    phase: crate::script::ScriptErrorPhase::ExecuteModule,
                    message: "BTS startup failed".into(),
                    location: None,
                }),
            })
            .unwrap();
        harness
            .until("the BTS failure was never reported", |h| {
                h.events
                    .iter()
                    .any(|e| matches!(e, EngineEvent::WorkerFailed(_)))
            })
            .await;
        assert!(
            !harness.view.token.is_cancelled(),
            "no BTS failure ends the view"
        );
        let failures: Vec<_> = harness
            .events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::WorkerFailed(error) => Some(error.to_string()),
                EngineEvent::StartupFailed(_)
                | EngineEvent::ListenerFailed(_)
                | EngineEvent::ScriptRunError(_) => {
                    panic!("BTS startup failure was misreported: {event:?}")
                }
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("BTS startup failed"));
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|e| matches!(e, EngineEvent::ScriptFinished))
                .count(),
            1
        );
        // The failed Worker was forgotten when MTS heard `__bobcat:close`, so
        // disposal has no acknowledgement to wait for.
        drop(background);
        harness.view.token.cancel();
        for _ in 0..TURNS {
            if harness.owner.is_finished() {
                break;
            }
            harness.turn().await;
        }
        harness.owner.await.unwrap();
    });
}

#[test]
fn card_url_uses_the_entry_response_url_before_requesting_styles() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("the entry was not requested", |h| h.sources.len() == 1)
            .await;
        harness.answer(
            "https://cdn.test/redirected/main.js?version=2#entry",
            r"
            if (__Card__ !== 'https://cdn.test/redirected/main.js?version=2#entry')
                throw Error('entry URL was not supplied by the resource loader');
            __LoadStyleSheet('CSS', '__Card__');
        ",
        );
        harness
            .until("the stylesheet was not requested", |h| {
                h.preloads.len() == 1
            })
            .await;
        let request = harness.preloads.pop().unwrap();
        assert!(matches!(request, SourceRequest::StyleSheet(ref url)
            if url == "https://cdn.test/redirected/main.js/index.css?version=2#entry"));
    });
}

#[test]
fn view_release_waits_for_js_dispose_before_terminating_its_bts() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness.boot(ONE_BOX).await;
        let mut background = harness.background_worker();
        harness.view.token.cancel();
        for _ in 0..8 {
            harness.turn().await;
        }
        assert!(
            !harness.owner.is_finished(),
            "MTS remains alive for the disposal reply"
        );
        assert!(
            !background.token.is_cancelled(),
            "host release does not cancel BTS"
        );
        answer_disposal(&mut background).await;
        harness
            .until("JS disposal never finished", |h| h.owner.is_finished())
            .await;
        assert!(
            matches!(background.messages.try_recv(), Ok(WorkerMessage::Terminate)),
            "MTS JavaScript terminates only after the reply"
        );
        harness.owner.await.unwrap();
    });
}

#[test]
fn a_disposal_reply_queued_with_view_release_is_not_discarded() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness.boot(&format!(
            "{ONE_BOX}\nglobalThis.updatePage = () => lynx.getEngine().dispatchEvent({{type:'__DestroyLifetime'}});"
        )).await;
        let mut background = harness.background_worker();
        harness
            .commands
            .send(ToMain::PageUpdate(crate::link::PageUpdate::Data {
                data: "{}".into(),
                processor_name: String::new(),
                reset: false,
            }))
            .unwrap();
        let mut requested = false;
        for _ in 0..TURNS {
            while let Ok(message) = background.messages.try_recv() {
                if is_dispose(&message) {
                    requested = true;
                }
            }
            if requested {
                break;
            }
            harness.turn().await;
        }
        assert!(requested, "the explicit engine event starts JS disposal");
        // Both wakes are ready before the normal event consumer runs again.
        // Its cancellation must leave the reply in the inbox for the owner.
        harness.view.token.cancel();
        acknowledge_disposal(&background);
        harness
            .until("the disposal reply was discarded at release", |h| {
                h.owner.is_finished()
            })
            .await;
        assert!(matches!(
            background.messages.try_recv(),
            Ok(WorkerMessage::Terminate)
        ));
        harness.owner.await.unwrap();
    });
}

/// An entry the fetcher has not answered parks nothing.
///
/// `entryUrl()` answers the boot module a `bobcat:future` instead of a URL
/// where the entry is still outstanding, and boot awaits it: the answer is
/// read on a task of this view's owner, and the job boot ran in has already
/// returned. So this view's realm is live with a document in it, its
/// `BeginFrame` is acknowledged by a job of its own, and the entry arriving
/// later is what finishes the boot.
#[test]
fn an_outstanding_entry_leaves_the_view_serving() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(Rc::clone(&context), workers);
        harness
            .until("the view never asked for its entry", |h| {
                !h.sources.is_empty()
            })
            .await;
        harness
            .commands
            .send(ToMain::BeginFrame { now: 0.0, seq: 7 })
            .expect("the view is serving");
        harness
            .until(
                "the acknowledgement never came while the entry was in flight",
                |h| h.view.published.begin_frame_serviced() == 7,
            )
            .await;
        // The realm is open and holds a document: only a live realm answers a
        // probe, and only a document answers `document_element`.
        let (probe, probed) = std::sync::mpsc::channel();
        harness
            .commands
            .send(ToMain::Probe(Box::new(move |document| {
                let _ = probe.send(document.document_element().id());
            })))
            .expect("the view is serving");
        let mut answered = None;
        harness
            .until("the realm never answered a probe", |h| {
                answered = probed.try_recv().ok();
                answered.is_some() || h.owner.is_finished()
            })
            .await;
        assert!(
            answered.is_some(),
            "the realm's document answered the probe"
        );
        assert!(!harness.finished(), "and boot has not finished either");

        harness.answer("app:///main.js", ONE_BOX);
        harness
            .until("boot never finished once its entry arrived", |h| {
                h.finished()
            })
            .await;
        harness.background = Some(harness.background_worker());
    });
}

/// The realm opens and the document is created before any source has
/// arrived: the boot module's first statement is what builds it, and the
/// sheets it mounts are answers the view was promised rather than answers it
/// has.
///
/// Observed through the one thing visible from outside while the entry is
/// still outstanding — a stylesheet that fails to load. Mounting is
/// `createDocument`'s, so a failure that arrives with the entry request still
/// unanswered says the document was being built before the entry was read.
/// The message is the whole of what an embedder is told about it, so it has
/// to name the sheet.
#[test]
fn the_document_is_created_before_the_entry_is_read() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                style_sheets: vec!["app:///a.css".to_owned()],
                ..ViewSources::new("app:///main.js")
            },
        );
        harness
            .until("the view never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        harness.refuse_style_sheet();
        harness
            .until("the sheet failure never reached the embedder", |h| {
                h.startup_failure().is_some()
            })
            .await;
        let message = harness.startup_failure().expect("a startup failure");
        assert!(message.contains("app:///a.css"), "{message}");
        assert!(
            harness.wants_its_entry(),
            "and the entry was never answered, so nothing waited for it first"
        );
    });
}

/// What a synchronous adoption stops and what it does not.
///
/// View A's entry adopts a stylesheet and the test withholds the answer, so
/// A's job is parked inside [`crate::jobs::JsThread::wait`] for the whole of
/// this test's middle. While it is:
///
/// - the scheduler keeps running, so a second view B attaches, its own tasks start and its startup
///   sources are answered — the routing a load needs is a task's, not a job's;
/// - nothing of B's reaches its realm: B's own `open_realm` is a job, and jobs are one FIFO, so B
///   has no document and its `BeginFrame` is not acknowledged either. That is the cost of the
///   loading phase having gone: an acknowledgement now waits for the queue, and what bounds it is
///   that the only wait boot itself makes is for sources the view already asked for;
/// - once A's answer arrives the queue drains in order, B opens its realm, boots, and answers the
///   `BeginFrame` that was queued behind it.
#[test]
fn a_synchronous_adoption_parks_javascript_and_leaves_a_siblings_tasks_running() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut adopting = Harness::new(Rc::clone(&context), workers);
        adopting
            .until("A never asked for its entry", |h| !h.sources.is_empty())
            .await;
        adopting.answer("app:///main.js", ADOPTING_BOX);
        adopting
            .until("A never adopted a stylesheet", |h| h.wants_a_style_sheet())
            .await;

        // A's job is parked from here to the answer below. Everything that
        // follows happens while it is.
        let mut loading = Harness::sibling(
            context,
            ViewSources {
                style_sheets: vec!["app:///b.css".to_owned()],
                ..ViewSources::new("app:///b.js")
            },
        );
        // Both of B's startup requests are outstanding from its construction,
        // the way `create_lynx_view` issues them, so both can be answered
        // while A holds the thread.
        loading
            .until("B never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        loading.answer_style_sheet(".box{width:10px}");
        loading.answer("app:///b.js", ONE_BOX);
        loading
            .commands
            .send(ToMain::BeginFrame { now: 0.0, seq: 4 })
            .expect("B is still serving");
        for _ in 0..8 {
            loading.turn().await;
        }
        assert_eq!(
            loading.view.published.begin_frame_serviced(),
            0,
            "B's acknowledgement is a job, and A's parked job is ahead of it"
        );
        assert!(
            !loading.finished() && loading.view.published.commit().is_none(),
            "no JavaScript of B's ran while A's job held the thread"
        );
        assert!(
            !adopting.finished(),
            "and A's own boot is still inside the adoption"
        );

        adopting.answer_style_sheet(".box{width:20px}");
        adopting
            .until("A never finished after its adoption returned", |h| {
                h.finished()
            })
            .await;
        loading
            .until("B never booted once the queue drained", |h| h.finished())
            .await;
        loading
            .until("B's BeginFrame was never acknowledged", |h| {
                h.view.published.begin_frame_serviced() == 4
            })
            .await;
    });
}

/// Releasing a view whose job is parked in an adoption ends the wait, and the
/// realm is released afterwards without a panic.
///
/// The wait's `select!` is biased on this view's own token, so the cancel wins
/// over an answer that never comes; the message it throws with is pinned in
/// `runtime/tests.rs`. What this pins is the rest: the owner runs its tail
/// — the disposal exchange and the release job — behind the job that was
/// parked, so nothing re-borrows a realm that is still under a JavaScript
/// stack.
#[test]
fn releasing_a_view_whose_job_is_waiting_ends_the_wait_and_releases_its_realm() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("the view never asked for its entry", |h| {
                !h.sources.is_empty()
            })
            .await;
        harness.answer("app:///main.js", ADOPTING_BOX);
        harness
            .until("the entry never adopted a stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;

        harness.view.token.cancel();
        harness
            .until("the released view's owner never returned", |h| {
                h.owner.is_finished()
            })
            .await;
        // A panic in any job of the view — a `BorrowMutError` from a release
        // landing on a realm still under a JavaScript stack, above all — would
        // come back here.
        (&mut harness.owner).await.expect("the owner did not trap");

        let (_, withheld) = harness
            .sources
            .pop()
            .expect("the stylesheet request the test never answered");
        assert!(
            withheld.is_cancelled(),
            "the host sees the request cancelled without waiting for a turn"
        );
        assert!(
            !harness.events.iter().any(|event| matches!(
                event,
                EngineEvent::StartupFailed(_) | EngineEvent::ScriptRunError(_)
            )),
            "a release is not a failure, so nothing is reported"
        );
    });
}

/// A clock task whose deadline passed while another view's job holds the
/// thread queues **one** settle and then waits for it.
///
/// It awaits the settle because the deadline its epilogue republishes is not
/// visible until that job has run: a turn that looped without waiting would
/// re-read the deadline it just passed, re-arm an already-expired sleep, and go
/// on queueing settles for as long as the other view stayed parked. What this
/// pins is that the queue does not grow with how long the park lasts — the
/// second reading is taken after twice the wait of the first.
#[test]
fn a_passed_deadline_queues_one_settle_while_a_sibling_holds_the_thread() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        // The timer view boots first, because a boot is itself a job. It is an
        // `OwnedPage` rather than a `Harness` because what this reads — the
        // armed deadline, and that the epilogue ran at all — is not something a
        // page with nothing to publish says out loud.
        //
        // A repeat rather than a one-shot, so that however long this view's own
        // boot took, a deadline is always at most one period away when the
        // sibling parks below.
        let mut ticking = OwnedPage::new(Rc::clone(&context));
        ticking
            .boot(
                r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  __AppendElement(page, __CreateView(0));
                  setInterval(() => {}, 100);
                };
                ",
            )
            .await;
        let settled = ticking.page.epilogue_count();
        assert!(
            ticking.page.armed_deadline().is_some(),
            "the booted realm armed a timer"
        );

        let mut adopting = Harness::new(context, workers);
        adopting
            .until("A never asked for its entry", |h| !h.sources.is_empty())
            .await;
        adopting.answer("app:///main.js", ADOPTING_BOX);
        adopting
            .until("A never adopted a stylesheet", |h| h.wants_a_style_sheet())
            .await;

        // Both sleeps are spent inside A's job, and tokio's own timer is driven
        // by the `block_on` that job's wait re-entered — which is why the
        // deadline passes at all while a job holds the thread.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let parked = thread.queued();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(
            parked, 1,
            "the passed deadline queued exactly one settle, and the clock task \
             is waiting for it"
        );
        assert_eq!(
            thread.queued(),
            parked,
            "and the queue did not grow over twice as long a park"
        );

        adopting.answer_style_sheet(".box{width:20px}");
        adopting
            .until("A never finished after its adoption returned", |h| {
                h.finished()
            })
            .await;
        for _ in 0..64 {
            task::yield_now().await;
        }
        assert!(
            ticking.page.epilogue_count() > settled,
            "and the settle it queued ran once the queue drained"
        );
    });
}
