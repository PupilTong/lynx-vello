//! The group's worker realms, driven the way the rest of the engine drives
//! them: a `Start` per worker carrying its own channels, and the test playing
//! both the realm that names a worker and the host that fetches for it.
//!
//! Nothing here builds a view, a document or a `bobcat-main`. That is the
//! point — a worker realm reaches none of them, so a test that had to build
//! one would be testing something else.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;
use terminus::link::clock::{Alignment, alignment_channel};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    WorkerCommand, WorkerEvent, WorkerHome, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart,
    wire_json, wire_value,
};
use crate::clock::ClockInstant;
use crate::esm::BTS_MODULE_SPECIFIER;
use crate::link::{HostOutbox, ViewNotice, block_on_deadline, detached_base};
use crate::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, RetryAdvice,
    SourceCompletion, SourceRequest, StyleSheetSource,
};

impl WorkerHome {
    pub(crate) fn start_aligned_for_test() -> (Self, Option<Alignment>) {
        let (clock, clock_end) = alignment_channel();
        let home = Self::start(clock_end).expect("the worker thread starts");
        let alignment = block_on_deadline(clock.answer(PATIENCE), ClockInstant::now() + PATIENCE)
            .expect("the worker thread runs");
        (home, alignment)
    }

    pub(crate) fn with_entry_for_test(entry: (String, String)) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let trapped = Arc::new(AtomicBool::new(false));
        let thread = super::ThreadBuilder::new()
            .name("bobcat-test-workers".into())
            .spawn({
                let trapped = Arc::clone(&trapped);
                move || super::thread::run_with_entry(receiver, entry, &trapped)
            })
            .unwrap();
        Self {
            commands,
            trapped,
            thread: crate::threads::ThreadJoin::new(thread),
        }
    }
}

const PATIENCE: Duration = Duration::from_secs(30);

fn wire(data: &str) -> HostValue {
    HostValue::String(data.to_owned())
}

struct View {
    messages: FxHashMap<WorkerKey, mpsc::UnboundedSender<WorkerMessage>>,
    events: mpsc::UnboundedSender<WorkerEvent>,
    incoming: mpsc::UnboundedReceiver<WorkerEvent>,
    token: CancellationToken,
    notices: mpsc::UnboundedSender<ViewNotice>,
    sources: mpsc::UnboundedReceiver<ViewNotice>,
    passed: VecDeque<(String, SourceCompletion)>,
    base: Arc<Url>,
}

impl View {
    fn new() -> Self {
        let (events, incoming) = mpsc::unbounded_channel();
        let (notices, sources) = mpsc::unbounded_channel();
        Self {
            messages: FxHashMap::default(),
            events,
            incoming,
            token: CancellationToken::new(),
            notices,
            sources,
            passed: VecDeque::new(),
            base: detached_base(),
        }
    }

    fn next(&mut self) -> WorkerEvent {
        block_on_deadline(self.incoming.recv(), ClockInstant::now() + PATIENCE)
            .flatten()
            .expect("the worker thread has something to report")
    }

    fn message(&mut self) -> HostValue {
        match self.next().payload {
            WorkerPayload::Message(data) => data,
            WorkerPayload::Errored(error) | WorkerPayload::Failed(error) => {
                panic!("expected a message, got {}", error.message)
            }
            WorkerPayload::Closed => panic!("expected a message, the worker closed"),
        }
    }

    fn source(&mut self) -> (String, SourceCompletion) {
        if let Some(passed) = self.passed.pop_front() {
            return passed;
        }
        self.arriving(ClockInstant::now() + PATIENCE)
            .expect("the worker requested a module")
    }

    fn request_for(&mut self, url: &str) -> SourceCompletion {
        if let Some(position) = self
            .passed
            .iter()
            .position(|(requested, _)| requested == url)
        {
            let (_, completion) = self.passed.remove(position).expect("the position found");
            return completion;
        }
        let deadline = ClockInstant::now() + PATIENCE;
        loop {
            let (requested, completion) = self
                .arriving(deadline)
                .unwrap_or_else(|| panic!("a worker asked its host for {url}"));
            if requested == url {
                return completion;
            }
            self.passed.push_back((requested, completion));
        }
    }

    fn arriving(&mut self, deadline: ClockInstant) -> Option<(String, SourceCompletion)> {
        match block_on_deadline(self.sources.recv(), deadline).flatten()? {
            ViewNotice::RequestSource {
                request: SourceRequest::Module(url),
                completion,
            } => Some((url, completion)),
            _ => panic!("a worker only requests module sources"),
        }
    }

    fn asked_for_nothing_more(&mut self) -> bool {
        self.passed.is_empty() && self.sources.try_recv().is_err()
    }
}

struct Constructed {
    view: usize,
    url: String,
    token: CancellationToken,
}

struct Group {
    home: WorkerHome,
    commands: Option<mpsc::UnboundedSender<WorkerCommand>>,
    views: Vec<View>,
    workers: FxHashMap<WorkerKey, Constructed>,
    next_key: Cell<u64>,
}

impl Group {
    fn new() -> Self {
        let (home, _) = WorkerHome::start_aligned_for_test();
        Self {
            commands: Some(home.commands()),
            home,
            views: vec![View::new(), View::new()],
            workers: FxHashMap::default(),
            next_key: Cell::new(1),
        }
    }

    fn tell(&self, command: WorkerCommand) {
        let _ = self
            .commands
            .as_ref()
            .expect("the group holds its sender until it is dropped")
            .send(command);
    }

    fn construct(&mut self, view: usize, name: &str) -> WorkerKey {
        let url = format!("app:///requested/{}.js", self.next_key.get());
        self.construct_requesting(view, name, &url)
    }

    fn construct_requesting(&mut self, view: usize, name: &str, url: &str) -> WorkerKey {
        self.start_worker(view, name, url, Vec::new())
    }

    fn construct_engine_name(&mut self, view: usize, url: &str) -> WorkerKey {
        self.start_worker(view, "", url, Vec::new())
    }

    fn construct_background(&mut self, view: usize) -> WorkerKey {
        self.construct_background_after(view, Vec::new())
    }

    fn construct_background_after(&mut self, view: usize, posted: Vec<WorkerMessage>) -> WorkerKey {
        self.start_worker(view, "lynx-bg", BTS_MODULE_SPECIFIER, posted)
    }

    fn background_running(&mut self, view: usize, source: &str) -> WorkerKey {
        let key = self.construct_background(view);
        let url = format!("app:///bts{}.js", key.get());
        self.send(
            key,
            WorkerMessage::post(wire_value(&format!(
                r#"({{bobcat: "runtime", method: "initialize", entry: "{url}"}})"#
            ))),
        );
        self.views[view]
            .request_for(&url)
            .complete(Ok(LoadedSource::Module {
                source: source.to_owned(),
                url,
            }));
        key
    }

    fn global_event(&self, key: WorkerKey, name: &str) {
        self.send(
            key,
            WorkerMessage::post(wire_value(&format!(
                r#"({{bobcat: "runtime", method: "sendGlobalEvent", name: "{name}", args: []}})"#
            ))),
        );
    }

    fn start_worker(
        &mut self,
        view: usize,
        name: &str,
        url: &str,
        posted: Vec<WorkerMessage>,
    ) -> WorkerKey {
        let key = WorkerKey::new(self.next_key.get());
        self.next_key.set(key.get() + 1);
        let (messages, incoming) = mpsc::unbounded_channel();
        for message in posted {
            let _ = messages.send(message);
        }
        let token = CancellationToken::new();
        let sources = HostOutbox::new(
            self.views[view].notices.clone(),
            Arc::new(crate::NoWakeup),
            token.clone(),
            None,
            Arc::clone(&self.views[view].base),
            crate::performance::PipelineRegistry::detached(),
        );
        self.tell(WorkerCommand::Start(WorkerStart {
            key,
            name: name.to_owned(),
            url: url.to_owned(),
            messages: incoming,
            events: self.views[view].events.clone(),
            token: token.clone(),
            sources,
        }));
        self.views[view].messages.insert(key, messages);
        self.workers.insert(
            key,
            Constructed {
                view,
                url: url.to_owned(),
                token,
            },
        );
        key
    }

    fn script_request(&mut self, key: WorkerKey) -> SourceCompletion {
        let Constructed { view, url, .. } = &self.workers[&key];
        self.views[*view].request_for(url)
    }

    fn answer(&mut self, key: WorkerKey, url: &str, source: &str) {
        self.script_request(key).complete(Ok(LoadedSource::Module {
            source: source.to_owned(),
            url: url.to_owned(),
        }));
    }

    fn wait_for_end(&self, key: WorkerKey, what: &str) {
        let deadline = ClockInstant::now() + PATIENCE;
        while !self.workers[&key].token.is_cancelled() {
            assert!(ClockInstant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn start(&mut self, source: &str) -> WorkerKey {
        let key = self.construct(0, "");
        self.answer(key, "app:///worker.js", source);
        key
    }

    fn post(&self, key: WorkerKey, data: &str) {
        self.send(key, WorkerMessage::post(wire(data)));
    }

    fn post_value(&self, key: WorkerKey, data: HostValue) {
        self.send(key, WorkerMessage::post(data));
    }

    fn send(&self, key: WorkerKey, message: WorkerMessage) {
        for view in &self.views {
            if let Some(messages) = view.messages.get(&key) {
                let _ = messages.send(message);
                return;
            }
        }
        panic!("no view holds worker {key:?}");
    }

    fn terminate(&mut self, key: WorkerKey) {
        for view in &mut self.views {
            if let Some(messages) = view.messages.remove(&key) {
                let _ = messages.send(WorkerMessage::Terminate);
                let _ = messages.send(WorkerMessage::post(wire("after terminate")));
                drop(messages);
                return;
            }
        }
    }

    fn release(&mut self, view: usize) {
        self.views[view].messages.clear();
    }

    fn cancel(&mut self, view: usize) {
        self.views[view].token.cancel();
    }

    fn wait_for_workers_to_end(&self, view: usize, patience: Duration, what: &str) {
        let deadline = ClockInstant::now() + patience;
        while self.views[view].events.strong_count() > 1 {
            assert!(ClockInstant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn next(&mut self, view: usize) -> WorkerEvent {
        self.views[view].next()
    }

    fn message(&mut self, view: usize) -> HostValue {
        self.views[view].message()
    }

    fn quiet(&mut self) {
        let probe = self.construct(0, "");
        self.answer(
            probe,
            "app:///probe.js",
            "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(\"probe\");",
        );
        let event = self.next(0);
        assert_eq!(event.key, probe, "something else was still to be reported");
        let expected = wire("probe");
        assert!(matches!(event.payload, WorkerPayload::Message(ref data) if *data == expected));
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.views.clear();
        drop(self.commands.take());
    }
}

#[test]
fn a_worker_answers_what_the_group_posts_and_carries_the_name_it_was_given() {
    let mut group = Group::new();
    let key = group.construct(0, "counter");
    group.answer(
        key,
        "app:///w.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         onmessage = (event) => postMessage(`${name}:${event.data}`);",
    );
    group.post(key, "ping");
    assert_eq!(group.message(0), wire("counter:ping"));
}

#[test]
fn a_structured_value_survives_a_round_trip_through_two_worker_realms() {
    let mut group = Group::new();
    group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         const value = { tag: 'payload', missing: undefined, nan: NaN,
  at: new Date(1700000000123), bytes: new Uint8Array([1, 2, 255]),
  big: 9007199254740993n };
value.self = value;
postMessage(value);",
    );
    let posted = group.message(0);
    assert!(
        matches!(posted, HostValue::Structured(_)),
        "an object crosses as a structured clone rather than as text"
    );

    let echo = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; addEventListener('message', (event) => {
  const d = event.data;
  postMessage([
    typeof d, d.tag, 'missing' in d, d.missing === undefined, Number.isNaN(d.nan),
    d.at instanceof Date && d.at.getTime() === 1700000000123,
    d.bytes instanceof Uint8Array && Array.from(d.bytes).join(',') === '1,2,255',
    d.big === 9007199254740993n, d.self === d,
  ].join(':'));
});",
    );
    group.post_value(echo, posted);
    assert_eq!(
        group.message(0),
        wire("object:payload:true:true:true:true:true:true:true")
    );
}

#[test]
fn what_is_posted_before_the_script_arrives_is_delivered_in_order() {
    let mut group = Group::new();
    let key = group.construct(0, "");
    group.post(key, "first");
    group.post(key, "second");
    group.answer(
        key,
        "app:///w.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         onmessage = (event) => postMessage(event.data);",
    );
    assert_eq!(group.message(0), wire("first"));
    assert_eq!(group.message(0), wire("second"));
}

#[test]
fn a_module_the_script_imports_statically_reads_the_workers_name() {
    let mut group = Group::new();
    let key = group.construct(0, "named");
    group.answer(
        key,
        "app:///w.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         import { seen } from './seen.js';\npostMessage(seen);",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///seen.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "export const seen = self.name;".to_owned(),
        url,
    }));
    assert_eq!(group.message(0), wire("named"));
}

#[test]
fn a_script_answered_from_another_url_runs_as_that_url_and_is_asked_for_once() {
    let mut group = Group::new();
    let key = group.construct_requesting(0, "", "app:///worker.js");
    group.answer(
        key,
        "https://cdn.test/redirected/worker.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(import.meta.url);\n\
         onmessage = (event) => postMessage(event.data);",
    );
    assert_eq!(
        group.message(0),
        wire("https://cdn.test/redirected/worker.js")
    );
    group.post(key, "echo");
    assert_eq!(group.message(0), wire("echo"));
    assert!(
        group.views[0].asked_for_nothing_more(),
        "the script was asked for once"
    );
}

#[test]
fn a_post_and_a_terminate_while_the_answered_script_still_imports_end_the_worker() {
    let mut group = Group::new();
    let key = group.construct(0, "");
    group.answer(
        key,
        "app:///w.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         onmessage = (event) => postMessage(event.data);\nawait import('./held.js');",
    );
    let (url, held) = group.views[0].source();
    assert_eq!(url, "app:///held.js");
    group.post(key, "queued");
    group.terminate(key);
    held.complete(Ok(LoadedSource::Module {
        source: String::new(),
        url,
    }));
    group.quiet();
}

#[test]
fn a_bts_imports_its_entry_through_bobcat_bts_once_initialized() {
    let mut group = Group::new();
    let key = group.construct_background(0);
    group.send(
        key,
        WorkerMessage::post(wire_value(
            r#"({bobcat: "runtime", method: "initialize", entry: "app:///bts.js"})"#,
        )),
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///bts.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "import { lynx } from 'bobcat:bts-runtime';\n\
                 postMessage([self.name, typeof lynx.getNativeApp, import.meta.url].join(' '));"
            .to_owned(),
        url,
    }));
    assert_eq!(group.message(0), wire("lynx-bg function app:///bts.js"));
}

#[test]
fn an_initialize_posted_before_the_bts_starts_waits_for_bobcat_bts() {
    let mut group = Group::new();
    group.construct_background_after(
        0,
        vec![WorkerMessage::post(wire_value(
            r#"({bobcat: "runtime", method: "initialize", entry: "app:///bts.js"})"#,
        ))],
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///bts.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "postMessage('entry ran');".to_owned(),
        url,
    }));
    assert_eq!(group.message(0), wire("entry ran"));
}

#[test]
fn a_bts_reads_its_screen_and_native_modules_from_initialize() {
    let mut group = Group::new();
    let table = vec![
        crate::native_module::TableEntry::new("Echo", vec!["ping".to_owned(), "pong".to_owned()]),
        crate::native_module::TableEntry::new("Bare", Vec::new()),
    ];
    let key = group.construct_background(0);
    let table = serde_json::to_string(&crate::native_module::encode_table(&table)).unwrap();
    group.send(
        key,
        WorkerMessage::post(wire_value(&format!(
            r#"({{bobcat: "runtime", method: "initialize", updateData: {{}}, entry: "app:///bts.js",
                systemInfo: {{platform: "headless", pixelRatio: 3, pixelWidth: 1170, pixelHeight: 2532}},
                nativeModuleTable: {table}}})"#
        ))),
    );
    let (url, completion) = group.views[0].source();
    completion.complete(Ok(LoadedSource::Module {
        source: r"
            import { lynx, NativeModules, SystemInfo } from 'bobcat:bts-runtime';
            postMessage([
                SystemInfo.pixelRatio,
                SystemInfo.pixelWidth,
                SystemInfo.pixelHeight,
                lynx.SystemInfo === SystemInfo && globalThis.SystemInfo === SystemInfo,
                Object.isFrozen(SystemInfo),
                Object.keys(NativeModules),
                Object.keys(NativeModules.Echo),
                Object.keys(NativeModules.Bare),
                lynx.getApp().NativeModules === NativeModules,
            ]);
        "
        .to_owned(),
        url,
    }));
    assert_eq!(
        wire_json(&group.message(0)),
        r#"[3,1170,2532,true,true,["Echo","Bare"],["ping","pong"],[],true]"#
    );
}

#[test]
fn a_script_that_cannot_be_fetched_is_reported_once_and_leaves_a_terminable_worker() {
    let mut group = Group::new();
    group.views.push(View::new());
    let doomed = group.construct(2, "");
    group.post(doomed, "held");
    group.script_request(doomed).complete(Err(ResourceError {
        kind: ResourceErrorKind::NotFound,
        phase: ResourceErrorPhase::ReceiveHeaders,
        locator: None,
        message: "404".into(),
        retry: RetryAdvice::Never,
    }
    .into()));
    let event = group.next(2);
    assert_eq!(event.key, doomed);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("a script that could not be loaded is something the realm threw")
    };
    assert!(
        error.message.contains("loading a worker module") && error.message.contains("404"),
        "{}",
        error.message
    );
    group.post(doomed, "dropped");
    group.quiet();
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "the failed load was reported once, and neither post reached anything"
    );
    assert!(
        group.views[2].asked_for_nothing_more(),
        "the script was asked for once"
    );
    assert!(
        group.views[2].events.strong_count() > 1,
        "the worker is still running"
    );
    group.terminate(doomed);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "its end reported nothing"
    );
    let key =
        group.start("import 'bobcat:worker'; import 'bobcat:timers'; postMessage(\"alive\");");
    assert_eq!(group.next(0).key, key);
}

#[test]
fn a_script_answered_with_a_stylesheet_is_reported_once_and_leaves_the_worker_running() {
    let mut group = Group::new();
    group.views.push(View::new());
    let key = group.construct(2, "");
    group
        .script_request(key)
        .complete(Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(
            "page { color: red; }".to_owned(),
        ))));
    let event = group.next(2);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("an answer that is not a script is a load the realm rejected")
    };
    assert!(
        error.message.contains("loading a worker module")
            && error.message.contains("the fetcher returned a stylesheet"),
        "{}",
        error.message
    );
    group.post(key, "dropped");
    group.quiet();
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "the refused answer was reported once, and the post reached nothing"
    );
    assert!(
        group.views[2].asked_for_nothing_more(),
        "the script was asked for once"
    );
    assert!(
        group.views[2].events.strong_count() > 1,
        "the worker is still running"
    );
    group.terminate(key);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "its end reported nothing"
    );
}

#[test]
fn a_script_answered_from_a_url_containing_nul_is_reported_once_and_leaves_the_worker_running() {
    let mut group = Group::new();
    group.views.push(View::new());
    let key = group.construct(2, "");
    group.answer(
        key,
        "app:///nul\0.js",
        "import 'bobcat:worker'; postMessage('ran');",
    );
    let event = group.next(2);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("a response URL that names no module is a load the realm rejected")
    };
    assert!(
        error.message.contains("loading a worker module") && error.message.contains("NUL"),
        "{}",
        error.message
    );
    group.quiet();
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "the refused answer was reported once, and the script did not run"
    );
    assert!(
        group.views[2].events.strong_count() > 1,
        "the worker is still running"
    );
    group.terminate(key);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
}

#[test]
fn a_worker_terminated_before_its_boot_job_asks_its_host_for_nothing() {
    let mut group = Group::new();
    group.views.push(View::new());
    group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         import { createRequire } from 'bobcat:module';
createRequire(import.meta.url)('./held.cjs');
postMessage('resumed');",
    );
    let (url, held) = group.views[0].source();
    assert_eq!(url, "app:///held.cjs");
    let ended = group.construct(2, "");
    group.terminate(ended);
    group.wait_for_end(ended, "the terminated worker ended");
    held.complete(Ok(LoadedSource::Module {
        source: String::new(),
        url,
    }));
    assert_eq!(group.message(0), wire("resumed"));
    group.wait_for_workers_to_end(2, PATIENCE, "the ended worker's task returned");
    assert!(
        group.views[2].asked_for_nothing_more(),
        "a worker that ended before its boot job asked its host for nothing"
    );
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "and reported nothing"
    );
}

#[test]
fn a_script_answered_with_a_font_is_reported_naming_the_module_and_the_answer() {
    let mut group = Group::new();
    let key = group.construct_requesting(0, "", "app:///font-worker.js");
    group.script_request(key).complete(Ok(LoadedSource::Font(
        terminus::link::FontBlob::from_static(b"not a script"),
    )));
    let event = group.next(0);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("an answer that is not a script is a load the realm rejected")
    };
    assert_eq!(
        &*error.message,
        "loading a worker module: TypeError: module 'app:///font-worker.js': \
         the fetcher returned a font for app:///font-worker.js"
    );
}

#[test]
fn an_import_answered_with_a_stylesheet_is_rejected_naming_the_answer() {
    let mut group = Group::new();
    group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         import('./dep.js').catch(error => postMessage(error.message));",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///dep.js");
    completion.complete(Ok(LoadedSource::StyleSheet(
        crate::resource::StyleSheetSource::Text(String::new()),
    )));
    assert_eq!(
        wire_json(&group.message(0)),
        r#""module 'app:///dep.js': the fetcher returned a stylesheet for app:///dep.js""#
    );
}

#[test]
fn a_worker_named_by_an_engine_url_is_loaded_by_its_realm_without_the_host() {
    let mut group = Group::new();
    group.views.push(View::new());
    let registered = group.construct_engine_name(1, "bobcat:timers");
    let refused = group.construct_engine_name(2, "bobcat:nope");
    let event = group.next(2);
    assert_eq!(event.key, refused);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("a refused import is something the realm threw")
    };
    assert!(
        error.message.contains("ReferenceError") && error.message.contains("'bobcat:nope'"),
        "{}",
        error.message
    );
    group.quiet();
    for view in [1, 2] {
        assert!(
            group.views[view].incoming.try_recv().is_err(),
            "the worker reported nothing more"
        );
        assert!(
            group.views[view].asked_for_nothing_more(),
            "the worker asked its host for nothing"
        );
    }
    for (view, key) in [(1, registered), (2, refused)] {
        assert!(
            group.views[view].events.strong_count() > 1,
            "the worker is still running"
        );
        group.terminate(key);
        group.wait_for_workers_to_end(view, PATIENCE, "the terminated worker ended");
    }
}

#[test]
fn a_trapped_worker_thread_fails_every_live_worker() {
    let mut group = Group::new();
    group.views.push(View::new());
    let first = group.construct(0, "");
    let second = group.construct(1, "");
    let parked = group.construct(2, "");
    group.answer(
        parked,
        "app:///parked.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         import { createRequire } from 'bobcat:module';
createRequire(import.meta.url)('./held.cjs');",
    );
    let (url, held) = group.views[2].source();
    let ended = group.construct(1, "");
    group.terminate(ended);
    group.wait_for_end(ended, "the terminated worker ended");
    group.tell(WorkerCommand::Panic);
    held.complete(Ok(LoadedSource::Module {
        source: String::new(),
        url,
    }));
    for (view, key) in [(0, first), (1, second), (2, parked)] {
        let event = group.next(view);
        assert_eq!(event.key, key);
        let WorkerPayload::Failed(error) = event.payload else {
            panic!("a worker on a trapped thread is over")
        };
        assert!(
            error.message.contains("the worker thread panicked"),
            "{}",
            error.message
        );
    }
    assert!(group.home.trapped().load(Ordering::Acquire));
    for view in [0, 1, 2] {
        group.wait_for_workers_to_end(
            view,
            PATIENCE,
            "the trapped thread dropped its clones of the view's event sender",
        );
        assert!(group.views[view].incoming.try_recv().is_err());
    }
}

#[test]
fn a_script_that_throws_on_load_leaves_a_worker_that_still_answers() {
    let mut group = Group::new();
    let key = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         onmessage = (event) => postMessage(event.data);
throw new Error(\"boom\");",
    );
    let event = group.next(0);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("HTML reports the exception and leaves the worker running")
    };
    assert!(error.message.contains("boom"), "{}", error.message);
    group.post(key, "still here");
    assert_eq!(group.message(0), wire("still here"));
}

#[test]
fn a_throw_at_a_scripts_top_level_is_reported_at_its_own_url_and_line() {
    let mut group = Group::new();
    let key = group.construct_requesting(0, "", "app:///thrower.js");
    group.answer(
        key,
        "app:///thrower.js",
        "import 'bobcat:worker';\nconst answer = 42;\nthrow new Error(`boom ${answer}`);",
    );
    let event = group.next(0);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("a script that throws on load is something the realm threw")
    };
    assert!(
        error.message.contains("loading a worker module") && error.message.contains("boom 42"),
        "{}",
        error.message
    );
    let location = error.location.expect("the throw has a location");
    assert_eq!(location.source.as_deref(), Some("app:///thrower.js"));
    assert_eq!(location.line, Some(3));
}

#[test]
fn a_worker_whose_script_imports_no_global_scope_drops_what_is_posted() {
    let mut group = Group::new();
    group.views.push(View::new());
    let key = group.construct(2, "");
    group.answer(key, "app:///bare.js", "import('./later.js');");
    let (url, _later) = group.views[2].source();
    assert_eq!(url, "app:///later.js");
    group.post(key, "dropped");
    group.quiet();
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "the post reached nothing and reported nothing"
    );
    assert!(
        group.views[2].events.strong_count() > 1,
        "the worker is still running"
    );
    group.terminate(key);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "its end reported nothing"
    );
}

#[test]
fn a_worker_whose_graph_failed_before_its_scope_ran_drops_what_is_posted() {
    let mut group = Group::new();
    group.views.push(View::new());
    let key = group.construct(2, "");
    group.answer(
        key,
        "app:///failing.js",
        "import 'bobcat:worker'; import './missing.js';",
    );
    let (url, completion) = group.views[2].source();
    assert_eq!(url, "app:///missing.js");
    completion.complete(Err(ResourceError {
        kind: ResourceErrorKind::NotFound,
        phase: ResourceErrorPhase::ReceiveHeaders,
        locator: None,
        message: "404".into(),
        retry: RetryAdvice::Never,
    }
    .into()));
    let event = group.next(2);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("a graph that fails to load is something the realm threw")
    };
    assert!(error.message.contains("404"), "{}", error.message);
    group.post(key, "dropped");
    group.quiet();
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "the failed load was reported once, and the post reached nothing"
    );
    group.terminate(key);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
}

#[test]
fn a_root_whose_dependency_fails_is_reported_once() {
    let mut group = Group::new();
    group.views.push(View::new());
    group.views.push(View::new());
    let missing = group.construct(2, "");
    group.answer(
        missing,
        "app:///needs-missing.js",
        "import 'bobcat:worker'; import './missing.js';",
    );
    let (url, completion) = group.views[2].source();
    assert_eq!(url, "app:///missing.js");
    completion.complete(Err(ResourceError {
        kind: ResourceErrorKind::NotFound,
        phase: ResourceErrorPhase::ReceiveHeaders,
        locator: None,
        message: "404".into(),
        retry: RetryAdvice::Never,
    }
    .into()));

    let throwing = group.construct(3, "");
    group.answer(
        throwing,
        "app:///needs-thrower.js",
        "import 'bobcat:worker'; import './thrower.js';",
    );
    let (url, completion) = group.views[3].source();
    assert_eq!(url, "app:///thrower.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "throw new Error('the dependency threw');".into(),
        url: "app:///thrower.js".into(),
    }));

    for (view, key, expected) in [(2, missing, "404"), (3, throwing, "the dependency threw")] {
        let event = group.next(view);
        assert_eq!(event.key, key);
        let WorkerPayload::Errored(error) = event.payload else {
            panic!("a graph that fails is something the realm threw")
        };
        assert!(error.message.contains(expected), "{}", error.message);
    }
    group.quiet();
    for view in [2, 3] {
        assert!(
            group.views[view].incoming.try_recv().is_err(),
            "the failure was reported once"
        );
    }
    group.terminate(missing);
    group.terminate(throwing);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
    group.wait_for_workers_to_end(3, PATIENCE, "the terminated worker ended");
}

#[test]
fn a_script_that_uses_the_global_scope_without_importing_it_throws_a_reference_error() {
    let mut group = Group::new();
    group.views.push(View::new());
    let key = group.construct_requesting(2, "", "app:///unscoped.js");
    group.answer(
        key,
        "app:///unscoped.js",
        "postMessage('reached');\nonmessage = (event) => postMessage(event.data);",
    );
    let event = group.next(2);
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("a use of a binding nothing declared is something the realm threw")
    };
    assert!(
        error.message.contains("ReferenceError") && error.message.contains("postMessage"),
        "{}",
        error.message
    );
    let location = error.location.expect("the throw has a location");
    assert_eq!(location.source.as_deref(), Some("app:///unscoped.js"));
    assert_eq!(location.line, Some(1));
    group.post(key, "dropped");
    group.quiet();
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "the throw was reported once, and the post reached nothing"
    );
    assert!(
        group.views[2].events.strong_count() > 1,
        "the worker is still running"
    );
    group.terminate(key);
    group.wait_for_workers_to_end(2, PATIENCE, "the terminated worker ended");
    assert!(
        group.views[2].incoming.try_recv().is_err(),
        "its end reported nothing"
    );
}

#[test]
fn a_worker_that_closes_itself_reports_and_takes_no_more() {
    let mut group = Group::new();
    let key = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; onmessage = () => postMessage(\"late\");
close();",
    );
    let event = group.next(0);
    assert_eq!(event.key, key);
    assert!(matches!(event.payload, WorkerPayload::Closed));
    group.post(key, "anyone?");
    group.quiet();
}

#[test]
fn a_terminated_worker_is_never_heard_from_again() {
    let mut group = Group::new();
    let key = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         onmessage = (event) => postMessage(event.data);",
    );
    group.post(key, "one");
    assert_eq!(group.message(0), wire("one"));
    group.terminate(key);
    group.quiet();
}

#[test]
fn terminating_a_worker_whose_script_is_still_in_flight_leaves_nothing_behind() {
    let mut group = Group::new();
    let key = group.construct(0, "");
    let completion = group.script_request(key);
    assert!(
        !completion.is_cancelled(),
        "the worker is waiting for its script"
    );
    group.terminate(key);
    group.wait_for_end(key, "the terminated worker ended");
    assert!(
        completion.is_cancelled(),
        "the request the host still holds is cancelled with the worker"
    );
    completion.complete(Ok(LoadedSource::Module {
        source: "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(\"too late\");"
            .to_owned(),
        url: "app:///w.js".to_owned(),
    }));
    group.quiet();
}

#[test]
fn releasing_a_view_ends_the_workers_it_created() {
    let mut group = Group::new();
    let key = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         onmessage = (event) => postMessage(event.data);",
    );
    group.post(key, "before");
    assert_eq!(group.message(0), wire("before"));

    group.release(0);

    group.wait_for_workers_to_end(0, PATIENCE, "the released view's worker task never ended");

    let survivor = group.construct(1, "");
    group.answer(
        survivor,
        "app:///other.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(\"alive\");",
    );
    let event = group.next(1);
    assert_eq!(event.key, survivor);
}

#[test]
fn cancelling_a_view_does_not_end_a_worker_whose_mts_handle_is_alive() {
    let mut group = Group::new();
    let worker = group.construct(0, "");
    group.cancel(0);
    group.answer(
        worker,
        "app:///worker.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; postMessage('still-owned');",
    );
    assert_eq!(group.message(0), wire("still-owned"));
    group.release(0);
    group.wait_for_workers_to_end(0, PATIENCE, "dropping the last sender ends the worker");
}

#[test]
fn a_worker_keeps_its_own_timers() {
    let mut group = Group::new();
    group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; let ticks = 0;
const handle = setInterval(() => {
  ticks += 1;
  if (ticks === 3) {
    clearInterval(handle);
    postMessage(ticks);
  }
}, 1);",
    );
    assert_eq!(group.message(0), HostValue::Number(3.0));
}

#[test]
fn a_bts_reads_what_another_views_bts_set_as_itself() {
    let mut group = Group::new();
    let setting = group.background_running(
        0,
        r"
        import { lynx } from 'bobcat:bts-runtime';
        const value = {n: 1};
        lynx.setSharedData('user', value);
        lynx.setSharedData('greet', () => 'from view 0');
        lynx.getJSModule('GlobalEventEmitter').addListener('mutate', () => {
            value.n = 2;
            postMessage('mutated');
        });
        postMessage('set');
    ",
    );
    assert_eq!(group.message(0), wire("set"));
    let reading = group.background_running(
        1,
        r"
        import { lynx } from 'bobcat:bts-runtime';
        lynx.getJSModule('GlobalEventEmitter').addListener('read', () => {
            const user = lynx.getSharedData('user');
            postMessage(JSON.stringify([
                user,
                user === lynx.getSharedData('user'),
                lynx.getSharedData('greet')(),
            ]));
        });
        postMessage('ready');
    ",
    );
    assert_eq!(group.message(1), wire("ready"));
    group.global_event(setting, "mutate");
    assert_eq!(group.message(0), wire("mutated"));
    group.global_event(reading, "read");
    assert_eq!(group.message(1), wire(r#"[{"n":2},true,"from view 0"]"#));
}

#[test]
fn a_set_calls_every_observer_of_the_group_inside_the_set() {
    let mut group = Group::new();
    let observing = group.background_running(
        1,
        r"
        import { lynx } from 'bobcat:bts-runtime';
        const observer = change => {
            change.seenBy = 'view 1';
            postMessage('observed ' + JSON.stringify(change));
        };
        lynx.registerSharedDataObserver(observer);
        lynx.getJSModule('GlobalEventEmitter').addListener('stop', () => {
            lynx.removeSharedDataObserver(observer);
            postMessage('stopped');
        });
        postMessage('observing');
    ",
    );
    assert_eq!(group.message(1), wire("observing"));
    let setting = group.background_running(
        0,
        r"
        import { lynx } from 'bobcat:bts-runtime';
        const own = [];
        lynx.registerSharedDataObserver(change => own.push(JSON.stringify(change)));
        let count = 0;
        lynx.getJSModule('GlobalEventEmitter').addListener('set', () => {
            count += 1;
            lynx.setSharedData('theme', {count});
            postMessage('own ' + own.splice(0).join(' '));
        });
        postMessage('ready');
    ",
    );
    assert_eq!(group.message(0), wire("ready"));

    group.global_event(setting, "set");
    assert_eq!(
        group.message(1),
        wire(r#"observed {"theme":{"count":1},"seenBy":"view 1"}"#)
    );
    assert_eq!(
        group.message(0),
        wire(r#"own {"theme":{"count":1},"seenBy":"view 1"}"#)
    );

    group.global_event(observing, "stop");
    assert_eq!(group.message(1), wire("stopped"));
    group.global_event(setting, "set");
    assert_eq!(group.message(0), wire(r#"own {"theme":{"count":2}}"#));
}

#[test]
fn the_shared_data_goes_with_the_groups_last_worker() {
    let mut group = Group::new();
    group.background_running(
        0,
        r"
        import { lynx } from 'bobcat:bts-runtime';
        lynx.setSharedData('color', 'green');
        postMessage(String(lynx.getSharedData('color')));
    ",
    );
    assert_eq!(group.message(0), wire("green"));
    group.release(0);
    group.wait_for_workers_to_end(0, PATIENCE, "the released view's BTS ended");
    group.background_running(
        1,
        r"
        import { lynx } from 'bobcat:bts-runtime';
        postMessage(String(lynx.getSharedData('color')));
    ",
    );
    assert_eq!(group.message(1), wire("undefined"));
}

#[test]
fn two_views_over_one_url_each_run_their_own_bytes() {
    let mut group = Group::new();
    let first = group.construct_requesting(0, "", "app:///shared.js");
    let second = group.construct_requesting(1, "", "app:///shared.js");
    group.answer(
        first,
        "app:///shared.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(\"first view\");",
    );
    assert_eq!(group.message(0), wire("first view"));
    group.answer(
        second,
        "app:///shared.js",
        "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(\"second view\");",
    );
    let event = group.next(1);
    assert_eq!(
        event.key, second,
        "an event arrives on the channel of the view whose realm created the worker"
    );
    let WorkerPayload::Message(data) = event.payload else {
        panic!("the second view's worker runs the second view's script")
    };
    assert_eq!(data, wire("second view"));
}

#[test]
fn one_worker_realm_shares_no_global_with_another_on_the_same_runtime() {
    let mut group = Group::new();
    let first = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; globalThis.marker = \"first\";
onmessage = () => postMessage(globalThis.marker);",
    );
    let second = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; postMessage(String(globalThis.marker));",
    );
    let event = group.next(0);
    assert_eq!(event.key, second);
    let WorkerPayload::Message(data) = event.payload else {
        panic!("the second worker starts on a global of its own")
    };
    assert_eq!(data, wire("undefined"));
    group.post(first, "ask");
    assert_eq!(group.message(0), wire("first"));
}

#[test]
fn imported_worker_graph_uses_response_urls_and_queues_messages_until_entry_finishes() {
    let mut group = Group::new();
    let worker = group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        const [first, second] = await Promise.all([import('./dep.js'), import('./dep.js')]);
        if (first !== second) throw Error('duplicate module evaluation');
        await new Promise(resolve => setTimeout(resolve, 1));
        addEventListener('message', event => postMessage([first.value, event.data]));
    ",
    );
    group.post(worker, "first");
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///dep.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "export { value } from './leaf.js';".to_owned(),
        url: "https://example.test/redirected/dep.js".to_owned(),
    }));
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://example.test/redirected/leaf.js");
    group.post(worker, "second");
    completion.complete(Ok(LoadedSource::Module {
        source: "export const value = 42;".to_owned(),
        url,
    }));
    assert_eq!(wire_json(&group.views[0].message()), r#"[42,"first"]"#);
    assert_eq!(wire_json(&group.views[0].message()), r#"[42,"second"]"#);
    assert!(group.views[0].asked_for_nothing_more());
}

#[test]
fn a_worker_requires_commonjs_and_json_against_its_own_response_url() {
    let mut group = Group::new();
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        import { createRequire } from 'bobcat:module';
        const require = createRequire(import.meta.url);
        const lib = require('./lib/answer.cjs');
        const config = require('./config.json');
        postMessage([lib.answer, lib.dir, config.name].join(':'));
    ",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///lib/answer.cjs");
    completion.complete(Ok(LoadedSource::Module {
        source: "exports.answer = require('./deep.cjs').answer + 1;\nexports.dir = __dirname;"
            .to_owned(),
        url: "https://cdn.test/lib/answer.cjs".to_owned(),
    }));
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://cdn.test/lib/deep.cjs");
    completion.complete(Ok(LoadedSource::Module {
        source: "exports.answer = 41;".to_owned(),
        url,
    }));
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "app:///config.json");
    completion.complete(Ok(LoadedSource::Module {
        source: r#"{"name": "card"}"#.to_owned(),
        url,
    }));
    assert_eq!(
        group.message(0),
        wire("42:https://cdn.test/lib/:card"),
        "a nested require resolves against the response URL, and `.json` parses"
    );
}

#[test]
fn a_bts_bundle_requires_a_chunk_under_its_container_url() {
    let mut group = Group::new();
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        import { lynx, __BobcatRegisterBundle } from 'bobcat:bts-runtime';
        __BobcatRegisterBundle('https://cdn.test/app/x.web.bundle');
        postMessage(JSON.stringify(lynx.requireModule('/chunk.js')));
    ",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://cdn.test/app/x.web.bundle/chunk.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "module.exports = { answer: 42 };".to_owned(),
        url,
    }));
    assert_eq!(
        group.message(0),
        wire(r#"{"answer":42}"#),
        "a bundle path hangs under its container URL and answers its exports"
    );
}

#[test]
fn a_registered_bundle_body_answers_through_its_modules_default_export() {
    let mut group = Group::new();
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        import { lynx, __BobcatRegisterBundle } from 'bobcat:bts-runtime';
        __BobcatRegisterBundle('https://cdn.test/app/x.web.bundle');
        postMessage(JSON.stringify(lynx.requireModule('/app-service.js')));
    ",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://cdn.test/app/x.web.bundle/app-service.js");
    completion.complete(Ok(LoadedSource::Module {
        source: format!(
            "{}export default {{init: ({{tt}}) => ({{\
             card: typeof tt.define, entry: globalThis.globDynamicComponentEntry, \
             runtime: typeof lynx.getNativeApp}})}};",
            crate::esm::BTS_CHUNK_PREAMBLE
        ),
        url,
    }));
    assert_eq!(
        group.message(0),
        wire(r#"{"card":"function","entry":"__Card__","runtime":"function"}"#),
        "the body's module default-exported the object the card starts from"
    );
}

#[test]
fn a_worker_sync_load_resolves_against_the_views_base_not_its_own_url() {
    let mut group = Group::new();
    group.views[0].base = Arc::new(Url::parse("https://cdn.test/page/").unwrap());
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        import { lynx } from 'bobcat:bts-runtime';
        postMessage(lynx.loadScript('x', {bundleName: 'lazy.bundle'}));
    ",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://cdn.test/page/lazy.bundle/x.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "module.exports = 'section x';".to_owned(),
        url,
    }));
    assert_eq!(group.message(0), wire("section x"));
}

#[test]
fn a_require_nobody_answers_throws_in_the_worker_and_leaves_it_usable() {
    let mut group = Group::new();
    let worker = group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        import { createRequire } from 'bobcat:module';
        let message = '';
        try { createRequire(import.meta.url)('./missing.cjs'); }
        catch (error) { message = String(error); }
        addEventListener('message', event =>
            postMessage([message.includes('app:///missing.cjs'), event.data]));
    ",
    );
    group.post(worker, "queued");
    let (_, completion) = group.views[0].source();
    drop(completion);
    assert_eq!(wire_json(&group.views[0].message()), r#"[true,"queued"]"#);
}

#[test]
fn a_worker_links_only_the_built_ins_its_host_modules_have_members_for() {
    let mut group = Group::new();
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        const outcomes = [];
        for (const specifier of ['bobcat:element', 'bobcat-internal', 'bobcat:nope']) {
            try { await import(specifier); outcomes.push('loaded'); }
            catch (error) { outcomes.push(`${error.name}: ${error.message}`); }
        }
        postMessage(JSON.stringify(outcomes));
    ",
    );
    let HostValue::String(outcomes) = group.message(0) else {
        panic!("the worker posts its outcomes as JSON text");
    };
    let outcomes: Vec<String> = serde_json::from_str(&outcomes).unwrap();
    let [element, worker_class, missing] = outcomes.as_slice() else {
        panic!("one outcome per import: {outcomes:?}");
    };
    for linked in [element, worker_class] {
        assert!(
            linked.starts_with("SyntaxError: Could not find export"),
            "{linked}"
        );
    }
    assert!(
        missing.starts_with("ReferenceError: ") && missing.contains("'bobcat:nope'"),
        "{missing}"
    );
    assert!(group.views[0].asked_for_nothing_more());
}

#[test]
fn a_worker_realm_declares_these_host_members() {
    let mut group = Group::new();
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        postMessage([
            Object.keys(await import('bobcat-internal:host')).join(','),
            Object.keys(await import('bobcat-internal:worker')).join(','),
            Object.keys(await import('bobcat-internal:native-modules')).join(','),
        ].join(' / '));
    ",
    );
    let host = [
        "bindPipelineTimingFlag",
        "clearTimer",
        "fetchResource",
        "generatePipelineId",
        "loadModuleSync",
        "logScriptMessage",
        "markLoadTiming",
        "markPipelineTiming",
        "observePerformance",
        "onPipelineStart",
        "reportScriptError",
        "requestScriptFrame",
        "resolveModuleUrl",
        "resolveUrl",
        "setTimer",
        "settleFuture",
        "takeFuture",
        "takePerformanceEntries",
        "testFuture",
        "waitFuture",
    ];
    let worker = [
        "closeWorker",
        "postWorkerMessage",
        "sharedObject",
        "workerName",
    ];
    let native_modules = ["invokeNativeModule"];
    assert_eq!(
        group.message(0),
        wire(&format!(
            "{} / {} / {}",
            crate::canvas::host_members_with_canvas(&host),
            worker.join(","),
            native_modules.join(",")
        ))
    );
}

#[test]
fn a_handled_import_failure_keeps_the_worker_usable() {
    let mut group = Group::new();
    let worker = group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        let failed = false;
        try { await import('./missing.js'); } catch { failed = true; }
        addEventListener('message', event => postMessage([failed, event.data]));
    ",
    );
    group.post(worker, "queued");
    let (_, completion) = group.views[0].source();
    drop(completion);
    assert_eq!(wire_json(&group.views[0].message()), r#"[true,"queued"]"#);
}

#[test]
fn worker_import_cancellation_follows_its_handle_instead_of_the_view_token() {
    let mut group = Group::new();
    let worker = group.start(
        "import 'bobcat:worker'; import 'bobcat:timers'; \
         await import('./pending.js'); postMessage('must not run');",
    );
    let (_, completion) = group.views[0].source();
    group.cancel(0);
    assert!(!completion.is_cancelled());
    group.terminate(worker);
    group.wait_for_workers_to_end(0, PATIENCE, "the terminated worker ends its import");
    assert!(completion.is_cancelled());
}

#[test]
fn a_rejected_worker_tla_is_reported_and_leaves_the_message_queue_usable() {
    let mut group = Group::new();
    let worker = group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        onmessage = event => postMessage(event.data);
        await import('./rejected.js');
    ",
    );
    group.post(worker, "queued");
    let (_, completion) = group.views[0].source();
    completion.complete(Ok(LoadedSource::Module {
        source: "await new Promise(resolve => setTimeout(resolve, 1)); throw Error('TLA failed');"
            .into(),
        url: "app:///rejected.js".into(),
    }));
    let WorkerPayload::Errored(error) = group.next(0).payload else {
        panic!("TLA rejection must be reported")
    };
    assert!(error.message.contains("TLA failed"), "{error}");
    assert_eq!(group.message(0), wire("queued"));
    group.quiet();
}

#[test]
fn one_worker_future_times_out_then_settles_as_a_promise_and_refuses_a_later_wait() {
    let mut group = Group::new();
    group.start(
        r"import 'bobcat:worker'; import 'bobcat:timers';
        import { Future } from 'bobcat:future';
        import { testFuture } from 'bobcat-internal:host';

        const slow = new Future(testFuture(200, 'late', false));
        try {
          slow.wait(20);
          postMessage('the wait answered');
        } catch (error) {
          postMessage('wait ' + error.name);
        }
        postMessage('then ' + await slow);
        try {
          slow.wait();
          postMessage('the third read answered');
        } catch (error) {
          postMessage('after ' + error.name);
        }
        try {
          await new Future(testFuture(1, 'why', true));
          postMessage('the rejection resolved');
        } catch (error) {
          postMessage('catch ' + (error instanceof Error) + ' ' + error.message);
        }
        postMessage('now ' + new Future(testFuture(1, 'now', false)).wait());
    ",
    );
    for expected in [
        "wait TimeoutError",
        "then late",
        "after TypeError",
        "catch true why",
        "now now",
    ] {
        assert_eq!(group.message(0), wire(expected));
    }
}
