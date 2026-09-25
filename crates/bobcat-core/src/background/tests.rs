//! The group's worker realms, driven the way the rest of the engine drives
//! them: a `Start` per worker carrying its own channels, and the test playing
//! both the realm that names a worker and the host that fetches for it.
//!
//! Nothing here builds a view, a document or a `bobcat-main`. That is the
//! point — a worker realm reaches none of them, so a test that had to build
//! one would be testing something else.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    WorkerCommand, WorkerEvent, WorkerHome, WorkerKey, WorkerMessage, WorkerPayload, WorkerRole,
    WorkerStart, wire_json,
};
use crate::clock::ClockInstant;
use crate::link::{HostOutbox, ViewNotice, block_on_deadline, detached_base};
use crate::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, RetryAdvice,
    SourceCompletion, SourceRequest,
};

impl WorkerHome {
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

/// How long a test waits for a thread that should already be working.
const PATIENCE: Duration = Duration::from_secs(30);

/// One string in the shape the boundary carries it: a `HostValue`, because a
/// primitive crosses as itself rather than inside an encoding.
fn wire(data: &str) -> HostValue {
    HostValue::String(data.to_owned())
}

/// One realm's whole side of its workers, which is one channel each, the one
/// they all report on. The view token does not own any worker's lifetime.
struct View {
    messages: FxHashMap<WorkerKey, mpsc::UnboundedSender<WorkerMessage>>,
    events: mpsc::UnboundedSender<WorkerEvent>,
    incoming: mpsc::UnboundedReceiver<WorkerEvent>,
    /// This view's end signal, standing in for the one `create_lynx_view`
    /// mints on the embedder's thread.
    token: CancellationToken,
    notices: mpsc::UnboundedSender<ViewNotice>,
    sources: mpsc::UnboundedReceiver<ViewNotice>,
    /// This view's base URL, which every worker it constructs resolves its
    /// synchronous loads against: `app:///` unless a test names another.
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
            base: detached_base(),
        }
    }

    fn next(&mut self) -> WorkerEvent {
        block_on_deadline(self.incoming.recv(), ClockInstant::now() + PATIENCE)
            .flatten()
            .expect("the worker thread has something to report")
    }

    /// What the worker said, for the tests that only care about that.
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
        match block_on_deadline(self.sources.recv(), ClockInstant::now() + PATIENCE)
            .flatten()
            .expect("the worker requested a module")
        {
            ViewNotice::RequestSource {
                request: SourceRequest::Module(url),
                completion,
            } => (url, completion),
            _ => panic!("a worker only requests module sources"),
        }
    }
}

/// One group's worker thread, with the test on both of its ends.
struct Group {
    /// The thread itself, waited for by its own drop — which is reached with
    /// the goodbye already said, because [`Drop for Group`](Group::drop) drops
    /// the test's own sender and every view before any field drops.
    home: WorkerHome,
    commands: Option<mpsc::UnboundedSender<WorkerCommand>>,
    views: Vec<View>,
    scripts: FxHashMap<WorkerKey, oneshot::Sender<Result<LoadedSource, crate::LynxViewError>>>,
    next_key: Cell<u64>,
}

impl Group {
    fn new() -> Self {
        let home = WorkerHome::start().expect("the worker thread starts");
        Self {
            commands: Some(home.commands()),
            home,
            views: vec![View::new(), View::new()],
            scripts: FxHashMap::default(),
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

    /// Names one worker on a view, without answering its script yet. The
    /// realm's other half of a construction — asking the host to fetch — is
    /// what the test does by hand in [`Self::answer`].
    fn construct(&mut self, view: usize, name: &str) -> WorkerKey {
        let key = WorkerKey::new(self.next_key.get());
        self.next_key.set(key.get() + 1);
        let (script, awaiting) = oneshot::channel();
        let (messages, incoming) = mpsc::unbounded_channel();
        let token = CancellationToken::new();
        let sources = HostOutbox::new(
            self.views[view].notices.clone(),
            Arc::new(crate::NoWakeup),
            token.clone(),
            None,
            Arc::clone(&self.views[view].base),
        );
        self.tell(WorkerCommand::Start(WorkerStart {
            key,
            name: name.to_owned(),
            role: WorkerRole::Dedicated,
            script: awaiting,
            messages: incoming,
            events: self.views[view].events.clone(),
            token,
            sources,
        }));
        self.views[view].messages.insert(key, messages);
        self.scripts.insert(key, script);
        key
    }

    /// The host's half: one script, answered.
    fn answer(&mut self, key: WorkerKey, url: &str, source: &str) {
        let _ = self
            .scripts
            .remove(&key)
            .expect("the worker is still waiting for its script")
            .send(Ok(LoadedSource::Module {
                source: source.to_owned(),
                url: url.to_owned(),
            }));
    }

    /// Constructs a worker on view 0 and answers its script with `source`.
    fn start(&mut self, source: &str) -> WorkerKey {
        let key = self.construct(0, "");
        self.answer(key, "app:///worker.js", source);
        key
    }

    fn post(&self, key: WorkerKey, data: &str) {
        self.send(key, WorkerMessage::Post(wire(data)));
    }

    /// Posts whatever the test built, for the structured-clone cases.
    fn post_value(&self, key: WorkerKey, data: HostValue) {
        self.send(key, WorkerMessage::Post(data));
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

    /// `Worker.terminate()`: the realm forgets the worker and tells it so.
    ///
    /// One more message follows the terminate on a channel that is still
    /// open, and only then is the sender dropped. That is what makes the
    /// silence afterwards mean `Terminate`: a worker that ended merely
    /// because its channel closed would have delivered this one first.
    fn terminate(&mut self, key: WorkerKey) {
        for view in &mut self.views {
            if let Some(messages) = view.messages.remove(&key) {
                let _ = messages.send(WorkerMessage::Terminate);
                let _ = messages.send(WorkerMessage::Post(wire("after terminate")));
                drop(messages);
                return;
            }
        }
    }

    /// A view is gone, and with it every sender it held.
    fn release(&mut self, view: usize) {
        self.views[view].messages.clear();
    }

    /// The embedder released a view, and nothing else has happened yet: the
    /// token is cancelled while every message sender is still open and
    /// unsent-to, which is the order `LynxView::drop` does it in.
    fn cancel(&mut self, view: usize) {
        self.views[view].token.cancel();
    }

    /// Waits for the released view's worker tasks to have ended, which is the
    /// count of that view's event senders falling back to the test's own.
    ///
    /// The task and the thread's own reporter table are what hold the other
    /// clones, so the count is the observation; silence alone could not say
    /// it.
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

    /// Nothing more arrives, and a full round of the thread has passed to
    /// prove it: a `close()` the thread has already served would have to
    /// overtake this message to make the assertion pass by accident.
    fn quiet(&mut self) {
        let probe = self.construct(0, "");
        self.answer(probe, "app:///probe.js", "postMessage(\"probe\");");
        let event = self.next(0);
        assert_eq!(event.key, probe, "something else was still to be reported");
        let expected = wire("probe");
        assert!(matches!(event.payload, WorkerPayload::Message(ref data) if *data == expected));
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // The group's own goodbye, which a test has no reason to spell: every
        // realm is gone, so every worker's own channel is too. The home's own
        // sender is the last one, and it closes — and the thread is waited for
        // — as this struct's fields drop behind this body.
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
        "onmessage = (event) => postMessage(`${name}:${event.data}`);",
    );
    group.post(key, "ping");
    assert_eq!(group.message(0), wire("counter:ping"));
}

/// A value that is not a primitive crosses as a structured clone, which is
/// what lets it cross a thread at all: the group's worker realms write and
/// read it with the engine's own serializer, so what one worker posts is what
/// the next one receives — an `undefined`-valued key, `NaN`, a `Date`, a typed
/// array, a `BigInt` and a cycle included.
#[test]
fn a_structured_value_survives_a_round_trip_through_two_worker_realms() {
    let mut group = Group::new();
    group.start(
        "const value = { tag: 'payload', missing: undefined, nan: NaN,
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
        "addEventListener('message', (event) => {
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
    // Both posted while the fetch is still in flight, which is the ordinary
    // shape: a card constructs a worker and posts to it in the same task.
    group.post(key, "first");
    group.post(key, "second");
    group.answer(
        key,
        "app:///w.js",
        "onmessage = (event) => postMessage(event.data);",
    );
    assert_eq!(group.message(0), wire("first"));
    assert_eq!(group.message(0), wire("second"));
}

#[test]
fn a_script_that_cannot_be_fetched_fails_its_worker_and_nothing_else() {
    let mut group = Group::new();
    let doomed = group.construct(0, "");
    let _ = group
        .scripts
        .remove(&doomed)
        .expect("the worker is waiting")
        .send(Err(ResourceError {
            kind: ResourceErrorKind::NotFound,
            phase: ResourceErrorPhase::ReceiveHeaders,
            locator: None,
            message: "404".into(),
            retry: RetryAdvice::Never,
        }
        .into()));
    let event = group.next(0);
    assert_eq!(event.key, doomed);
    let WorkerPayload::Failed(error) = event.payload else {
        panic!("a script that never arrived leaves no worker")
    };
    assert!(
        error.message.contains("loading the worker's script") && error.message.contains("404"),
        "{}",
        error.message
    );
    // The runtime is untouched: the next worker over the same thread runs.
    let key = group.start("postMessage(\"alive\");");
    assert_eq!(group.next(0).key, key);
}

/// A trap in the thread's own loop ends every worker on it at once, and no
/// worker's own owner is left to say so: the thread tells the creator of each
/// live one `Failed` and sets the flag `bobcat-main` reads before a `Start`.
/// A worker that ended before the trap is not told again, even while its task
/// has not been joined yet. The thread is over, so the group's drop still
/// joins it.
#[test]
fn a_trapped_worker_thread_fails_every_live_worker() {
    let mut group = Group::new();
    group.views.push(View::new());
    // One worker per view, each still waiting for its script.
    let first = group.construct(0, "");
    let second = group.construct(1, "");
    // This one's boot job waits on a load until the test answers it, and no
    // other job runs meanwhile.
    let parked = group.construct(2, "");
    group.answer(
        parked,
        "app:///parked.js",
        "import { createRequire } from 'bobcat:module';
createRequire(import.meta.url)('./held.cjs');",
    );
    let (url, held) = group.views[2].source();
    // Ended by a `Terminate` its task reads during that wait. Its task is not
    // joined before the trap: the job that reclaims its realm is queued
    // behind the waiting one. Its boot task dropping the script's receiver is
    // what shows it has ended.
    let ended = group.construct(1, "");
    group.terminate(ended);
    let deadline = ClockInstant::now() + PATIENCE;
    while !group.scripts[&ended].is_closed() {
        assert!(
            ClockInstant::now() < deadline,
            "the terminated worker ended"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
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
    // Exactly one each, and none for the worker that had ended: once the
    // thread has dropped every clone of a view's sender, nothing more can
    // arrive on it.
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
        "onmessage = (event) => postMessage(event.data);
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
fn a_worker_that_closes_itself_reports_and_takes_no_more() {
    let mut group = Group::new();
    let key = group.start(
        "onmessage = () => postMessage(\"late\");
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
    let key = group.start("onmessage = (event) => postMessage(event.data);");
    group.post(key, "one");
    assert_eq!(group.message(0), wire("one"));
    // The terminate is followed by a post the worker would echo if it were
    // still running, so the silence below is this worker obeying rather than
    // its channel having closed under it.
    group.terminate(key);
    group.quiet();
}

#[test]
fn terminating_a_worker_whose_script_is_still_in_flight_leaves_nothing_behind() {
    let mut group = Group::new();
    let key = group.construct(0, "");
    group.terminate(key);
    group.answer(key, "app:///w.js", "postMessage(\"too late\");");
    group.quiet();
}

#[test]
fn releasing_a_view_ends_the_workers_it_created() {
    let mut group = Group::new();
    let key = group.start("onmessage = (event) => postMessage(event.data);");
    group.post(key, "before");
    assert_eq!(group.message(0), wire("before"));

    group.release(0);

    group.wait_for_workers_to_end(0, PATIENCE, "the released view's worker task never ended");

    // Another view's worker still answers, which is what makes the silence
    // above a release rather than a stopped thread.
    let survivor = group.construct(1, "");
    group.answer(survivor, "app:///other.js", "postMessage(\"alive\");");
    let event = group.next(1);
    assert_eq!(event.key, survivor);
}

/// The MTS handle, not the view token, owns a worker still loading its script.
#[test]
fn cancelling_a_view_does_not_end_a_worker_whose_mts_handle_is_alive() {
    let mut group = Group::new();
    let worker = group.construct(0, "");
    group.cancel(0);
    group.answer(worker, "app:///worker.js", "postMessage('still-owned');");
    assert_eq!(group.message(0), wire("still-owned"));
    group.release(0);
    group.wait_for_workers_to_end(0, PATIENCE, "dropping the last sender ends the worker");
}

#[test]
fn a_worker_keeps_its_own_timers() {
    let mut group = Group::new();
    group.start(
        "let ticks = 0;
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
fn two_views_over_one_url_each_run_their_own_bytes() {
    let mut group = Group::new();
    // Every view has its own `ResourceFetcher`, so one URL can resolve to two
    // different scripts in one group. Each worker must run the bytes its own
    // view answered with — which nothing has to arrange, because a worker's
    // script is evaluated into its realm rather than registered on the
    // runtime under a name a second view could reach.
    let first = group.construct(0, "");
    let second = group.construct(1, "");
    group.answer(first, "app:///shared.js", "postMessage(\"first view\");");
    assert_eq!(group.message(0), wire("first view"));
    group.answer(second, "app:///shared.js", "postMessage(\"second view\");");
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
        "globalThis.marker = \"first\";
onmessage = () => postMessage(globalThis.marker);",
    );
    let second = group.start("postMessage(String(globalThis.marker));");
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
        r"
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
    assert!(group.views[0].sources.try_recv().is_err());
}

/// `require` reaches the same host as an import and resolves against the same
/// response URLs, from the boot job of a worker on the thread every worker of
/// the group shares: no JavaScript of that thread's runs while a load is out.
#[test]
fn a_worker_requires_commonjs_and_json_against_its_own_response_url() {
    let mut group = Group::new();
    group.start(
        r"
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

/// The compiled-bundle loader over the same synchronous load: a bundle path is
/// asked for beside the template URL the bundle was registered with, and the
/// exports of its body are what `requireModule` answers. Nothing is registered
/// with the realm but that URL, so a path a container carried and one it did
/// not are one mechanism.
#[test]
fn a_bts_bundle_requires_a_chunk_beside_its_template_url() {
    let mut group = Group::new();
    group.start(
        r"
        import { lynx, __BobcatRegisterBundle } from 'bobcat:bts-runtime';
        __BobcatRegisterBundle('https://cdn.test/app/x.web.bundle');
        postMessage(JSON.stringify(lynx.requireModule('/chunk.js')));
    ",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://cdn.test/app/chunk.js");
    completion.complete(Ok(LoadedSource::Module {
        source: "module.exports = { answer: 42 };".to_owned(),
        url,
    }));
    assert_eq!(
        group.message(0),
        wire(r#"{"answer":42}"#),
        "a bundle path resolves against its template URL and answers its exports"
    );
}

/// A body `PageSource` registered is an **ES module**, and the same
/// `requireModule` loads it: `require(esm)` compiles it, links its
/// `BTS_CHUNK_PREAMBLE` import of this realm's own runtime, evaluates it and
/// answers its namespace, whose default export is what native's host would
/// have kept as that script's completion value. An `{init}` there is what
/// starts the card, with the entry published for the call.
///
/// The `require` runs from inside the entry module's own evaluation, which is
/// where a page's boot script calls it from, and the body's import of
/// `bobcat:bts-runtime` links to the instance this realm already has.
#[test]
fn a_registered_bundle_body_answers_through_its_modules_default_export() {
    let mut group = Group::new();
    group.start(
        r"
        import { lynx, __BobcatRegisterBundle } from 'bobcat:bts-runtime';
        __BobcatRegisterBundle('https://cdn.test/app/x.web.bundle');
        postMessage(JSON.stringify(lynx.requireModule('/app-service.js')));
    ",
    );
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://cdn.test/app/app-service.js");
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

/// A synchronous load in a worker resolves against its view's base URL, not
/// against the worker's own script URL: the base a view's MTS realm resolves
/// its loads against too, and the one its fetcher registered a lazy
/// container's sections under.
#[test]
fn a_worker_sync_load_resolves_against_the_views_base_not_its_own_url() {
    let mut group = Group::new();
    group.views[0].base = Arc::new(Url::parse("https://cdn.test/page/").unwrap());
    group.start(
        r"
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
        r"
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

/// The worker runtime registers every built-in, and a worker realm's host
/// modules decide which of them link: the MTS modules import members only an
/// MTS realm's `bobcat-internal:host` has, so they fail at link, and a name
/// no runtime registered fails to load. None of the three reaches the host.
#[test]
fn a_worker_links_only_the_built_ins_its_host_modules_have_members_for() {
    let mut group = Group::new();
    group.start(
        r"
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
    assert!(group.views[0].sources.try_recv().is_err());
}

/// The members a worker realm's two host modules export, which is what
/// decides the built-ins it can link. Written down so that a change to either
/// set is a change to these lists. A namespace lists its exports sorted by
/// name; `testFuture` is the test build's own producer.
#[test]
fn a_worker_realm_declares_these_host_members() {
    let mut group = Group::new();
    group.start(
        r"
        postMessage([
            Object.keys(await import('bobcat-internal:host')).join(','),
            Object.keys(await import('bobcat-internal:worker')).join(','),
        ].join(' / '));
    ",
    );
    let host = [
        "clearTimer",
        "fetchResource",
        "loadModuleSync",
        "logScriptMessage",
        "reportScriptError",
        "requestScriptFrame",
        "resolveModuleUrl",
        "setTimer",
        "settleFuture",
        "takeFuture",
        "testFuture",
        "waitFuture",
    ];
    let worker = ["closeWorker", "invokeNativeModule", "postWorkerMessage"];
    assert_eq!(
        group.message(0),
        wire(&format!("{} / {}", host.join(","), worker.join(",")))
    );
}

#[test]
fn a_handled_import_failure_keeps_the_worker_usable() {
    let mut group = Group::new();
    let worker = group.start(
        r"
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
    let worker = group.start("await import('./pending.js'); postMessage('must not run');");
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
        r"
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
    let mut reported = false;
    loop {
        match group.next(0).payload {
            WorkerPayload::Errored(error) => {
                // The existing engine exposes both checkpoint failures and
                // entry rejection; neither may poison later message delivery.
                assert!(error.message.contains("TLA failed"), "{error}");
                reported = true;
            }
            WorkerPayload::Message(value) => {
                assert!(reported, "TLA rejection must be reported");
                assert_eq!(value, wire("queued"));
                break;
            }
            _ => panic!("a rejected entry must leave its worker usable"),
        }
    }
    group.quiet();
}

/// One `Future` in a worker realm, read both ways over the real boundary:
/// this worker's own token, its own job queue, and the settle task its own
/// epilogue spawns rather than a view's.
///
/// Nothing in production registers a future yet, so the operation is the
/// host's test-only producer — `testFuture(delayMs, value, rejects)`, which
/// is why this test is in the crate rather than beside it. `wait(20)` runs
/// out its deadline against a 200 ms operation, and the `await` that follows
/// is the *same* Future, which is what says the timeout cancelled nothing.
/// Past that conversion the Future is a Promise, and a third read of it is
/// refused.
#[test]
fn one_worker_future_times_out_then_settles_as_a_promise_and_refuses_a_later_wait() {
    let mut group = Group::new();
    group.start(
        r"
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
