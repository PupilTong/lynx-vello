//! The group's worker realms, driven the way the rest of the engine drives
//! them: a `Start` per worker carrying its own channels, and the test playing
//! both the realm that names a worker and the host that fetches for it.
//!
//! Nothing here builds a view, a document or a `bobcat-main`. That is the
//! point — a worker realm reaches none of them, so a test that had to build
//! one would be testing something else.

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{
    WorkerCommand, WorkerEvent, WorkerHome, WorkerKey, WorkerMessage, WorkerPayload, WorkerStart,
};
use crate::clock::ClockInstant;
use crate::link::{SourceRequester, ViewNotice, block_on_deadline};
use crate::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, RetryAdvice,
    SourceCompletion, SourceRequest,
};

impl WorkerHome {
    pub(crate) fn with_entry_for_test(entry: (String, String)) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let thread = super::ThreadBuilder::new()
            .name("bobcat-test-workers".into())
            .spawn(move || super::thread::run_with_entry(receiver, entry))
            .unwrap();
        Self {
            commands,
            thread: crate::threads::ThreadJoin::new(thread),
        }
    }
}

/// How long a test waits for a thread that should already be working.
const PATIENCE: Duration = Duration::from_secs(30);

/// One message in the shape the boundary carries: JSON, wrapped in an array
/// so that `undefined` has an encoding at all.
fn wire(data: &str) -> String {
    format!("[\"{data}\"]")
}

/// One realm's whole side of its workers, which is one channel each, the one
/// they all report on, and the token every one of them holds a child of.
struct View {
    messages: FxHashMap<WorkerKey, mpsc::UnboundedSender<WorkerMessage>>,
    events: mpsc::UnboundedSender<WorkerEvent>,
    incoming: mpsc::UnboundedReceiver<WorkerEvent>,
    /// This view's end signal, standing in for the one `create_lynx_view`
    /// mints on the embedder's thread.
    token: CancellationToken,
    notices: mpsc::UnboundedSender<ViewNotice>,
    sources: mpsc::UnboundedReceiver<ViewNotice>,
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
        }
    }

    fn next(&mut self) -> WorkerEvent {
        block_on_deadline(self.incoming.recv(), ClockInstant::now() + PATIENCE)
            .flatten()
            .expect("the worker thread has something to report")
    }

    /// What the worker said, for the tests that only care about that.
    fn message(&mut self) -> String {
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
    #[expect(dead_code, reason = "held to end and wait for the worker thread")]
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
        let token = self.views[view].token.child_token();
        let sources = SourceRequester::new(
            self.views[view].notices.clone(),
            Arc::new(crate::NoWakeup),
            token.clone(),
        );
        self.tell(WorkerCommand::Start(WorkerStart {
            key,
            name: name.to_owned(),
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
            .send(Ok(LoadedSource::Entry {
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

    fn message(&mut self, view: usize) -> String {
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
    assert_eq!(group.message(0), "[\"counter:ping\"]");
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
    assert_eq!(group.message(0), "[\"first\"]");
    assert_eq!(group.message(0), "[\"second\"]");
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
    assert_eq!(group.message(0), "[\"still here\"]");
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
    assert_eq!(group.message(0), "[\"one\"]");
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
    assert_eq!(group.message(0), "[\"before\"]");

    group.release(0);

    group.wait_for_workers_to_end(0, PATIENCE, "the released view's worker task never ended");

    // Another view's worker still answers, which is what makes the silence
    // above a release rather than a stopped thread.
    let survivor = group.construct(1, "");
    group.answer(survivor, "app:///other.js", "postMessage(\"alive\");");
    let event = group.next(1);
    assert_eq!(event.key, survivor);
}

/// A released view ends the workers it created without a message reaching any
/// of them: each holds a child of that view's token, so cancelling the one on
/// the embedder's thread is what wakes a worker still waiting for its script.
///
/// The message sender stays open and unused throughout, so what ends this
/// worker cannot be a `Terminate` or a closed channel — and it ends silently,
/// which is the other half of the same claim: every other way out reports
/// something on the view's event channel first.
#[test]
fn cancelling_a_view_ends_a_worker_whose_script_never_arrived() {
    let mut group = Group::new();
    let _parked = group.construct(0, "");

    group.cancel(0);

    // A second or two rather than PATIENCE: nothing here waits for IO, and a
    // worker that has to be told is a worker this never wakes at all.
    group.wait_for_workers_to_end(
        0,
        Duration::from_secs(2),
        "a cancelled view's parked worker never ended",
    );
    assert!(
        group.views[0].incoming.try_recv().is_err(),
        "and it ended without saying anything: a worker that took the failure path or found \
         its channel closed would have reported one of those first"
    );
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
    assert_eq!(group.message(0), "[3]");
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
    assert_eq!(group.message(0), "[\"first view\"]");
    group.answer(second, "app:///shared.js", "postMessage(\"second view\");");
    let event = group.next(1);
    assert_eq!(
        event.key, second,
        "an event arrives on the channel of the view whose realm created the worker"
    );
    let WorkerPayload::Message(data) = event.payload else {
        panic!("the second view's worker runs the second view's script")
    };
    assert_eq!(data, "[\"second view\"]");
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
    assert_eq!(data, "[\"undefined\"]");
    group.post(first, "ask");
    assert_eq!(group.message(0), "[\"first\"]");
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
    completion.complete(Ok(LoadedSource::Entry {
        source: "export { value } from './leaf.js';".to_owned(),
        url: "https://example.test/redirected/dep.js".to_owned(),
    }));
    let (url, completion) = group.views[0].source();
    assert_eq!(url, "https://example.test/redirected/leaf.js");
    group.post(worker, "second");
    completion.complete(Ok(LoadedSource::Entry {
        source: "export const value = 42;".to_owned(),
        url,
    }));
    assert_eq!(group.views[0].message(), "[[42,\"first\"]]");
    assert_eq!(group.views[0].message(), "[[42,\"second\"]]");
    assert!(group.views[0].sources.try_recv().is_err());
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
    assert_eq!(group.views[0].message(), "[[true,\"queued\"]]");
}

#[test]
fn releasing_a_view_cancels_its_workers_import_requests_immediately() {
    let mut group = Group::new();
    group.start("await import('./pending.js'); postMessage('must not run');");
    let (_, completion) = group.views[0].source();
    assert!(!completion.is_cancelled());
    group.views[0].token.cancel();
    assert!(completion.is_cancelled());
    completion.complete(Ok(LoadedSource::Entry {
        source: String::new(),
        url: "app:///pending.js".to_owned(),
    }));
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
    completion.complete(Ok(LoadedSource::Entry {
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
