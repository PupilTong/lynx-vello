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
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::background::{WorkerCommand, WorkerMessage, WorkerStart};
use crate::esm::build_main_runtime;
use crate::jobs::{JsThread, JsThreadHandle};
use crate::link::{
    CommandSender, DetachedView, FramePost, InputEventPayload, PageUpdate, Published, ScrollEntry,
    ScrollMailbox, ViewNotice, detached_base, detached_outbox,
};
use crate::main::WorkerFactory;
use crate::main::runtime::{bound_metrics, card_entry};
use crate::main::tree::PageConfig;
use crate::resource::{SourceCompletion, SourceRequest, unanswered_source};
use crate::script::ScriptError;
use crate::test_support::{IntersectionWatcher, WatcherLog, take_watched};
use crate::threads::platform_script_error;
use crate::view::{
    EventRequester, NoWakeup, ScreenMetrics, StartupSource, StartupSources, resolve_startup_urls,
};

const TURNS: usize = 512;

fn on_a_js_thread<F, B>(body: B)
where
    F: std::future::Future<Output = ()> + 'static,
    B: FnOnce(JsThreadHandle) -> F,
{
    let thread = JsThread::new();
    let body = body(thread.handle());
    thread.run(body);
}

async fn open_realm(page: &Rc<Page>, entry: &str, url: &str) {
    let startup = RealmStartup {
        entry: url.to_owned(),
        ..RealmStartup::default()
    };
    crate::lifetime::run_job(page, JobKind::Start, move |page| {
        page.open_realm(ingredients(), startup);
        Some(())
    })
    .await;
    load_entry(
        Rc::clone(page),
        answered_entry(entry, url, &page.lifetime.token().clone()),
    )
    .await;
}

fn answered_entry(entry: &str, url: &str, token: &CancellationToken) -> StartupSource {
    let (completion, answer) = SourceCompletion::new(token.clone());
    completion.complete(Ok(LoadedSource::Module {
        source: card_entry(entry),
        url: url.to_owned(),
    }));
    StartupSource {
        url: url.to_owned(),
        answer,
    }
}

fn group(thread: &JsThreadHandle) -> (Rc<GroupContext>, mpsc::UnboundedReceiver<WorkerCommand>) {
    let js = build_main_runtime().expect("the group's runtime builds");
    group_over(thread, Ok(js))
}

fn group_over(
    thread: &JsThreadHandle,
    runtime: Result<ScriptRuntime, ScriptError>,
) -> (Rc<GroupContext>, mpsc::UnboundedReceiver<WorkerCommand>) {
    let (workers, commands) = mpsc::unbounded_channel();
    let context = GroupContext {
        js: Rc::new(RefCell::new(runtime)),
        style_pool: None,
        requester: Arc::new(NoWakeup),
        workers: WorkerFactory::new(workers, Arc::default()),
        thread: thread.clone(),
    };
    (Rc::new(context), commands)
}

const CREATE_VIEWPORT: Viewport = Viewport::new(320.0, 240.0);

fn committed_at(frame: &terminus::link::CommittedFrame) -> (u32, u32, u32) {
    (
        frame.viewport().width.to_bits(),
        frame.viewport().height.to_bits(),
        frame.device_pixel_ratio().to_bits(),
    )
}

fn metrics_of(viewport: Viewport) -> (u32, u32, u32) {
    (
        viewport.width.to_bits(),
        viewport.height.to_bits(),
        viewport.device_pixel_ratio.to_bits(),
    )
}

const SCREEN: ScreenMetrics =
    ScreenMetrics::for_viewport(CREATE_VIEWPORT.width, CREATE_VIEWPORT.height, 1.0);

fn ingredients() -> DocumentIngredients {
    DocumentIngredients::for_test(CREATE_VIEWPORT, PageConfig::default())
}

struct Harness {
    workers: mpsc::UnboundedReceiver<WorkerCommand>,
    background: Option<WorkerStart>,
    commands: CommandSender,
    metrics: watch::Sender<Option<Viewport>>,
    scroll: Arc<ScrollMailbox>,
    view: DetachedView,
    events: Vec<EngineEvent>,
    sources: Vec<(SourceRequest, SourceCompletion)>,
    preloads: Vec<SourceRequest>,
    owner: task::JoinHandle<()>,
}

impl Harness {
    fn new(context: Rc<GroupContext>, workers: mpsc::UnboundedReceiver<WorkerCommand>) -> Self {
        Self::serving(
            context,
            workers,
            ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
        )
    }

    fn unbound(context: Rc<GroupContext>, workers: mpsc::UnboundedReceiver<WorkerCommand>) -> Self {
        Self::binding(
            context,
            workers,
            ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US"),
            None,
        )
    }

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

    fn binding(
        context: Rc<GroupContext>,
        workers: mpsc::UnboundedReceiver<WorkerCommand>,
        mut sources: ViewSources,
        bound: Option<Viewport>,
    ) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let (commands, incoming) = mpsc::unbounded_channel();
        let (metrics, metric_receiver) = watch::channel(bound);
        let scroll = Arc::new(ScrollMailbox::default());
        let base = resolve_startup_urls(&mut sources).expect("a harness names URLs that resolve");
        assert_eq!(
            base,
            *detached_base(),
            "a harness view's base is its outbox's"
        );
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
            answer: request(SourceRequest::Module(entry_url.clone()), &mut outstanding),
            url: entry_url,
        };
        let attached = AttachedView {
            viewport: CREATE_VIEWPORT,
            sources,
            text_context: crate::view::stage_text_context(Vec::new(), None, Vec::new(), "en-US")
                .expect("en-US is a language tag"),
            startup: StartupSources { sheets, entry },
            native_modules: String::new(),
            commands: incoming,
            metrics: metric_receiver,
            scroll: Arc::clone(&scroll),
            cancel: view.token.clone(),
        };
        let owner = task::spawn_local(serve_view(context, attached, outbox));
        Self {
            workers,
            background: None,
            commands: CommandSender::new(commands),
            metrics,
            scroll,
            view,
            events: Vec::new(),
            sources: outstanding,
            preloads: Vec::new(),
            owner,
        }
    }

    fn post_frame(&self, now: f64, seq: u64) -> Result<(), mpsc::error::SendError<ToMain>> {
        post_frame(&self.scroll, &self.commands, now, seq)
    }

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
                | ViewNotice::RequestDocuments(_)
                | ViewNotice::Playback(_)
                | ViewNotice::WorkerCreated { .. }
                | ViewNotice::NativeModuleCall { .. }
                | ViewNotice::TextInputState(_)
                | ViewNotice::ScriptFrameDemand { .. } => {}
                ViewNotice::PreloadSource(request) => self.preloads.push(request),
            }
        }
    }

    async fn until(&mut self, what: &str, mut ready: impl FnMut(&mut Self) -> bool) {
        for _ in 0..TURNS {
            if ready(self) {
                return;
            }
            self.turn().await;
        }
        if self.owner.is_finished() {
            match (&mut self.owner).await {
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                _ => {}
            }
        }
        panic!("{what}");
    }

    fn answer(&mut self, url: &str, source: &str) {
        let (_, completion) = self.sources.pop().expect("a source request is outstanding");
        completion.complete(Ok(LoadedSource::Module {
            source: source.to_owned(),
            url: url.to_owned(),
        }));
    }

    fn answer_entry(&mut self, url: &str, body: &str) {
        self.answer(url, &card_entry(body));
    }

    fn answer_style_sheet_at(&mut self, url: &str, css: &str) {
        let position = self
            .sources
            .iter()
            .position(
                |(request, _)| matches!(request, SourceRequest::StyleSheet(named) if named == url),
            )
            .expect("that stylesheet request is outstanding");
        let (_, completion) = self.sources.remove(position);
        completion.complete(Ok(LoadedSource::StyleSheet(
            crate::resource::StyleSheetSource::Text(css.to_owned()),
        )));
    }

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

    fn refuse(&mut self) {
        let (_, completion) = self.sources.pop().expect("a source request is outstanding");
        completion.complete(Err(unanswered_source().into()));
    }

    fn refuse_style_sheet(&mut self) {
        let position = self
            .sources
            .iter()
            .position(|(request, _)| matches!(request, SourceRequest::StyleSheet(_)))
            .expect("a stylesheet request is outstanding");
        let (_, completion) = self.sources.remove(position);
        completion.complete(Err(unanswered_source().into()));
    }

    fn startup_failure(&self) -> Option<String> {
        self.events.iter().find_map(|event| match event {
            EngineEvent::StartupFailed(error) => Some(error.to_string()),
            _ => None,
        })
    }

    async fn boot(&mut self, entry: &str) -> u64 {
        self.until("the view never asked for its entry", |harness| {
            !harness.sources.is_empty()
        })
        .await;
        self.answer_entry("app:///main.js", entry);
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
    matches!(message, WorkerMessage::Post { data, .. }
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

fn post_frame(
    scroll: &ScrollMailbox,
    commands: &CommandSender,
    now: f64,
    seq: u64,
) -> Result<(), mpsc::error::SendError<ToMain>> {
    let fence = commands.sent();
    if scroll.post([], Some(FramePost { now, seq, fence })) {
        commands.send(ToMain::Posted)?;
    }
    Ok(())
}

struct OwnedPage {
    page: Rc<Page>,
    view: DetachedView,
    token: CancellationToken,
    _commands: mpsc::UnboundedSender<ToMain>,
    _metrics: watch::Sender<Option<Viewport>>,
    scroll: Arc<ScrollMailbox>,
}

impl OwnedPage {
    fn new(context: Rc<GroupContext>) -> Self {
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let token = view.token.clone();
        let (commands, incoming) = mpsc::unbounded_channel();
        let (metrics, metric_receiver) = watch::channel(Some(CREATE_VIEWPORT));
        let scroll = Arc::new(ScrollMailbox::default());
        let page = Page::new(
            context,
            outbox,
            metric_receiver,
            Arc::clone(&scroll),
            token.clone(),
        );
        page.spawn(consume_commands(Rc::clone(&page), incoming));
        Self {
            page,
            view,
            token,
            _commands: commands,
            _metrics: metrics,
            scroll,
        }
    }

    async fn scroll_to(
        &self,
        scroller: terminus::link::NodeId,
        offset: terminus::link::Vector2D<f32>,
    ) {
        self.post(
            scroller,
            ScrollEntry {
                offset,
                at: 0.0,
                rest: Some(offset),
                request: None,
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::zero(),
            },
        )
        .await;
    }

    async fn post(&self, scroller: terminus::link::NodeId, entry: ScrollEntry) {
        assert!(
            self.scroll.post([(scroller, entry)], None),
            "a marker is due"
        );
        self.page.apply(vec![ToMain::Posted]).await;
    }

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

const ONE_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  globalThis.box = box;
};
";

const ADOPTING_BOX: &str = r"
__AdoptStyleSheet(__LoadStyleSheet('CSS', '__Card__'));
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __AppendElement(page, __CreateView(0));
};
";

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

fn update_command(value: u8) -> ToMain {
    ToMain::PageUpdate(PageUpdate::Data {
        data: format!(r#"{{"value":{value}}}"#),
        processor_name: String::new(),
        reset: false,
    })
}

const ONE_BOX_WITH_TIMER: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  setTimeout(() => {}, 3600000);
};
";

const LISTENING_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  __AddEventListener(box, 'tap', () => {});
};
";

const SIZED_PAGE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;width:200px;height:240px;align-items:flex-start');
};
";

const LISTENING_ROWS_PAGE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;width:200px;height:240px;align-items:flex-start');
  const scroller = __CreateView(0);
  __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:hidden;width:200px;height:100px;align-items:flex-start');
  __AppendElement(page, scroller);
  for (let index = 0; index < 20; index++) {
    const row = __CreateView(0);
    __SetID(row, 'row' + index);
    __SetInlineStyles(row, 'display:flex;width:200px;height:20px;flex-shrink:0;content-visibility:auto;contain-intrinsic-size:200px 20px');
    __AppendElement(scroller, row);
  }
  const heard = [];
  __AddEventListener(page, 'contentvisibilityautostatechange', (event) => {
    heard.push(event.target.id + ':' + event.skipped);
    __SetAttribute(page, 'data-js-heard', heard.join(','));
  }, {});
};
";

const ROWS: usize = 20;
const ROW_HEIGHT: usize = 20;
const SCROLLPORT: usize = 100;

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

type Log = Arc<std::sync::Mutex<Vec<String>>>;

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

struct Recorder {
    who: &'static str,
    log: Log,
}

impl terminus::link::CustomElement<()> for Recorder {
    fn observed_events(&self) -> &'static [&'static str] {
        &["contentvisibilityautostatechange"]
    }

    fn handle_event(
        &self,
        document: &mut terminus::link::Document<()>,
        element: terminus::link::NodeId,
        event: &mut terminus::link::event::ElementEvent<'_>,
    ) {
        let terminus::link::event::EventDetail::ContentVisibility { skipped } = *event.detail()
        else {
            panic!(
                "an engine event this page never fires: {:?}",
                event.detail()
            )
        };
        let target = document
            .get(event.target())
            .and_then(|node| node.attribute("id"))
            .expect("every target here is a live row")
            .into_owned();
        let phase = match event.phase() {
            terminus::link::event::EventPhase::Capturing => "capture",
            terminus::link::event::EventPhase::AtTarget => "at-target",
            terminus::link::event::EventPhase::Bubbling => "bubble",
        };
        let state = if skipped { "skipped" } else { "shown" };
        self.log
            .lock()
            .expect("the log is never poisoned")
            .push(format!("{}:{phase}:{target}:{state}", self.who));
        document.set_inline_style_property(element, "opacity", "0.5");
    }
}

fn whole_path(row: usize, skipped: bool) -> Vec<String> {
    let state = if skipped { "skipped" } else { "shown" };
    vec![
        format!("scroller:capture:row{row}:{state}"),
        format!("row:at-target:row{row}:{state}"),
        format!("scroller:bubble:row{row}:{state}"),
    ]
}

async fn build_scrolling_rows(page: &Rc<Page>, log: &Log) -> terminus::link::NodeId {
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

async fn js_heard(page: &Rc<Page>) -> Option<String> {
    let (answer, heard) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let _ = answer.send(
            document
                .document_element()
                .attribute("data-js-heard")
                .map(std::borrow::Cow::into_owned),
        );
    }))])
    .await;
    heard.try_recv().expect("the probe ran")
}

#[test]
fn a_commit_delivers_its_state_changes_in_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(SIZED_PAGE).await;
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
    });
}

#[test]
fn a_scroll_past_the_window_delivers_both_directions_in_the_entry_after_its_commit() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(SIZED_PAGE).await;
        let log: Log = Arc::default();
        let scroller = build_scrolling_rows(&owned.page, &log).await;
        let before = admitted_rows(0);
        assert_eq!(take(&log).len(), before.len() * 3);
        let settled = owned.page.epilogue_count();

        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 150.0))
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the marker was one entry and the deliveries its commit decided \
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
    });
}

#[test]
fn a_realm_listener_hears_each_state_change_with_skipped() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(LISTENING_ROWS_PAGE).await;
        let expected: Vec<String> = admitted_rows(0)
            .into_iter()
            .map(|row| format!("row{row}:false"))
            .collect();
        assert_eq!(js_heard(&owned.page).await, Some(expected.join(",")));
    });
}

const WATCHED_ROWS: usize = 5;
const WATCHED_ROW_HEIGHT: usize = 40;
const WATCHED_SCROLLPORT: usize = 100;

async fn build_watched_rows(
    page: &Rc<Page>,
    watcher: IntersectionWatcher,
) -> (
    terminus::link::NodeId,
    terminus::link::IntersectionObserverId,
) {
    let (answer, built) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        document.define("x-watcher", Box::new(watcher));
        let root = document.document_element().id();
        let watcher = document.create_element("x-watcher", ());
        document.set_inline_style(
            watcher,
            "position:absolute;left:0;top:150px;width:10px;height:10px",
        );
        document.append_child(root, watcher);

        let scroller = document.create_element("view", ());
        document.set_inline_style(
            scroller,
            &format!(
                "display:flex;flex-direction:column;overflow:hidden;\
                 width:200px;height:{WATCHED_SCROLLPORT}px"
            ),
        );
        document.append_child(root, scroller);
        let rows: Vec<terminus::link::NodeId> = (0..WATCHED_ROWS)
            .map(|index| {
                let row = document.create_element("view", ());
                document.set_inline_style(
                    row,
                    &format!("flex-shrink:0;width:200px;height:{WATCHED_ROW_HEIGHT}px"),
                );
                document.set_id_attribute(row, Some(&format!("row{index}")));
                document.append_child(scroller, row);
                row
            })
            .collect();

        let observer = document.create_intersection_observer(
            Box::new(terminus::link::ElementHandler(watcher)),
            terminus::link::IntersectionObserverInit {
                root: Some(scroller),
                thresholds: vec![0.0, 0.5, 1.0],
                ..terminus::link::IntersectionObserverInit::default()
            },
        );
        for row in rows {
            document.observe_intersection(observer, row);
        }
        let _ = answer.send((scroller, observer));
    }))])
    .await;
    built.try_recv().expect("the probe ran")
}

fn watched(
    observer: terminus::link::IntersectionObserverId,
    row: usize,
    intersecting: bool,
    ratio: f64,
) -> String {
    format!("{}:row{row}:{intersecting}:{ratio}", observer.get())
}

#[test]
fn intersection_entries_are_delivered_by_an_entry_after_the_update() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(SIZED_PAGE).await;
        let log: WatcherLog = Arc::default();
        let settled = owned.page.epilogue_count();

        let (scroller, observer) =
            build_watched_rows(&owned.page, IntersectionWatcher::new(&log)).await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the probe's entry committed the rows and updated against that \
             commit, and the delivery was the entry after it",
        );
        assert_eq!(
            take_watched(&log),
            vec![
                watched(observer, 0, true, 1.0),
                watched(observer, 1, true, 1.0),
                watched(observer, 2, true, 0.5),
                watched(observer, 3, false, 0.0),
                watched(observer, 4, false, 0.0),
            ],
            "the first update after observe reports every target once, \
             whatever its state, in observe order",
        );

        let committed = owned.view.published.commit();
        let settled = owned.page.epilogue_count();
        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 30.0))
            .await;
        assert_eq!(
            owned.view.published.commit(),
            committed,
            "a windowed scroll commits nothing",
        );
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the marker was one entry and the delivery its update queued \
             was another",
        );
        assert_eq!(
            take_watched(&log),
            vec![
                watched(observer, 0, true, 0.25),
                watched(observer, 2, true, 1.0),
                watched(observer, 3, true, 0.25),
            ],
            "only the rows whose threshold index or intersecting state \
             moved: row 0 fell below 0.5, row 2 reached 1, row 3 appeared; \
             rows 1 and 4 stayed where they were",
        );

        let settled = owned.page.epilogue_count();
        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 31.0))
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 1,
            "an update that queues nothing posts no delivery",
        );
        assert_eq!(take_watched(&log), Vec::<String>::new());
        assert_eq!(owned.view.published.commit(), committed);
    });
}

#[test]
fn an_intersection_hook_that_mutates_gets_its_own_commit_and_no_second_delivery() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(SIZED_PAGE).await;
        let log: WatcherLog = Arc::default();
        let watcher = IntersectionWatcher::new(&log);
        let mutate = Arc::clone(&watcher.mutate);
        let (scroller, observer) = build_watched_rows(&owned.page, watcher).await;
        assert_eq!(take_watched(&log).len(), WATCHED_ROWS);
        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 30.0))
            .await;
        assert_eq!(take_watched(&log).len(), 3);

        mutate.store(true, std::sync::atomic::Ordering::Relaxed);
        let committed = owned
            .view
            .published
            .commit()
            .expect("the rows were committed");
        let settled = owned.page.epilogue_count();
        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 0.0))
            .await;
        assert_eq!(
            take_watched(&log),
            vec![
                watched(observer, 0, true, 1.0),
                watched(observer, 2, true, 0.5),
                watched(observer, 3, false, 0.0),
            ],
        );
        assert!(
            !mutate.load(std::sync::atomic::Ordering::Relaxed),
            "the delivery wrote to the tree"
        );
        assert_eq!(
            owned.view.published.commit(),
            Some(committed + 1),
            "the marker's windowed scroll committed nothing; the delivery \
             entry's epilogue committed what the hook wrote",
        );
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the marker and the delivery, and no third entry: the update \
             after the hook's commit found no row's observation moved",
        );
    });
}

fn exposure_page(exposed: Option<usize>) -> String {
    let exposed = exposed.map_or_else(|| "-1".to_owned(), |row| row.to_string());
    format!(
        r"
import {{ switchExposure }} from 'bobcat-internal:host';
const rows = [];
globalThis.renderPage = function () {{
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;width:200px;height:240px;align-items:flex-start');
  const scroller = __CreateView(0);
  __SetInlineStyles(
    scroller,
    'display:flex;flex-direction:column;overflow:hidden;width:200px;height:{WATCHED_SCROLLPORT}px',
  );
  __AppendElement(page, scroller);
  const heard = [];
  for (let index = 0; index < {WATCHED_ROWS}; index++) {{
    const row = __CreateView(0);
    rows.push(__GetElementUniqueID(row));
    __SetInlineStyles(row, 'flex-shrink:0;width:200px;height:{WATCHED_ROW_HEIGHT}px');
    __AppendElement(scroller, row);
    if (index === {exposed}) __SetAttribute(row, 'exposure-id', 'first');
    __AddEventListener(row, 'bobcat:exposure', () => {{}}, {{}});
    for (const name of ['uiappear', 'uidisappear']) {{
      __AddEventListener(row, name, () => {{
        heard.push(name + ':row' + index);
        __SetAttribute(page, 'data-heard', heard.join(','));
      }}, {{}});
    }}
  }}
}};
globalThis.updatePage = function (data) {{
  switchExposure(data.on, data.sendEvent, rows.join(','));
}};
"
    )
}

async fn heard(page: &Rc<Page>) -> Vec<String> {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let heard = document
            .document_element()
            .attribute("data-heard")
            .unwrap_or_default();
        let _ = answer.send(
            heard
                .split(',')
                .filter(|entry| !entry.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        );
    }))])
    .await;
    read.try_recv().expect("the probe ran")
}

async fn first_page_child(page: &Rc<Page>) -> terminus::link::NodeId {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let _ = answer.send(
            document
                .document_element()
                .first_child()
                .map(terminus::link::Node::id),
        );
    }))])
    .await;
    read.try_recv()
        .expect("the probe ran")
        .expect("the card rendered its scroller")
}

async fn switch_exposure(page: &Rc<Page>, on: u8, send_event: u8) {
    page.apply(vec![ToMain::PageUpdate(PageUpdate::Data {
        data: format!(r#"{{"on":{on},"sendEvent":{send_event}}}"#),
        processor_name: String::new(),
        reset: false,
    })])
    .await;
}

#[test]
fn exposure_events_are_delivered_by_an_entry_after_the_update() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(&exposure_page(None)).await;
        assert_eq!(
            heard(&owned.page).await,
            ["uiappear:row0", "uiappear:row1", "uiappear:row2"],
            "boot's own update reported every visible row, in registration order",
        );
        let scroller = first_page_child(&owned.page).await;

        let settled = owned.page.epilogue_count();
        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 30.0))
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the marker was one entry and the delivery its update queued was \
             another",
        );
        assert_eq!(
            heard(&owned.page).await[3..],
            ["uiappear:row3"],
            "row 3 appeared; rows 0 to 2 stayed visible and said nothing",
        );

        let settled = owned.page.epilogue_count();
        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 31.0))
            .await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 1,
            "an update that moves no exposure posts no delivery",
        );
        assert_eq!(heard(&owned.page).await.len(), 4);

        owned
            .scroll_to(scroller, terminus::link::Vector2D::new(0.0, 0.0))
            .await;
        assert_eq!(heard(&owned.page).await[4..], ["uidisappear:row3"]);
    });
}

const LAYOUT_CHANGE_ENTRY: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetInlineStyles(box, 'width:100px;height:50px');
  __AppendElement(page, box);
  const heard = [];
  __AddEventListener(box, 'layoutchange', (event) => {
    heard.push(event.detail.width + 'x' + event.detail.height);
    __SetAttribute(page, 'data-heard', heard.join(','));
  }, {});
  globalThis.box = box;
};
globalThis.updatePage = function (data) {
  __SetInlineStyles(globalThis.box, 'width:' + data.width + 'px;height:50px');
};
";

async fn resize_box(page: &Rc<Page>, width: u32) {
    page.apply(vec![ToMain::PageUpdate(PageUpdate::Data {
        data: format!(r#"{{"width":{width}}}"#),
        processor_name: String::new(),
        reset: false,
    })])
    .await;
}

#[test]
fn layout_changes_are_delivered_by_an_entry_after_the_commit() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(LAYOUT_CHANGE_ENTRY).await;
        assert_eq!(heard(&owned.page).await, ["100x50"]);

        let settled = owned.page.epilogue_count();
        resize_box(&owned.page, 140).await;
        assert_eq!(
            heard(&owned.page).await,
            ["100x50", "140x50"],
            "the resize was reported once, by an entry of its own",
        );
        assert_eq!(owned.page.epilogue_count(), settled + 3);

        let settled = owned.page.epilogue_count();
        resize_box(&owned.page, 140).await;
        assert_eq!(heard(&owned.page).await.len(), 2);
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the update and the probe: an unchanged size posts no delivery",
        );
    });
}

#[test]
fn a_resumed_exposure_is_delivered_by_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(&exposure_page(None)).await;
        assert_eq!(heard(&owned.page).await.len(), 3);

        let settled = owned.page.epilogue_count();
        switch_exposure(&owned.page, 0, 1).await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 1,
            "a stop that owes no record posts no delivery",
        );
        assert_eq!(heard(&owned.page).await.len(), 3, "and fires no event");

        let settled = owned.page.epilogue_count();
        switch_exposure(&owned.page, 1, 1).await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the resume was one entry and its delivery another",
        );
        assert_eq!(
            heard(&owned.page).await[3..],
            ["uiappear:row0", "uiappear:row1", "uiappear:row2"],
        );
    });
}

#[test]
fn a_stop_that_owes_records_posts_a_delivery_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(&exposure_page(Some(0))).await;
        assert_eq!(heard(&owned.page).await.len(), 3);

        let settled = owned.page.epilogue_count();
        switch_exposure(&owned.page, 0, 1).await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the stop was one entry and the delivery of its records another",
        );
        assert_eq!(
            heard(&owned.page).await.len(),
            3,
            "a stop sends records and fires no element event",
        );

        let settled = owned.page.epilogue_count();
        switch_exposure(&owned.page, 0, 1).await;
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 1,
            "a second stop owes nothing: the first forgot the row as exposed",
        );
    });
}

async fn scroll_state(
    page: &Rc<Page>,
    scroller: terminus::link::NodeId,
) -> (terminus::link::Vector2D<f32>, Option<std::num::NonZeroU64>) {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let _ = answer.send((
            document.scroll_offset(scroller),
            document.pending_scroll_request(scroller),
        ));
    }))])
    .await;
    read.try_recv().expect("the probe ran")
}

#[test]
fn a_post_older_than_the_pending_scroll_request_is_dropped() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX).await;
        let (answer, built) = std::sync::mpsc::channel();
        owned
            .page
            .apply(vec![ToMain::Probe(Box::new(move |document| {
                let root = document.document_element().id();
                let scroller = document.create_element("view", ());
                document.set_inline_style(
                    scroller,
                    "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px",
                );
                document.append_child(root, scroller);
                let filler = document.create_element("view", ());
                document.set_inline_style(filler, "flex-shrink:0;width:200px;height:1000px");
                document.append_child(scroller, filler);
                document.layout();
                document.scroll_to_with(
                    scroller,
                    terminus::link::Vector2D::new(0.0, 100.0),
                    terminus::link::scroll::ScrollBehavior::Instant,
                );
                let _ = answer.send((scroller, document.pending_scroll_request(scroller)));
            }))])
            .await;
        let (scroller, pending) = built.try_recv().expect("the probe ran");
        let serial = pending.expect("the request is pending");
        let entry = |offset: f32, request| ScrollEntry {
            offset: terminus::link::Vector2D::new(0.0, offset),
            at: 0.0,
            rest: None,
            request,
            anchor_shift: terminus::link::Vector2D::zero(),
            reach: terminus::link::Vector2D::zero(),
        };

        owned.post(scroller, entry(40.0, None)).await;
        assert_eq!(
            scroll_state(&owned.page, scroller).await,
            (terminus::link::Vector2D::new(0.0, 100.0), Some(serial)),
            "the stale post was dropped"
        );

        owned.post(scroller, entry(100.0, Some(serial))).await;
        assert_eq!(
            scroll_state(&owned.page, scroller).await,
            (terminus::link::Vector2D::new(0.0, 100.0), None),
            "adopted, and the request acknowledged"
        );

        owned.post(scroller, entry(60.0, Some(serial))).await;
        assert_eq!(
            scroll_state(&owned.page, scroller).await,
            (terminus::link::Vector2D::new(0.0, 60.0), None),
            "with nothing pending every post is adopted"
        );
    });
}

#[test]
fn a_post_made_before_an_anchoring_shift_is_completed_by_it() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX).await;
        let (answer, built) = std::sync::mpsc::channel();
        owned
            .page
            .apply(vec![ToMain::Probe(Box::new(move |document| {
                let root = document.document_element().id();
                let scroller = document.create_element("view", ());
                document.set_inline_style(
                    scroller,
                    "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px",
                );
                document.append_child(root, scroller);
                let rows: Vec<_> = (0..10)
                    .map(|_| {
                        let row = document.create_element("view", ());
                        document.set_inline_style(row, "flex-shrink:0;width:200px;height:100px");
                        document.append_child(scroller, row);
                        row
                    })
                    .collect();
                document.layout();
                document.scroll_to(scroller, terminus::link::Vector2D::new(0.0, 150.0));
                document.set_inline_style(rows[0], "flex-shrink:0;width:200px;height:150px");
                document.layout();
                let _ = answer.send((scroller, document.scroll_anchor_shift(scroller)));
            }))])
            .await;
        let (scroller, shift) = built.try_recv().expect("the probe ran");
        assert_eq!(shift, terminus::link::Vector2D::new(0.0, 50.0));
        assert_eq!(
            scroll_state(&owned.page, scroller).await.0,
            terminus::link::Vector2D::new(0.0, 200.0),
            "row 0 grew 50px above the anchor"
        );
        let entry = |offset: f32, applied: f32| ScrollEntry {
            offset: terminus::link::Vector2D::new(0.0, offset),
            at: 0.0,
            rest: None,
            request: None,
            anchor_shift: terminus::link::Vector2D::new(0.0, applied),
            reach: terminus::link::Vector2D::zero(),
        };

        owned.post(scroller, entry(160.0, 0.0)).await;
        assert_eq!(
            scroll_state(&owned.page, scroller).await.0,
            terminus::link::Vector2D::new(0.0, 210.0),
            "the painter moved 10px before it saw the shift: both count"
        );

        owned.post(scroller, entry(215.0, 50.0)).await;
        assert_eq!(
            scroll_state(&owned.page, scroller).await.0,
            terminus::link::Vector2D::new(0.0, 215.0),
            "a post that names the whole shift is adopted as it is"
        );
    });
}

const SWIPER_WITH_STATE_LISTENERS: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const swiper = __CreateElement('x-swiper', 0);
  __SetAttribute(swiper, 'vertical', '');
  __SetInlineStyles(swiper, 'width:100px;height:200px');
  __AppendElement(page, swiper);
  for (let i = 0; i < 3; i++) {
    __AppendElement(swiper, __CreateElement('x-swiper-item', 0));
  }
  globalThis.swiper = swiper;
  const heard = [];
  for (const name of ['scrollstart', 'change', 'scrollend']) {
    __AddEventListener(swiper, name, (event) => {
      heard.push(name + ':' + event.detail.current);
      __SetAttribute(page, 'data-heard', heard.join(','));
    }, {});
  }
};
globalThis.updatePage = function (data) {
  __SetAttribute(globalThis.swiper, 'current', String(data.current));
};
";

#[test]
fn a_current_written_without_a_flush_fires_the_swipers_events_from_its_epilogue() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(SWIPER_WITH_STATE_LISTENERS).await;
        assert_eq!(heard(&owned.page).await, Vec::<String>::new());
        let (answer, read) = std::sync::mpsc::channel();
        owned
            .page
            .apply(vec![ToMain::Probe(Box::new(move |document| {
                let swiper = document
                    .document_element()
                    .first_child()
                    .map(terminus::link::Node::id)
                    .expect("the card rendered its swiper");
                let _ = answer.send(crate::main::tree::swiper::content(document, swiper));
            }))])
            .await;
        let scroller = read
            .try_recv()
            .expect("the probe ran")
            .expect("the swiper's `#content`");

        owned
            .page
            .apply(vec![ToMain::PageUpdate(PageUpdate::Data {
                data: r#"{"current":2}"#.to_owned(),
                processor_name: String::new(),
                reset: false,
            })])
            .await;
        assert_eq!(heard(&owned.page).await, ["scrollstart:0", "change:2"]);

        let (offset, pending) = scroll_state(&owned.page, scroller).await;
        assert_eq!(
            offset,
            terminus::link::Vector2D::new(0.0, 400.0),
            "turned to item 2"
        );
        let serial = pending.expect("the turn is a pending request");
        owned
            .post(
                scroller,
                ScrollEntry {
                    offset,
                    at: 0.0,
                    rest: Some(offset),
                    request: Some(serial),
                    anchor_shift: terminus::link::Vector2D::zero(),
                    reach: terminus::link::Vector2D::zero(),
                },
            )
            .await;
        assert_eq!(
            heard(&owned.page).await,
            ["scrollstart:0", "change:2", "scrollend:2"]
        );
    });
}

const TRANSITION_ON_TAP: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __AppendElement(page, box);
  __SetInlineStyles(box, 'width:10px;height:10px;opacity:1;transition:opacity 1s linear');
  __AddEventListener(box, 'tap', () => {
    __SetInlineStyles(box, 'width:10px;height:10px;opacity:0.5;transition:opacity 1s linear');
    __FlushElementTree();
  });
};
";

async fn first_box(page: &Rc<Page>) -> terminus::link::NodeId {
    let (target, found) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let root = document.document_element().id();
        let _ = target.send(document.get(root).expect("the page is live").child_ids()[0]);
    }))])
    .await;
    found.try_recv().expect("the probe ran")
}

fn tap(target: terminus::link::NodeId) -> ToMain {
    ToMain::DispatchEvent {
        target,
        name: "tap",
        payload: InputEventPayload::default(),
        release: None,
    }
}

#[test]
fn a_burst_runs_its_listeners_before_the_frame_it_posted() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(TRANSITION_ON_TAP).await;
        let target = first_box(&owned.page).await;
        let fence = owned.page.applied.get();
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.5,
                seq: 1,
                fence
            })
        ));
        owned.page.apply(vec![ToMain::Posted, tap(target)]).await;
        let frame = owned.view.published.frame().expect("a frame is published");
        assert_eq!(
            frame.animation_slots().len(),
            1,
            "the frame anchored the transition the listener started"
        );
        assert_eq!(owned.view.published.begin_frame_serviced(), 1);
    });
}

#[test]
fn a_frame_post_waits_for_the_commands_sent_before_it() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(TRANSITION_ON_TAP).await;
        let target = first_box(&owned.page).await;
        let serviced = owned.view.published.begin_frame_serviced();
        let fence = owned.page.applied.get() + 2;
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.5,
                seq: serviced + 1,
                fence
            })
        ));
        owned.page.apply(vec![ToMain::Posted]).await;
        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            serviced,
            "not acknowledged ahead of the tap sent before it"
        );
        owned.page.apply(vec![tap(target)]).await;
        assert_eq!(owned.view.published.begin_frame_serviced(), serviced + 1);
        let frame = owned.view.published.frame().expect("a frame is published");
        assert_eq!(
            frame.animation_slots().len(),
            1,
            "the frame anchored the transition the tap's listener started"
        );
    });
}

#[test]
fn a_frame_post_behind_a_module_callback_still_waits_for_the_commands_sent_before_it() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .boot(&format!(
                "{TRANSITION_ON_TAP}
import {{ callNativeModule }} from 'bobcat:native-modules';
callNativeModule('Echo', 'echo', [() => console.log('answered')]);
"
            ))
            .await;
        let target = first_box(&owned.page).await;
        let serviced = owned.view.published.begin_frame_serviced();
        let sent = owned.page.applied.get();
        owned
            .page
            .apply(vec![ToMain::ModuleCallback {
                call: 1,
                index: 0,
                arguments: Some("[]".to_owned()),
            }])
            .await;
        assert!(
            owned.events().iter().any(|event| {
                matches!(event, EngineEvent::ConsoleMessage { message, .. } if message == "answered")
            }),
            "the burst ran the function the realm kept for the answer"
        );
        let fence = sent + 2;
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.5,
                seq: serviced + 1,
                fence
            })
        ));
        owned.page.apply(vec![ToMain::Posted]).await;
        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            serviced,
            "not acknowledged ahead of the tap sent before it"
        );
        owned.page.apply(vec![tap(target)]).await;
        assert_eq!(owned.view.published.begin_frame_serviced(), serviced + 1);
        let frame = owned.view.published.frame().expect("a frame is published");
        assert_eq!(
            frame.animation_slots().len(),
            1,
            "the frame anchored the transition the tap's listener started"
        );
    });
}

#[test]
fn two_frame_posts_before_a_take_are_one_frame_at_the_latest_clock() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(TRANSITION_ON_TAP).await;
        let target = first_box(&owned.page).await;
        owned.page.apply(vec![tap(target)]).await;

        let fence = owned.page.applied.get();
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.25,
                seq: 3,
                fence
            })
        ));
        assert!(
            !owned.scroll.post(
                [],
                Some(FramePost {
                    now: 0.5,
                    seq: 5,
                    fence
                })
            ),
            "the first post's marker stands for both"
        );
        owned.page.apply(vec![ToMain::Posted]).await;
        assert_eq!(owned.view.published.begin_frame_serviced(), 5);
        let frame = owned.view.published.frame().expect("a frame is published");
        assert_eq!(frame.animation_slots().len(), 1, "the frame anchored it");
        assert!(
            !frame.animation_boundary_passed(1.375) && frame.animation_boundary_passed(1.5),
            "anchored at 0.5, the later post's clock, so it ends at 1.5"
        );
    });
}

#[test]
fn a_job_entered_at_the_painter_clock_sees_the_timeline_there() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX).await;
        let clock = |page: &Rc<Page>| {
            owner::enter(page, JobKind::Command, |runtime, _| {
                runtime.animation_clock()
            })
        };

        owned.scroll.set_clock(2.5);
        assert_eq!(clock(&owned.page).await, Some(2.5));

        owned.scroll.set_clock(3.0);
        assert_eq!(Page::settle(&owned.page, JobKind::Settle).await, Some(()));
        owned.scroll.set_clock(1.0);
        assert_eq!(
            clock(&owned.page).await,
            Some(3.0),
            "the timer's job moved the timeline, and nothing moves it back"
        );
    });
}

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
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
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
        let failures: Vec<_> = harness
            .events
            .iter()
            .filter(|event| {
                event.is_fatal()
                    || matches!(
                        event,
                        EngineEvent::ScriptRunError(_) | EngineEvent::ScriptReported { .. }
                    )
            })
            .collect();
        assert!(
            failures.is_empty(),
            "the page data reached the realm as given: {failures:?}"
        );
    });
}

#[test]
fn a_burst_of_commands_is_one_commit_and_one_acknowledgement() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX_WITH_UPDATE).await;

        for step in 0..5u8 {
            harness
                .commands
                .send(update_command(step))
                .expect("the view is still serving");
        }
        harness
            .post_frame(0.0, 7)
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

#[test]
fn a_frame_held_before_the_binding_never_replaces_a_newer_one() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::unbound(context, workers);
        harness
            .until("the view never asked for its entry", |h| {
                !h.sources.is_empty()
            })
            .await;
        for _ in 0..8 {
            harness.turn().await;
        }
        assert_eq!(
            harness.view.published.commit(),
            None,
            "the empty document's frame is held rather than published"
        );

        harness.answer_entry("app:///main.js", ONE_BOX);
        harness.metrics.send_replace(Some(CREATE_VIEWPORT));
        harness
            .until("boot never finished once bound", |h| h.finished())
            .await;
        harness.background = Some(harness.background_worker());
        let (probe, probed) = std::sync::mpsc::channel();
        harness
            .commands
            .send(ToMain::Probe(Box::new(move |document| {
                let _ = probe.send(document.committed_frame().map(|frame| frame.commit_id()));
            })))
            .expect("the view is serving");
        let mut last = None;
        harness
            .until("the probe never ran", |_| {
                last = probed.try_recv().ok();
                last.is_some()
            })
            .await;
        assert_eq!(
            harness.view.published.commit(),
            last.flatten(),
            "the published frame is the document's newest commit"
        );
    });
}

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
        harness.answer_entry("app:///main.js", ONE_BOX);

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
        harness.answer_entry("app:///main.js", ONE_BOX);
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
        harness.answer_entry("app:///main.js", ONE_BOX);
        for _ in 0..64 {
            harness.turn().await;
        }
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

        for _ in 0..8 {
            harness.turn().await;
        }
        assert_eq!(harness.view.published.commit(), Some(booted + 1));
    });
}

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
                    assert_eq!(node.attribute("data-value").as_deref(), Some("7"));
                })),
            ])
            .await;
        assert_eq!(owned.view.published.commit(), Some(booted + 1));
        assert!(!owned.page.ended());
    });
}

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

async fn image_attributes(page: &Rc<Page>, name: &'static str) -> Option<String> {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let root = document.document_element().id();
        let image = document.get(root).expect("the page is live").child_ids()[0];
        let _ = answer.send(
            document
                .get(image)
                .and_then(|node| node.attribute(name))
                .map(std::borrow::Cow::into_owned),
        );
    }))])
    .await;
    read.try_recv().expect("the probe ran")
}

#[test]
fn an_image_load_reaches_its_listener_in_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        let booted = owned.boot(LOADING_IMAGE).await;

        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![ToMain::ImageEvents(vec![
                terminus::link::ImageEvent::Loaded {
                    source: Arc::from("app:///b.png"),
                    width: 10,
                    height: 5,
                    animated: false,
                },
            ])])
            .await;
        assert_eq!(owned.page.epilogue_count(), settled + 1);
        assert_eq!(image_attributes(&owned.page, "data-loads").await, None);

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
            .apply(vec![ToMain::ImageEvents(vec![
                terminus::link::ImageEvent::Loaded {
                    source: Arc::from("app:///a.png"),
                    width: 40,
                    height: 20,
                    animated: false,
                },
            ])])
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

#[test]
fn an_image_event_queued_by_a_listener_is_delivered_by_an_entry_of_its_own() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .boot(&LOADING_IMAGE.replace(
                "__SetAttribute(image, 'data-loads', String(globalThis.loads));",
                "__SetAttribute(image, 'data-loads', String(globalThis.loads));
        if (globalThis.loads === 1) __SetAttribute(image, 'src', 'app:///b.png');",
            ))
            .await;
        owned
            .page
            .apply(vec![ToMain::ImageEvents(vec![
                terminus::link::ImageEvent::Loaded {
                    source: Arc::from("app:///b.png"),
                    width: 10,
                    height: 5,
                    animated: false,
                },
            ])])
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
            .apply(vec![ToMain::ImageEvents(vec![
                terminus::link::ImageEvent::Loaded {
                    source: Arc::from("app:///a.png"),
                    width: 40,
                    height: 20,
                    animated: false,
                },
            ])])
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

#[cfg(not(target_arch = "wasm32"))]
const LOADING_SVGS: &str = r"
globalThis.runWorklet = (value, params) => value.body(params[0]);
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const holder = __CreateView(0);
  __SetInlineStyles(holder, 'display:flex;flex-direction:row;align-items:flex-start');
  __AppendElement(page, holder);
  globalThis.svgs = ['width:120px;height:80px', '', 'display:none'].map((style) => {
    const svg = __CreateElement('svg', 0);
    if (style) __SetInlineStyles(svg, style);
    __AppendElement(holder, svg);
    const seen = [];
    for (const name of ['load', 'error']) {
      __AddEvent(svg, 'bindEvent', name, {
        type: 'worklet',
        value: {
          body: (event) => {
            seen.push(event.type + ':' + JSON.stringify(event.detail));
            __SetAttribute(svg, 'data-seen', seen.join('|'));
          },
        },
      });
    }
    return svg;
  });
};
globalThis.updatePage = (data) => {
  for (const svg of svgs) __SetAttribute(svg, data.name, data.value);
};
";

#[cfg(not(target_arch = "wasm32"))]
const LOADING_IMAGES: &str = r"
globalThis.runWorklet = (value, params) => value.body(params[0]);
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const holder = __CreateView(0);
  __SetInlineStyles(holder, 'display:flex;flex-direction:row;align-items:flex-start');
  __AppendElement(page, holder);
  globalThis.images = ['width:120px;height:80px', '', 'display:none'].map((style) => {
    const image = __CreateImage(0);
    if (style) __SetInlineStyles(image, style);
    __AppendElement(holder, image);
    const seen = [];
    for (const name of ['load', 'error']) {
      __AddEvent(image, 'bindEvent', name, {
        type: 'worklet',
        value: {
          body: (event) => {
            seen.push(event.type + ':' + JSON.stringify(event.detail));
            __SetAttribute(image, 'data-seen', seen.join('|'));
          },
        },
      });
    }
    return image;
  });
};
globalThis.updatePage = (data) => {
  for (const image of images) __SetAttribute(image, data.name, data.value);
};
";

#[cfg(not(target_arch = "wasm32"))]
async fn images_seen(page: &Rc<Page>) -> Vec<Option<String>> {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let root = document.document_element().id();
        let holder = document.get(root).expect("the page is live").child_ids()[0];
        let seen = document
            .get(holder)
            .expect("the holder is live")
            .child_ids()
            .iter()
            .map(|image| {
                document
                    .get(*image)
                    .and_then(|node| node.attribute("data-seen"))
                    .map(std::borrow::Cow::into_owned)
            })
            .collect();
        let _ = answer.send(seen);
    }))])
    .await;
    read.try_recv().expect("the probe ran")
}

#[cfg(not(target_arch = "wasm32"))]
async fn write_on_images(page: &Rc<Page>, name: &str, value: &str) {
    page.apply(vec![ToMain::PageUpdate(PageUpdate::Data {
        data: format!(r#"{{"name":"{name}","value":"{value}"}}"#),
        processor_name: String::new(),
        reset: false,
    })])
    .await;
}

fn empty_svg(width: u32, height: u32) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"/>"#)
}

#[cfg(not(target_arch = "wasm32"))]
fn host_report(source: &str, svg: &str) -> ToMain {
    ToMain::ImageEvents(vec![terminus::link::ImageEvent::parse_document(
        Arc::from(source),
        svg.as_bytes(),
        terminus::link::DocumentKind::Svg,
    )])
}

#[cfg(not(target_arch = "wasm32"))]
impl OwnedPage {
    fn document_requests(&mut self) -> Vec<Vec<terminus::link::DocumentRequest>> {
        let mut requests = Vec::new();
        while let Ok(notice) = self.view.notices.try_recv() {
            if let ViewNotice::RequestDocuments(documents) = notice {
                requests.push(documents);
            }
        }
        requests
    }

    async fn answer_documents(&self, requests: Vec<terminus::link::DocumentRequest>) {
        let reports = requests
            .into_iter()
            .map(|(source, bytes, kind)| {
                terminus::link::ImageEvent::parse_document(source, &bytes, kind)
            })
            .collect();
        self.page.apply(vec![ToMain::ImageEvents(reports)]).await;
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn an_image_load_on_a_parsed_svg_document_carries_its_natural_size() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(LOADING_IMAGES).await;

        write_on_images(&owned.page, "src", "app:///a.svg").await;
        assert_eq!(images_seen(&owned.page).await, vec![None, None, None]);
        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![host_report("app:///a.svg", &empty_svg(30, 15))])
            .await;
        for _ in 0..TURNS {
            task::yield_now().await;
        }
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the report was one entry, and the three `load`s it settled were \
             delivered by one more"
        );
        let natural = Some(r#"load:{"width":30,"height":15}"#.to_owned());
        assert_eq!(
            images_seen(&owned.page).await,
            vec![natural.clone(), natural.clone(), natural]
        );
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn an_svg_load_carries_the_layout_size_for_src_and_content() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(LOADING_SVGS).await;

        write_on_images(&owned.page, "src", "app:///a.svg").await;
        assert_eq!(images_seen(&owned.page).await, vec![None, None, None]);
        assert!(
            owned.document_requests().is_empty(),
            "a URL is fetched, not handed over"
        );
        let settled = owned.page.epilogue_count();
        owned
            .page
            .apply(vec![host_report("app:///a.svg", &empty_svg(30, 15))])
            .await;
        for _ in 0..TURNS {
            task::yield_now().await;
        }
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the report was one entry, and the three `load`s it settled were \
             delivered by one more"
        );
        assert_eq!(
            images_seen(&owned.page).await,
            vec![
                Some(r#"load:{"width":120,"height":80}"#.to_owned()),
                Some(r#"load:{"width":30,"height":15}"#.to_owned()),
                Some(r#"load:{"width":0,"height":0}"#.to_owned()),
            ]
        );

        let markup = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='5'/>";
        let settled = owned.page.epilogue_count();
        write_on_images(&owned.page, "content", markup).await;
        let requests = owned.document_requests();
        let [request] = requests.as_slice() else {
            panic!("one notice for the write's entry: {requests:?}");
        };
        let [(source, bytes, kind)] = request.as_slice() else {
            panic!("one document for three elements with one markup: {request:?}");
        };
        assert!(source.starts_with("svg-content:"), "{source}");
        assert_eq!(&**bytes, markup.as_bytes(), "the markup as written");
        assert_eq!(*kind, terminus::link::DocumentKind::Svg);
        for _ in 0..TURNS {
            task::yield_now().await;
        }
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 1,
            "the write's entry alone: nothing parses here, so nothing applies \
             until the host reports"
        );
        assert_eq!(
            images_seen(&owned.page).await,
            vec![
                Some(r#"load:{"width":120,"height":80}"#.to_owned()),
                Some(r#"load:{"width":30,"height":15}"#.to_owned()),
                Some(r#"load:{"width":0,"height":0}"#.to_owned()),
            ],
            "no `load` before the report"
        );

        let settled = owned.page.epilogue_count();
        owned.answer_documents(request.clone()).await;
        for _ in 0..TURNS {
            task::yield_now().await;
        }
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 2,
            "the report was one entry, and the three `load`s it settled were \
             delivered by one more"
        );
        let after_content = vec![
            Some(r#"load:{"width":120,"height":80}|load:{"width":120,"height":80}"#.to_owned()),
            Some(r#"load:{"width":30,"height":15}|load:{"width":10,"height":5}"#.to_owned()),
            Some(r#"load:{"width":0,"height":0}|load:{"width":0,"height":0}"#.to_owned()),
        ];
        assert_eq!(images_seen(&owned.page).await, after_content);

        let settled = owned.page.epilogue_count();
        write_on_images(&owned.page, "content", markup).await;
        for _ in 0..TURNS {
            task::yield_now().await;
        }
        assert_eq!(
            owned.page.epilogue_count(),
            settled + 1,
            "the same markup again is the write's entry alone"
        );
        assert!(
            owned.document_requests().is_empty(),
            "and asks the host for nothing"
        );
        assert_eq!(images_seen(&owned.page).await, after_content);
    });
}

#[cfg(not(target_arch = "wasm32"))]
const IMAGE_AND_SVG: &str = r"
globalThis.runWorklet = (value, params) => value.body(params[0]);
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  for (const element of [__CreateImage(0), __CreateElement('svg', 0)]) {
    __AppendElement(page, element);
    const seen = [];
    for (const name of ['load', 'error']) {
      __AddEvent(element, 'bindEvent', name, {
        type: 'worklet',
        value: {
          body: (event) => {
            seen.push(event.type);
            __SetAttribute(element, 'data-seen', seen.join('|'));
          },
        },
      });
    }
    __SetAttribute(element, 'src', 'app:///broken.svg');
  }
};
";

#[cfg(not(target_arch = "wasm32"))]
async fn page_children_seen(page: &Rc<Page>) -> Vec<Option<String>> {
    let (answer, read) = std::sync::mpsc::channel();
    page.apply(vec![ToMain::Probe(Box::new(move |document| {
        let root = document.document_element().id();
        let seen = document
            .get(root)
            .expect("the page is live")
            .child_ids()
            .iter()
            .map(|child| {
                document
                    .get(*child)
                    .and_then(|node| node.attribute("data-seen"))
                    .map(std::borrow::Cow::into_owned)
            })
            .collect();
        let _ = answer.send(seen);
    }))])
    .await;
    read.try_recv().expect("the probe ran")
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_malformed_svg_document_fails_its_source() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(IMAGE_AND_SVG).await;
        assert_eq!(page_children_seen(&owned.page).await, vec![None, None]);

        owned
            .page
            .apply(vec![host_report(
                "app:///broken.svg",
                r#"<svg xmlns="http://www.w3.org/2000/svg"><rect></svg>"#,
            )])
            .await;
        assert_eq!(
            page_children_seen(&owned.page).await,
            vec![Some("error".to_owned()), None]
        );
    });
}

const SVG_BEFORE_THE_FIRST_FLUSH: &str = r"
globalThis.runWorklet = (value, params) => value.body(params[0]);
const page = __CreatePage('card', 0);
const svg = __CreateElement('svg', 0);
__AppendElement(page, svg);
__AddEvent(svg, 'bindEvent', 'load', {
  type: 'worklet',
  value: { body: (event) => __SetAttribute(svg, 'data-seen', JSON.stringify(event.detail)) },
});
__SetAttribute(svg, 'src', 'app:///a.svg');
await import('app:///dep.js');
";

fn answer_module_at(harness: &mut Harness, url: &str, source: &str) {
    let position = harness
        .sources
        .iter()
        .position(|(request, _)| matches!(request, SourceRequest::Module(named) if named == url))
        .expect("that module request is outstanding");
    let (_, completion) = harness.sources.remove(position);
    completion.complete(Ok(LoadedSource::Module {
        source: source.to_owned(),
        url: url.to_owned(),
    }));
}

async fn first_child_seen(harness: &mut Harness) -> Option<String> {
    let (probe, probed) = std::sync::mpsc::channel();
    harness
        .commands
        .send(ToMain::Probe(Box::new(move |document| {
            let page = document.document_element().id();
            let first = document.get(page).expect("the page is live").child_ids()[0];
            let _ = probe.send(
                document
                    .get(first)
                    .and_then(|node| node.attribute("data-seen"))
                    .map(std::borrow::Cow::into_owned),
            );
        })))
        .expect("the view is serving");
    let mut seen = None;
    harness
        .until("the probe never ran", |_| {
            seen = probed.try_recv().ok();
            seen.is_some()
        })
        .await;
    seen.expect("the probe answered")
}

#[test]
fn an_svg_load_settled_before_the_first_commit_waits_for_it() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                style_sheets: vec!["app:///a.css".to_owned()],
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
            },
        );
        harness
            .until("the view never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        answer_module_at(
            &mut harness,
            "app:///main.js",
            &card_entry(SVG_BEFORE_THE_FIRST_FLUSH),
        );
        harness
            .until("the entry's import was never requested", |harness| {
                harness.sources.iter().any(|(request, _)| {
                    matches!(request, SourceRequest::Module(url) if url == "app:///dep.js")
                })
            })
            .await;

        harness
            .commands
            .send(ToMain::ImageEvents(vec![
                terminus::link::ImageEvent::parse_document(
                    Arc::from("app:///a.svg"),
                    empty_svg(30, 15).as_bytes(),
                    terminus::link::DocumentKind::Svg,
                ),
            ]))
            .expect("the view is serving");
        for _ in 0..16 {
            harness.turn().await;
        }
        assert_eq!(
            first_child_seen(&mut harness).await,
            None,
            "nothing is committed yet, so the `load` is held"
        );
        assert!(harness.view.published.commit().is_none());

        harness.answer_style_sheet_at("app:///a.css", "lynx-svg{width:50px;height:40px}");
        answer_module_at(&mut harness, "app:///dep.js", "export const value = 1;");
        harness.until("boot never finished", |h| h.finished()).await;
        let mut seen = None;
        for _ in 0..TURNS {
            seen = first_child_seen(&mut harness).await;
            if seen.is_some() {
                break;
            }
        }
        assert_eq!(
            seen.as_deref(),
            Some(r#"{"width":50,"height":40}"#),
            "delivered after the commit that laid the element out"
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

#[test]
fn a_failed_module_is_reported_once_and_the_view_goes_on() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness
            .boot(&format!("{ONE_BOX_WITH_UPDATE}\nimport('app:///dep.js');"))
            .await;
        let mut background = harness.background_worker();
        assert!(
            matches!(
                background.messages.try_recv(),
                Ok(WorkerMessage::Post { .. })
            ),
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
        harness
            .post_frame(0.0, 9)
            .expect("the view is still serving");
        harness
            .until(
                "the frame post after the failure was never acknowledged",
                |h| h.view.published.begin_frame_serviced() == 9,
            )
            .await;
        harness
            .commands
            .send(update_command(1))
            .expect("the view is still serving");
        harness
            .until("the update after the failure never committed", |h| {
                h.view.published.commit() > Some(booted)
            })
            .await;

        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, EngineEvent::ScriptRunError(_)))
                .count(),
            1,
            "one failure is reported once: {:?}",
            harness.events
        );
        assert!(
            !harness.view.token.is_cancelled(),
            "the failure did not end the view"
        );
        while let Ok(message) = background.messages.try_recv() {
            assert!(
                !matches!(message, WorkerMessage::Terminate),
                "the failure did not end the BTS"
            );
        }
    });
}

#[test]
fn a_page_update_that_fails_is_reported_and_the_next_one_applies() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        let booted = owned.boot(ONE_BOX_WITH_UPDATE).await;
        owned
            .page
            .apply(vec![
                ToMain::PageUpdate(PageUpdate::Data {
                    data: "not JSON".into(),
                    processor_name: String::new(),
                    reset: false,
                }),
                update_command(2),
                ToMain::Probe(Box::new(|document| {
                    let node = document.document_element().children().next().unwrap();
                    assert_eq!(node.attribute("data-value").as_deref(), Some("2"));
                })),
            ])
            .await;

        assert!(
            !owned.page.ended(),
            "the failed update did not end the view"
        );
        assert_eq!(owned.view.published.commit(), Some(booted + 1));
        let failures: Vec<_> = owned
            .events()
            .into_iter()
            .filter(|event| event.is_fatal() || matches!(event, EngineEvent::ScriptRunError(_)))
            .collect();
        assert!(
            matches!(failures.as_slice(), [EngineEvent::ScriptRunError(_)]),
            "one ScriptRunError and nothing fatal: {failures:?}"
        );
    });
}

#[test]
fn a_frame_that_fails_during_boot_is_reported_and_boot_still_finishes_once() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("the view never asked for its entry", |h| {
                !h.sources.is_empty()
            })
            .await;
        harness.answer_entry(
            "app:///main.js",
            &format!(
                "{ONE_BOX}
import {{ Future }} from 'bobcat:future';
import {{ testFuture }} from 'bobcat-internal:host';
lynx.requestAnimationFrame(() => Promise.reject(new Error('the frame failed')));
console.log('armed');
await new Future(testFuture(1000, 'settled', false));
"
            ),
        );
        harness
            .until("the entry never asked for its frame", |h| {
                h.events.iter().any(|event| {
                    matches!(event, EngineEvent::ConsoleMessage { message, .. } if message == "armed")
                })
            })
            .await;
        harness
            .commands
            .send(ToMain::Vsync(16.0))
            .expect("the view is serving");
        harness
            .until("the failed frame was never reported", |h| {
                h.events
                    .iter()
                    .any(|event| matches!(event, EngineEvent::ScriptRunError(_)))
            })
            .await;
        assert!(
            !harness.finished(),
            "boot was still outstanding when the frame failed"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !harness.finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "boot never finished: {:?}",
                harness.events
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            harness.turn().await;
        }
        harness.background = Some(harness.background_worker());
        for _ in 0..8 {
            harness.turn().await;
        }
        let failures: Vec<_> = harness
            .events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::ScriptRunError(error) => Some(error.message.to_string()),
                _ if event.is_fatal() => panic!("boot failed: {event:?}"),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("the frame failed"), "{}", failures[0]);
        assert_eq!(
            harness
                .events
                .iter()
                .filter(|event| matches!(event, EngineEvent::ScriptFinished))
                .count(),
            1,
            "boot finished once"
        );
    });
}

#[test]
fn a_siblings_checkpoint_makes_a_parked_page_settle() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let page = Page::new(
            Rc::clone(&context),
            outbox,
            bound_metrics(CREATE_VIEWPORT),
            Arc::default(),
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

        for _ in 0..8 {
            task::yield_now().await;
        }
        let settled = page.epilogue_count();

        context
            .js
            .borrow()
            .as_ref()
            .expect("the group's runtime was built")
            .mark_checkpoint();
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

#[test]
fn finishing_a_task_on_a_failed_runtime_marks_nothing() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) =
            group_over(&thread, Err(platform_script_error("injected".to_owned())));
        let mut tasks = task::JoinSet::new();
        tasks.spawn_local(async {});
        let finished = tasks
            .join_next_with_id()
            .await
            .expect("the task was spawned");

        crate::main::finish_view(&context, finished, &mut rustc_hash::FxHashMap::default());
        for _ in 0..8 {
            task::yield_now().await;
        }

        assert_eq!(
            thread.run(|| ()).await,
            Some(()),
            "the queue ran past the bump"
        );
    });
}

#[test]
fn a_pages_own_entries_never_wake_its_clock_task() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let (outbox, mut view) = detached_outbox(Arc::new(NoWakeup));
        let page = Page::new(
            Rc::clone(&context),
            outbox,
            bound_metrics(CREATE_VIEWPORT),
            Arc::default(),
            view.token.clone(),
        );
        open_realm(&page, LISTENING_BOX, "app:///main.js").await;
        for _ in 0..TURNS {
            if view.published.commit().is_some() {
                break;
            }
            task::yield_now().await;
        }
        while let Ok(notice) = view.notices.try_recv() {
            if let ViewNotice::Engine(EngineEvent::StartupFailed(error)) = notice {
                panic!("the page never booted: {error}");
            }
        }
        assert!(view.published.commit().is_some(), "the page booted");

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

        page.apply(vec![ToMain::DispatchEvent {
            target,
            name: "tap",
            payload: InputEventPayload::default(),
            release: None,
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
                EngineEvent::Panicked(error) => Some(error.message.to_string()),
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

#[test]
fn a_command_that_panics_is_reported_once_as_panicked_and_ends_the_view() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX).await;
        let booted = owned.events();
        assert!(
            !booted.iter().any(EngineEvent::is_fatal),
            "the page booted: {booted:?}"
        );

        owned
            .page
            .apply(vec![ToMain::Probe(Box::new(|_| {
                panic!("a command of the view panicked");
            }))])
            .await;

        assert!(owned.page.ended(), "the panic ended the view");
        assert!(owned.token.is_cancelled());
        let events = owned.events();
        let [EngineEvent::Panicked(error)] = events.as_slice() else {
            panic!("one Panicked and nothing else: {events:?}");
        };
        assert!(
            error.message.contains("the Lynx main thread panicked")
                && error.message.contains("a command of the view panicked"),
            "the report carries the payload: {error}"
        );
    });
}

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

        let Some(WorkerCommand::Start(mut background)) = workers.recv().await else {
            panic!("BTS starts")
        };
        owned.token.cancel();
        tokio::join!(owned.page.run_owner(), answer_disposal(&mut background));

        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            11,
            "the end acknowledged the pending frame post"
        );
        assert!(
            owned.page.armed_deadline().is_none(),
            "and withdrew the deadline the realm had armed"
        );
    });
}

#[test]
fn the_end_acknowledges_a_frame_post_that_has_not_reached_its_fence() {
    on_a_js_thread(|thread| async move {
        let (context, mut workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX).await;
        let serviced = owned.view.published.begin_frame_serviced();
        let fence = owned.page.applied.get() + 2;
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.5,
                seq: serviced + 1,
                fence
            })
        ));
        owned.page.apply(vec![ToMain::Posted]).await;
        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            serviced,
            "taken, and not applied ahead of the command sent before it"
        );

        let Some(WorkerCommand::Start(mut background)) = workers.recv().await else {
            panic!("BTS starts")
        };
        owned.token.cancel();
        tokio::join!(owned.page.run_owner(), answer_disposal(&mut background));

        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            serviced + 1,
            "the end acknowledged the frame post no burst had applied"
        );
    });
}

#[test]
fn a_startup_failure_acknowledges_the_frame_post_a_painter_is_blocked_on() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        let (completion, answer) = SourceCompletion::new(owned.token.clone());
        completion.complete(Err(unanswered_source().into()));
        let startup = RealmStartup {
            sheets: vec![StartupSource {
                url: "app:///a.css".to_owned(),
                answer,
            }],
            entry: "app:///main.js".to_owned(),
            ..RealmStartup::default()
        };
        crate::lifetime::run_job(&owned.page, JobKind::Command, move |page| {
            page.open_realm(ingredients(), startup);
            Some(())
        })
        .await;
        load_entry(
            Rc::clone(&owned.page),
            answered_entry(
                &format!(
                    "{ONE_BOX}
await new Promise((resolve) => lynx.requestAnimationFrame(resolve));
"
                ),
                "app:///main.js",
                &owned.token,
            ),
        )
        .await;
        let fence = owned.page.applied.get() + 1;
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.0,
                seq: 9,
                fence
            })
        ));
        owned
            .page
            .apply(vec![ToMain::Vsync(16.0), ToMain::Posted])
            .await;

        let events = owned.events();
        let failures: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::StartupFailed(error) => Some(error),
                _ => None,
            })
            .collect();
        let [LynxViewError::Script(error)] = failures.as_slice() else {
            panic!("one Script startup failure, got {events:?}");
        };
        let message = error.to_string();
        assert!(
            message.contains("loading stylesheet app:///a.css"),
            "{message}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, EngineEvent::ScriptFinished)),
            "boot never finished: {events:?}"
        );
        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            9,
            "the startup failure's end acknowledged the frame post its burst applied"
        );
        assert!(
            owned.page.armed_deadline().is_none(),
            "the end left no deadline armed"
        );
    });
}

#[test]
fn a_panic_acknowledges_the_frame_post_a_painter_is_blocked_on() {
    on_a_js_thread(|thread| async move {
        let (context, mut workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned.boot(ONE_BOX).await;
        let Some(WorkerCommand::Start(mut background)) = workers.recv().await else {
            panic!("BTS starts")
        };
        let serviced = owned.view.published.begin_frame_serviced();
        let fence = owned.page.applied.get() + 2;
        assert!(owned.scroll.post(
            [],
            Some(FramePost {
                now: 0.0,
                seq: serviced + 1,
                fence
            })
        ));
        owned.page.apply(vec![ToMain::Posted]).await;
        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            serviced,
            "taken, and not applied ahead of the command sent before it"
        );
        owned
            .page
            .spawn(async { panic!("a task of the view trapped") });
        tokio::join!(owned.page.run_owner(), answer_disposal(&mut background));

        let events = owned.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, EngineEvent::Panicked(_)))
                .count(),
            1,
            "the panic is reported once: {events:?}"
        );
        assert_eq!(
            owned.view.published.begin_frame_serviced(),
            serviced + 1,
            "the panic's end acknowledged the frame post no burst had applied"
        );
    });
}

enum Seen {
    Notice(ViewNotice),
    Published { commit: Option<u64>, serviced: u64 },
}

impl Seen {
    fn is_script_finished(&self) -> bool {
        matches!(
            self,
            Self::Notice(ViewNotice::Engine(EngineEvent::ScriptFinished))
        )
    }

    fn describe(&self) -> String {
        match self {
            Self::Notice(ViewNotice::Engine(event)) => format!("{event:?}"),
            Self::Notice(_) => "a notice".to_owned(),
            Self::Published { commit, serviced } => {
                format!("published commit {commit:?}, frame post {serviced}")
            }
        }
    }
}

struct RecordingRequester {
    link: Mutex<(
        mpsc::UnboundedReceiver<ViewNotice>,
        watch::Receiver<Published>,
    )>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl EventRequester for RecordingRequester {
    fn request_event(&self) {
        let mut link = self.link.lock().expect("no recording panicked");
        let (notices, frames) = &mut *link;
        let mut seen = self.seen.lock().expect("no recording panicked");
        while let Ok(notice) = notices.try_recv() {
            seen.push(Seen::Notice(notice));
        }
        let published = frames.borrow();
        seen.push(Seen::Published {
            commit: published.commit(),
            serviced: published.begin_frame_serviced,
        });
    }
}

#[test]
fn a_frame_post_pending_across_boot_is_acknowledged_after_the_boot_report_and_its_commit() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (notices, notice_receiver) = mpsc::unbounded_channel();
        let (frames, frame_receiver) = watch::channel(Published::default());
        let token = CancellationToken::new();
        let outbox = ViewOutbox::new(
            notices,
            frames,
            Arc::new(RecordingRequester {
                link: Mutex::new((notice_receiver, frame_receiver)),
                seen: Arc::clone(&seen),
            }),
            token.clone(),
            None,
            None,
            detached_base(),
            crate::performance::PipelineRegistry::detached(),
        );
        let (_metrics, metric_receiver) = watch::channel(Some(CREATE_VIEWPORT));
        let scroll = Arc::new(ScrollMailbox::default());
        let page = Page::new(
            context,
            outbox,
            metric_receiver,
            Arc::clone(&scroll),
            token.clone(),
        );
        let startup = RealmStartup {
            entry: "app:///main.js".to_owned(),
            ..RealmStartup::default()
        };
        crate::lifetime::run_job(&page, JobKind::Command, move |page| {
            page.open_realm(ingredients(), startup);
            Some(())
        })
        .await;
        load_entry(
            Rc::clone(&page),
            answered_entry(
                &format!(
                    "{ONE_BOX}
await new Promise((resolve) => lynx.requestAnimationFrame(resolve));
"
                ),
                "app:///main.js",
                &token,
            ),
        )
        .await;
        let fence = page.applied.get() + 2;
        assert!(scroll.post(
            [],
            Some(FramePost {
                now: 0.0,
                seq: 7,
                fence
            })
        ));
        page.apply(vec![ToMain::Posted]).await;
        let before_boot = {
            let seen = seen.lock().expect("no recording panicked");
            let log: Vec<String> = seen.iter().map(Seen::describe).collect();
            assert!(
                !seen.iter().any(|seen| {
                    seen.is_script_finished() || matches!(seen, Seen::Published { serviced: 7, .. })
                }),
                "boot is still open, and the post waits for the vsync sent before it: {log:#?}"
            );
            seen.iter()
                .filter_map(|seen| match seen {
                    Seen::Published { commit, .. } => Some(*commit),
                    Seen::Notice(_) => None,
                })
                .next_back()
                .flatten()
        };
        page.apply(vec![ToMain::Vsync(16.0)]).await;

        let seen = std::mem::take(&mut *seen.lock().expect("no recording panicked"));
        let log: Vec<String> = seen.iter().map(Seen::describe).collect();
        let finished = seen
            .iter()
            .position(Seen::is_script_finished)
            .unwrap_or_else(|| panic!("boot finished: {log:#?}"));
        let acknowledged = seen
            .iter()
            .position(|seen| matches!(seen, Seen::Published { serviced: 7, .. }))
            .unwrap_or_else(|| panic!("the frame post was acknowledged: {log:#?}"));
        assert!(
            finished < acknowledged,
            "ScriptFinished came before the acknowledgement: {log:#?}"
        );
        let Seen::Published { commit, .. } = &seen[acknowledged] else {
            unreachable!("the position was found by this pattern");
        };
        assert!(
            *commit > before_boot,
            "boot's commit was published before the acknowledgement: {log:#?}"
        );
    });
}

#[test]
fn a_burst_queued_behind_a_release_is_never_applied() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        let booted = harness.boot(ONE_BOX_WITH_UPDATE).await;

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

#[test]
fn an_entry_that_fails_to_load_fails_the_startup_with_the_fetchers_error() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("the realm never opened", |h| {
                !h.sources.is_empty() && h.view.published.commit().is_some()
            })
            .await;
        let opened = harness.view.published.commit();
        harness.refuse();
        harness
            .until("the view never ended", |h| h.owner.is_finished())
            .await;
        harness.turn().await;

        let failures: Vec<_> = harness
            .events
            .iter()
            .filter(|event| event.is_fatal() || matches!(event, EngineEvent::ScriptRunError(_)))
            .collect();
        let [EngineEvent::StartupFailed(LynxViewError::Resource(error))] = failures.as_slice()
        else {
            panic!("one Resource startup failure, got {:?}", harness.events);
        };
        assert_eq!(error.to_string(), unanswered_source().to_string());
        assert!(!harness.finished());
        assert_eq!(
            harness.view.published.commit(),
            opened,
            "nothing was committed after the failure"
        );
    });
}

#[test]
fn an_entry_answered_from_a_url_that_is_not_absolute_fails_the_startup_naming_it() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("the view never asked for its entry", |h| {
                !h.sources.is_empty()
            })
            .await;
        harness.answer_entry("main.js", "globalThis.ran = true;");
        harness
            .until("the view never ended", |h| h.owner.is_finished())
            .await;
        harness.turn().await;

        let failures: Vec<_> = harness
            .events
            .iter()
            .filter(|event| event.is_fatal() || matches!(event, EngineEvent::ScriptRunError(_)))
            .collect();
        let [EngineEvent::StartupFailed(LynxViewError::Script(error))] = failures.as_slice() else {
            panic!("one Script startup failure, got {:?}", harness.events);
        };
        assert!(
            error.message.contains("app:///main.js")
                && error.message.contains("\"main.js\"")
                && error.message.contains("not an absolute URL"),
            "{error}"
        );
        assert!(!harness.finished());
        assert!(harness.workers.try_recv().is_err(), "no worker was started");
    });
}

#[test]
fn a_view_on_a_runtime_that_was_never_built_fails_its_startup() {
    on_a_js_thread(|thread| async move {
        let (context, workers) =
            group_over(&thread, Err(platform_script_error("injected".to_owned())));
        let mut harness = Harness::new(context, workers);
        harness
            .until("the view never finished", |h| h.owner.is_finished())
            .await;
        harness.turn().await;

        let failures: Vec<_> = harness
            .events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::StartupFailed(error) => Some(error),
                _ => None,
            })
            .collect();
        let [LynxViewError::Script(error)] = failures.as_slice() else {
            panic!("one Script startup failure, got {:?}", harness.events);
        };
        assert!(error.message.contains("injected"), "{error}");
        assert!(!harness.finished());
        assert!(harness.workers.try_recv().is_err(), "no worker was started");
        let requests: Vec<_> = harness.sources.iter().map(|(request, _)| request).collect();
        assert!(
            matches!(
                requests.as_slice(),
                [SourceRequest::Module(url)] if url == "app:///main.js"
            ),
            "only the startup entry was asked for: {requests:?}"
        );
    });
}

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
                EngineEvent::Panicked(error) => Some(error.message.to_string()),
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
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut sources = ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US");
        sources.background_entry = Some("app:///background.js".into());
        let mut harness = Harness::serving(context, workers, sources);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer_entry("app:///main.js", "__CreatePage();");
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
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer_entry(
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
fn a_release_served_while_a_worker_event_waits_drops_the_rest_of_its_burst() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer_entry(
            "app:///main.js",
            r"__CreatePage();
lynx.getJSContext().addEventListener('park', () => {
    __AdoptStyleSheet(__LoadStyleSheet('CSS', '__Card__'));
});
lynx.getJSContext().addEventListener('after', () => {
    console.log('delivered after the release');
});",
        );
        harness
            .until("MTS did not render", |h| {
                h.view.published.commit().is_some()
            })
            .await;
        let background = harness.background_worker();
        harness
            .until("MTS boot did not finish", |h| h.finished())
            .await;
        for kind in ["park", "after"] {
            background
                .events
                .send(crate::background::WorkerEvent {
                    key: background.key,
                    payload: crate::background::WorkerPayload::Message(
                        crate::background::wire_value(&format!(
                            r#"{{type:"{kind}",data:null,origin:"JSContext"}}"#
                        )),
                    ),
                })
                .unwrap();
        }
        harness
            .until("the first event's handler never waited", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        harness.view.token.cancel();
        harness.background = Some(background);
        harness
            .until("the released view's owner never returned", |h| {
                h.owner.is_finished()
            })
            .await;
        (&mut harness.owner).await.expect("the owner did not trap");
        assert!(
            !harness.events.iter().any(|event| matches!(
                event,
                EngineEvent::ConsoleMessage { message, .. }
                    if message.contains("delivered after the release")
            )),
            "the event behind the waiting handler reached the released view: {:?}",
            harness.events
        );
    });
}

#[test]
fn a_bts_worker_that_fails_reports_worker_ended_and_leaves_boot_alone() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut sources = ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US");
        sources.background_entry = Some("app:///background.js".into());
        let mut harness = Harness::serving(context, workers, sources);
        harness
            .until("entry request", |h| !h.sources.is_empty())
            .await;
        harness.answer_entry("app:///main.js", "__CreatePage();");
        harness
            .until("MTS did not render", |h| {
                h.view.published.commit().is_some()
            })
            .await;
        let background = harness.background_worker();
        assert_eq!(
            background.url,
            crate::esm::BTS_MODULE_SPECIFIER,
            "boot's worker is the one at the URL that names the background thread"
        );
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
        let named = crate::ScriptSource::Worker {
            id: crate::WorkerId::from(background.key),
            url: "bobcat:bts".into(),
        };
        harness
            .until("the BTS failure was never reported", |h| {
                h.events.iter().any(
                    |e| matches!(e, EngineEvent::WorkerEnded { source, .. } if *source == named),
                )
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
                EngineEvent::WorkerEnded { source, error } if *source == named => {
                    Some(error.to_string())
                }
                EngineEvent::StartupFailed(_)
                | EngineEvent::ListenerFailed(_)
                | EngineEvent::ScriptRunError(_)
                | EngineEvent::WorkerThrew { .. }
                | EngineEvent::WorkerEnded { .. } => {
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
        harness.answer_entry(
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
            if url == "https://cdn.test/redirected/main.js/custom_section/CSS.css?version=2#entry"));
    });
}

#[test]
fn a_card_url_is_what_the_card_is_named_by() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                card_url: Some("cards/card.bundle".to_owned()),
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
            },
        );
        harness
            .until("the entry was not requested", |h| h.sources.len() == 1)
            .await;
        harness.answer_entry(
            "app:///main.js",
            r"
            import { Worker } from 'bobcat-internal';
            if (__Card__ !== 'app:///cards/card.bundle')
                throw Error('the card is not the card URL: ' + __Card__);
            __LoadStyleSheet('CSS', '__Card__');
            globalThis.worker = new Worker('./worker.js');
        ",
        );
        harness
            .until("the stylesheet was not requested", |h| {
                h.preloads.len() == 1
            })
            .await;
        let request = harness.preloads.pop().unwrap();
        assert!(
            matches!(request, SourceRequest::StyleSheet(ref url)
                if url == "app:///cards/card.bundle/custom_section/CSS.css"),
            "{request:?}"
        );
        let mut started = Vec::new();
        harness
            .until("the entry never started its Worker", |h| {
                while let Ok(WorkerCommand::Start(start)) = h.workers.try_recv() {
                    started.push(start.url);
                }
                started.iter().any(|url| url.ends_with("worker.js"))
            })
            .await;
        assert!(
            started.contains(&"app:///cards/worker.js".to_owned()),
            "{started:?}"
        );
        assert!(
            !harness
                .events
                .iter()
                .any(|event| event.is_fatal() || matches!(event, EngineEvent::ScriptRunError(_))),
            "{:?}",
            harness.events
        );
    });
}

#[test]
fn a_card_url_that_does_not_resolve_is_an_invalid_url() {
    let mut sources = ViewSources {
        card_url: Some("https://[".to_owned()),
        ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
    };
    let error = resolve_startup_urls(&mut sources).expect_err("the card URL does not resolve");
    assert!(
        matches!(error, crate::EngineError::InvalidUrl { ref url, .. } if url == "https://["),
        "{error:?}"
    );
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
        harness.post_frame(0.0, 7).expect("the view is serving");
        harness
            .until(
                "the acknowledgement never came while the entry was in flight",
                |h| h.view.published.begin_frame_serviced() == 7,
            )
            .await;
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

        harness.answer_entry("app:///main.js", ONE_BOX);
        harness
            .until("boot never finished once its entry arrived", |h| {
                h.finished()
            })
            .await;
        harness.background = Some(harness.background_worker());
    });
}

const CLASSED_BOX: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetClasses(box, 'box');
  __AppendElement(page, box);
};
";

async fn classed_box_size(harness: &mut Harness) -> (f32, f32) {
    let (probe, probed) = std::sync::mpsc::channel();
    harness
        .commands
        .send(ToMain::Probe(Box::new(move |document| {
            let page = document.document_element().id();
            let box_id = document.get(page).expect("the page is live").child_ids()[0];
            let size = document.rounded_layout(box_id).expect("laid out").size;
            let _ = probe.send((size.width, size.height));
        })))
        .expect("the view is serving");
    let mut size = None;
    harness
        .until("the probe never ran", |_| {
            size = probed.try_recv().ok();
            size.is_some()
        })
        .await;
    size.expect("the probe answered")
}

fn answer_classed_entry(harness: &mut Harness) {
    let entry = harness
        .sources
        .iter()
        .position(
            |(request, _)| matches!(request, SourceRequest::Module(url) if url == "app:///main.js"),
        )
        .expect("the entry request is outstanding");
    let (_, completion) = harness.sources.remove(entry);
    completion.complete(Ok(LoadedSource::Module {
        source: card_entry(CLASSED_BOX),
        url: "app:///main.js".to_owned(),
    }));
}

fn started_background(harness: &mut Harness) -> bool {
    if harness.background.is_none()
        && let Ok(WorkerCommand::Start(start)) = harness.workers.try_recv()
    {
        harness.background = Some(start);
    }
    harness.background.is_some()
}

#[test]
fn author_sheets_cascade_in_listed_order_and_boot_waits_for_all_of_them() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                style_sheets: vec!["app:///a.css".to_owned(), "app:///b.css".to_owned()],
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
            },
        );
        harness
            .until("the view never asked for its stylesheets", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        harness.answer_style_sheet_at("app:///b.css", ".box{width:30px;height:20px}");
        answer_classed_entry(&mut harness);
        harness
            .until("boot never reached its flush", started_background)
            .await;
        for _ in 0..8 {
            harness.turn().await;
        }
        assert!(
            !harness.finished() && harness.view.published.commit().is_none(),
            "boot finished or published with a.css still unanswered: {:?}",
            harness.events
        );

        harness.answer_style_sheet_at("app:///a.css", ".box{width:10px}");
        harness
            .until("boot never finished once both sheets arrived", |h| {
                h.finished()
            })
            .await;
        assert!(harness.view.published.commit().is_some());
        assert!(harness.startup_failure().is_none());
        assert_eq!(
            classed_box_size(&mut harness).await,
            (30.0, 20.0),
            "both sheets are mounted, and b.css, listed last, wins the width"
        );
    });
}

#[test]
fn boot_publishes_nothing_until_a_withheld_sheet_arrives() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                style_sheets: vec!["app:///a.css".to_owned()],
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
            },
        );
        harness
            .until("the view never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        answer_classed_entry(&mut harness);
        harness
            .until("boot never reached its flush", started_background)
            .await;
        harness.post_frame(0.0, 4).expect("the view is serving");
        for _ in 0..8 {
            harness.turn().await;
        }
        assert!(
            harness.view.published.commit().is_none(),
            "a frame was published without the listed sheet"
        );
        assert!(
            !harness.finished(),
            "boot finished with its sheet outstanding"
        );
        assert_eq!(
            harness.view.published.begin_frame_serviced(),
            0,
            "the frame post is a job behind the parked flush"
        );

        harness.answer_style_sheet_at("app:///a.css", ".box{width:10px;height:20px}");
        harness
            .until("boot never finished once the sheet arrived", |h| {
                h.finished()
            })
            .await;
        assert!(
            harness.view.published.commit().is_some(),
            "ScriptFinished follows the frame boot published"
        );
        assert!(harness.startup_failure().is_none());
        harness
            .until("the frame post was never acknowledged", |h| {
                h.view.published.begin_frame_serviced() == 4
            })
            .await;
        assert_eq!(
            classed_box_size(&mut harness).await,
            (10.0, 20.0),
            "the first frame is laid out with the sheet mounted"
        );
    });
}

#[test]
fn a_listed_sheet_that_fails_to_load_fails_the_boot_naming_it() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                style_sheets: vec!["app:///a.css".to_owned()],
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
            },
        );
        harness
            .until("the view never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        answer_classed_entry(&mut harness);
        harness
            .until("boot never reached its flush", started_background)
            .await;
        harness.refuse_style_sheet();
        harness
            .until("the sheet failure never reached the embedder", |h| {
                h.startup_failure().is_some()
            })
            .await;
        let failures: Vec<_> = harness
            .events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::StartupFailed(error) => Some(error),
                _ => None,
            })
            .collect();
        let [LynxViewError::Script(error)] = failures.as_slice() else {
            panic!("one Script startup failure, got {:?}", harness.events);
        };
        let message = error.to_string();
        assert!(
            message.contains("loading stylesheet app:///a.css"),
            "{message}"
        );
        assert!(!harness.finished());
        assert!(
            harness.view.published.commit().is_none(),
            "nothing was published without the sheet"
        );
    });
}

#[test]
fn a_listed_sheet_that_app_code_settles_first_still_fails_the_boot() {
    for caught in [false, true] {
        on_a_js_thread(move |thread| async move {
            let (context, workers) = group(&thread);
            let mut harness = Harness::serving(
                context,
                workers,
                ViewSources {
                    style_sheets: vec!["app:///a.css".to_owned()],
                    ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
                },
            );
            harness
                .until("the view never asked for its stylesheet", |h| {
                    h.wants_a_style_sheet()
                })
                .await;
            harness.refuse_style_sheet();
            let adoption = "__AdoptStyleSheet(__LoadStyleSheet('CSS', '__Card__'));";
            let entry = if caught {
                format!("{ONE_BOX}\ntry {{ {adoption} }} catch {{}}")
            } else {
                format!("{ONE_BOX}\n{adoption}")
            };
            harness.answer_entry("app:///main.js", &entry);
            harness
                .until("the sheet failure never reached the embedder", |h| {
                    h.startup_failure().is_some()
                })
                .await;
            harness.turn().await;

            let failures: Vec<_> = harness
                .events
                .iter()
                .filter(|event| {
                    event.is_fatal()
                        || matches!(
                            event,
                            EngineEvent::ScriptRunError(_) | EngineEvent::ScriptFinished
                        )
                })
                .collect();
            let [
                EngineEvent::ScriptRunError(reported),
                EngineEvent::StartupFailed(LynxViewError::Script(error)),
            ] = failures.as_slice()
            else {
                panic!(
                    "caught: {caught}: one ScriptRunError, then one Script startup failure, \
                     got {:?}",
                    harness.events
                );
            };
            assert!(
                error.message.contains("loading stylesheet app:///a.css"),
                "{error}"
            );
            assert_eq!(reported.message, error.message, "caught: {caught}");
            assert!(
                harness.view.published.commit().is_none(),
                "nothing was published without the sheet"
            );
        });
    }
}

#[test]
fn an_entry_a_worker_event_resumes_reports_its_throw_before_boots_failure() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::serving(
            context,
            workers,
            ViewSources {
                style_sheets: vec!["app:///a.css".to_owned()],
                ..ViewSources::new("app:///", "app:///main.js", SCREEN, "en-US")
            },
        );
        harness
            .until("the view never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        harness.refuse_style_sheet();
        harness.answer_entry(
            "app:///main.js",
            r"
            import { Worker } from 'bobcat-internal';
            const worker = new Worker('./worker.js');
            await new Promise(resolve => { worker.onmessage = resolve; });
            throw Error('the entry threw');
            ",
        );
        let mut worker = None;
        harness
            .until("the entry never started its Worker", |h| {
                if let Ok(WorkerCommand::Start(start)) = h.workers.try_recv() {
                    worker = Some(start);
                }
                worker.is_some()
            })
            .await;
        let worker = worker.expect("the entry's Worker started");
        worker
            .events
            .send(crate::background::WorkerEvent {
                key: worker.key,
                payload: crate::background::WorkerPayload::Message(crate::background::wire_value(
                    "'ready'",
                )),
            })
            .unwrap();
        harness
            .until("the sheet failure never reached the embedder", |h| {
                h.startup_failure().is_some()
            })
            .await;
        harness.turn().await;

        let failures: Vec<_> = harness
            .events
            .iter()
            .filter(|event| {
                event.is_fatal()
                    || matches!(
                        event,
                        EngineEvent::ListenerFailed(_)
                            | EngineEvent::ScriptRunError(_)
                            | EngineEvent::ScriptFinished
                    )
            })
            .collect();
        let [
            EngineEvent::ListenerFailed(thrown),
            EngineEvent::StartupFailed(LynxViewError::Script(error)),
        ] = failures.as_slice()
        else {
            panic!(
                "one ListenerFailed, then one Script startup failure, got {:?}",
                harness.events
            );
        };
        assert!(thrown.message.contains("the entry threw"), "{thrown}");
        assert!(
            error.message.contains("loading stylesheet app:///a.css"),
            "{error}"
        );
    });
}

#[test]
fn a_synchronous_adoption_parks_javascript_and_leaves_a_siblings_tasks_running() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut adopting = Harness::new(Rc::clone(&context), workers);
        adopting
            .until("A never asked for its entry", |h| !h.sources.is_empty())
            .await;
        adopting.answer_entry("app:///main.js", ADOPTING_BOX);
        adopting
            .until("A never adopted a stylesheet", |h| h.wants_a_style_sheet())
            .await;

        let mut loading = Harness::sibling(
            context,
            ViewSources {
                style_sheets: vec!["app:///b.css".to_owned()],
                ..ViewSources::new("app:///", "app:///b.js", SCREEN, "en-US")
            },
        );
        loading
            .until("B never asked for its stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;
        loading.answer_style_sheet(".box{width:10px}");
        loading.answer_entry("app:///b.js", ONE_BOX);
        loading.post_frame(0.0, 4).expect("B is still serving");
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
            .until("B's frame post was never acknowledged", |h| {
                h.view.published.begin_frame_serviced() == 4
            })
            .await;
    });
}

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
        harness.answer_entry("app:///main.js", ADOPTING_BOX);
        harness
            .until("the entry never adopted a stylesheet", |h| {
                h.wants_a_style_sheet()
            })
            .await;

        harness.view.token.cancel();
        harness
            .until("boot never went on to start the BTS", |h| {
                !h.workers.is_empty()
            })
            .await;
        harness.background = Some(harness.background_worker());
        harness
            .until("the released view's owner never returned", |h| {
                h.owner.is_finished()
            })
            .await;
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
                EngineEvent::StartupFailed(_)
                    | EngineEvent::ScriptRunError(_)
                    | EngineEvent::Panicked(_)
            )),
            "a release is not a failure, so nothing is reported"
        );
    });
}

#[test]
fn a_passed_deadline_queues_one_settle_while_a_sibling_holds_the_thread() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
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
        adopting.answer_entry("app:///main.js", ADOPTING_BOX);
        adopting
            .until("A never adopted a stylesheet", |h| h.wants_a_style_sheet())
            .await;

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

fn console_messages(events: &[EngineEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::ConsoleMessage { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

async fn child_realms(page: &Rc<Page>) -> Option<usize> {
    owner::enter(page, JobKind::Command, |runtime, _| runtime.child_realms()).await
}

#[test]
fn a_shadow_realm_timer_fires_in_a_later_epilogue_of_its_owner() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .boot(&format!(
                "{ONE_BOX}
globalThis.realm = new ShadowRealm();
await realm.importValue('bobcat:timers', '__BobcatRunTimer');
realm.evaluate('report => {{ setTimeout(() => report(\"fired\"), 30); }}')(
  message => console.log(message),
);
console.log('armed');
"
            ))
            .await;
        let mut events = owned.events();
        assert_eq!(
            console_messages(&events).first().map(String::as_str),
            Some("armed"),
            "{events:?}"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !console_messages(&events)
            .iter()
            .any(|message| message == "fired")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the child's timer never fired: {events:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            task::yield_now().await;
            events.extend(owned.events());
        }
        assert!(
            !events.iter().any(EngineEvent::is_fatal),
            "the child's timer ended nothing: {events:?}"
        );
        assert_eq!(child_realms(&owned.page).await, Some(1));
    });
}

#[test]
fn a_shadow_realm_rejection_is_reported_once_and_ends_nothing() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        let booted = owned
            .boot(&format!(
                "{ONE_BOX_WITH_UPDATE}
new ShadowRealm().evaluate('Promise.reject(new Error(\"the child failed\")); undefined');
"
            ))
            .await;
        for _ in 0..8 {
            task::yield_now().await;
        }
        let failures: Vec<String> = owned
            .events()
            .into_iter()
            .filter_map(|event| match event {
                EngineEvent::ScriptRunError(error) => Some(error.message.to_string()),
                _ if event.is_fatal() => panic!("the rejection ended the view: {event:?}"),
                _ => None,
            })
            .collect();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures[0].starts_with("in a ShadowRealm: ")
                && failures[0].contains("the child failed"),
            "{}",
            failures[0]
        );
        assert!(!owned.page.ended(), "the rejection did not end the view");
        assert_eq!(child_realms(&owned.page).await, Some(0));
        owned.page.apply(vec![update_command(1)]).await;
        assert_eq!(owned.view.published.commit(), Some(booted + 1));
    });
}

#[test]
fn a_shadow_realm_nothing_needs_is_released_by_its_owners_epilogue() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .boot(&format!(
                "{ONE_BOX}
globalThis.idle = new ShadowRealm();
idle.evaluate('1');
globalThis.waiting = new ShadowRealm();
await waiting.importValue('bobcat:timers', '__BobcatRunTimer');
waiting.evaluate('setTimeout(() => {{}}, 3600000); undefined');
globalThis.updatePage = () => {{
  globalThis.idle = undefined;
  globalThis.waiting = undefined;
}};
"
            ))
            .await;
        assert_eq!(child_realms(&owned.page).await, Some(2));

        owned.page.apply(vec![update_command(1)]).await;
        owner::enter(&owned.page, JobKind::Command, |runtime, js| {
            runtime.core().engine.collect_garbage(js)
        })
        .await
        .expect("the page is live")
        .expect("the collection runs");
        assert_eq!(
            child_realms(&owned.page).await,
            Some(1),
            "the idle child is released and the one with a timer armed stays"
        );
        let events = owned.events();
        assert!(
            !events.iter().any(EngineEvent::is_fatal),
            "the release ended nothing: {events:?}"
        );
    });
}

#[test]
fn a_closed_child_is_freed_by_the_closing_entrys_epilogue() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .boot(&format!(
                "{ONE_BOX}
globalThis.realm = new ShadowRealm();
const close = await new Promise(resolve => realm.evaluate(`ready => {{
  import('bobcat-internal:host').then(H => ready(() => H.closeRealm()));
}}`)(resolve));
// Called with no receiver: a wrapped function refuses an object `this`.
globalThis.updatePage = () => {{
  close();
}};
"
            ))
            .await;
        assert_eq!(child_realms(&owned.page).await, Some(1));

        owned.page.apply(vec![update_command(1)]).await;
        assert_eq!(
            child_realms(&owned.page).await,
            Some(0),
            "the retired child goes though the page still refers to it"
        );
        let events = owned.events();
        assert!(
            !events.iter().any(EngineEvent::is_fatal),
            "the release ended nothing: {events:?}"
        );
    });
}

#[test]
fn a_shadow_realms_module_loads_and_future_settles_land_in_the_child() {
    on_a_js_thread(|thread| async move {
        let (context, workers) = group(&thread);
        let mut harness = Harness::new(context, workers);
        harness
            .boot(&format!(
                "{ONE_BOX}
globalThis.realm = new ShadowRealm();
realm.importValue('app:///child.js', 'value').then(value => console.log('loaded ' + value));
await realm.importValue('bobcat:future', 'Future');
realm.evaluate(`report => {{
  import('bobcat-internal:host').then(({{ testFuture }}) => import('bobcat:future').then(({{ Future }}) =>
    new Future(testFuture(5, 'settled', false)).then(report)));
}}`)(value => console.log('future ' + value));
"
            ))
            .await;
        harness
            .until("the child never asked for its module", |harness| {
                harness.sources.iter().any(|(request, _)| {
                    matches!(request, SourceRequest::Module(url) if url == "app:///child.js")
                })
            })
            .await;
        harness.answer("app:///child.js", "export const value = 7;");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let messages = console_messages(&harness.events);
            if ["loaded 7", "future settled"]
                .iter()
                .all(|wanted| messages.iter().any(|message| message == wanted))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the child's load or settle never landed: {:?}",
                harness.events
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            harness.turn().await;
        }
        assert!(
            !harness
                .events
                .iter()
                .any(|event| event.is_fatal() || matches!(event, EngineEvent::ScriptRunError(_))),
            "{:?}",
            harness.events
        );
    });
}

#[test]
fn the_epilogue_replays_the_canvases_an_entry_drew() {
    on_a_js_thread(|thread| async move {
        let (context, _workers) = group(&thread);
        let mut owned = OwnedPage::new(context);
        owned
            .boot(&format!(
                "{ONE_BOX}
import * as H from 'bobcat-internal:host';
globalThis.H = H;
globalThis.canvas = H.canvasCreate(8, 8);
H.canvasFillRect(canvas, 0, 0, 4, 4);
"
            ))
            .await;
        let state = |owned: &OwnedPage| {
            owner::enter(&owned.page, JobKind::Command, |runtime, _| {
                let canvases = &runtime.core().canvases;
                (canvases.has_pending_draws(), canvases.settled.get())
            })
        };
        let (pending, mut settled) = state(&owned).await.expect("the page is live");
        assert!(!pending, "boot's draw was settled");
        assert_eq!(settled, 1, "boot drew the one canvas");
        for tick in 0..3 {
            owner::enter(&owned.page, JobKind::Command, move |runtime, js| {
                runtime.evaluate_module(
                    js,
                    &format!("H.canvasFillRect(canvas, {tick}, 0, 1, 1);"),
                    &format!("app:///draw-{tick}.js"),
                    "drawing",
                )
            })
            .await
            .expect("the page is live")
            .expect("the draw runs");
            let (pending, now) = state(&owned).await.expect("the page is live");
            assert!(!pending, "entry {tick} left draws recorded");
            assert_eq!(now, settled + 1, "entry {tick} settled its canvas once");
            settled = now;
        }
        owner::enter(&owned.page, JobKind::Command, |runtime, js| {
            runtime.evaluate_module(
                js,
                "H.canvasGetNumber(canvas, 0); H.canvasGetTransform(canvas);",
                "app:///read.js",
                "reading",
            )
        })
        .await
        .expect("the page is live")
        .expect("the read runs");
        let (_, now) = state(&owned).await.expect("the page is live");
        assert_eq!(now, settled, "an entry that drew nothing settled nothing");
    });
}
