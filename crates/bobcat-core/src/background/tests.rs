//! The group's worker realms, driven the way the rest of the engine drives
//! them: commands in, events out on the group's own FIFO, and the test
//! playing both the realm that names a worker and the painter that fetches
//! for it.
//!
//! Nothing here builds a view, a document or a `bobcat-main`. That is the
//! point — a worker realm reaches none of them, so a test that had to build
//! one would be testing something else.

use std::cell::Cell;
use std::time::Duration;

use super::{
    WorkerCommand, WorkerEvent, WorkerHome, WorkerKey, WorkerPayload, WorkerScript, WorkerStart,
};
use crate::mailbox::{Mailbox, Sender};
use crate::view::{ToMain, ViewId, test_view};

impl WorkerHome {
    pub(crate) fn with_entry_for_test(to_main: Sender<ToMain>, entry: WorkerScript) -> Self {
        let (commands, receiver) = Mailbox::channel();
        let thread = super::ThreadBuilder::new()
            .name("bobcat-test-workers".into())
            .spawn(move || super::thread::run_with_entry(&receiver, &to_main, &entry))
            .unwrap();
        Self {
            commands: Some(commands),
            thread: Some(thread),
        }
    }
}

/// How long a test waits for a thread that should already be working.
const PATIENCE: Duration = Duration::from_secs(10);

/// One message in the shape the boundary carries: JSON, wrapped in an array
/// so that `undefined` has an encoding at all.
fn wire(data: &str) -> String {
    format!("[\"{data}\"]")
}

/// One group's worker thread, with the test on both of its ends.
struct Group {
    home: WorkerHome,
    commands: Sender<WorkerCommand>,
    events: Mailbox<ToMain>,
    next_key: Cell<u64>,
}

impl Group {
    fn new() -> Self {
        let (to_main, events) = Mailbox::channel();
        let home = WorkerHome::start(to_main).expect("the worker thread starts");
        Self {
            commands: home.commands(),
            home,
            events,
            next_key: Cell::new(1),
        }
    }

    /// Everything this thread is told is group-scoped, so it is addressed to
    /// the group.
    fn tell(&self, command: WorkerCommand) {
        let _ = self.commands.send((None, command));
    }

    fn view(index: u64) -> ViewId {
        test_view(index + 1)
    }

    /// Names one worker on a view, without answering its script yet. The
    /// realm's other half of a construction — telling the painter to fetch —
    /// is what the test does by hand in [`Self::answer`].
    fn construct(&self, view: u64, name: &str) -> WorkerKey {
        let key = WorkerKey::new(self.next_key.get());
        self.next_key.set(key.get() + 1);
        self.tell(WorkerCommand::Start(WorkerStart {
            key,
            view: Self::view(view),
            name: name.to_owned(),
        }));
        key
    }

    /// The painter's half: one script, answered.
    fn answer(&self, key: WorkerKey, url: &str, source: &str) {
        self.tell(WorkerCommand::Script {
            key,
            script: Ok(WorkerScript {
                source: source.to_owned(),
                url: url.to_owned(),
            }),
        });
    }

    /// Constructs a worker on view 0 and answers its script with `source`.
    fn start(&self, source: &str) -> WorkerKey {
        let key = self.construct(0, "");
        self.answer(key, "app:///worker.js", source);
        key
    }

    fn post(&self, key: WorkerKey, data: &str) {
        self.tell(WorkerCommand::Message {
            key,
            data: wire(data),
        });
    }

    fn next(&self) -> WorkerEvent {
        let (view, command) = self
            .events
            .recv(Some(std::time::Instant::now() + PATIENCE))
            .expect("the worker thread has something to report");
        let (Some(view), ToMain::Worker { key, payload }) = (view, command) else {
            panic!("the worker thread sends nothing else, and always addressed")
        };
        WorkerEvent { view, key, payload }
    }

    /// What the worker said, for the tests that only care about that.
    fn message(&self) -> String {
        match self.next().payload {
            WorkerPayload::Message(data) => data,
            WorkerPayload::Errored(error) | WorkerPayload::Failed(error) => {
                panic!("expected a message, got {}", error.message)
            }
            WorkerPayload::Closed => panic!("expected a message, the worker closed"),
        }
    }

    /// Nothing more arrives, and a full round of the thread has passed to
    /// prove it: a `close()` the thread has already served would have to
    /// overtake this message to make the assertion pass by accident.
    fn quiet(&self) {
        let probe = self.construct(0, "");
        self.answer(probe, "app:///probe.js", "postMessage(\"probe\");");
        let event = self.next();
        assert_eq!(event.key, probe, "something else was still to be reported");
        let expected = wire("probe");
        assert!(matches!(event.payload, WorkerPayload::Message(ref data) if *data == expected));
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // The group's own goodbye, which a test has no reason to spell.
        drop(std::mem::replace(&mut self.commands, Mailbox::channel().0));
        self.home.join();
    }
}

#[test]
fn a_worker_answers_what_the_group_posts_and_carries_the_name_it_was_given() {
    let group = Group::new();
    let key = group.construct(0, "counter");
    group.answer(
        key,
        "app:///w.js",
        "onmessage = (event) => postMessage(`${name}:${event.data}`);",
    );
    group.post(key, "ping");
    assert_eq!(group.message(), "[\"counter:ping\"]");
}

#[test]
fn what_is_posted_before_the_script_arrives_is_delivered_in_order() {
    let group = Group::new();
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
    assert_eq!(group.message(), "[\"first\"]");
    assert_eq!(group.message(), "[\"second\"]");
}

#[test]
fn a_script_that_cannot_be_fetched_fails_its_worker_and_nothing_else() {
    let group = Group::new();
    let doomed = group.construct(0, "");
    group.tell(WorkerCommand::Script {
        key: doomed,
        script: Err("404".to_owned()),
    });
    let event = group.next();
    assert_eq!(event.key, doomed);
    let WorkerPayload::Failed(error) = event.payload else {
        panic!("a script that never arrived leaves no worker")
    };
    assert!(
        error.message.contains("loading the worker's script: 404"),
        "{}",
        error.message
    );
    // The runtime is untouched: the next worker over the same thread runs.
    let key = group.start("postMessage(\"alive\");");
    assert_eq!(group.next().key, key);
}

#[test]
fn a_script_that_throws_on_load_leaves_a_worker_that_still_answers() {
    let group = Group::new();
    let key = group.start(
        "onmessage = (event) => postMessage(event.data);
throw new Error(\"boom\");",
    );
    let event = group.next();
    assert_eq!(event.key, key);
    let WorkerPayload::Errored(error) = event.payload else {
        panic!("HTML reports the exception and leaves the worker running")
    };
    assert!(error.message.contains("boom"), "{}", error.message);
    group.post(key, "still here");
    assert_eq!(group.message(), "[\"still here\"]");
}

#[test]
fn a_worker_that_closes_itself_reports_and_takes_no_more() {
    let group = Group::new();
    let key = group.start(
        "onmessage = () => postMessage(\"late\");
close();",
    );
    let event = group.next();
    assert_eq!(event.key, key);
    assert!(matches!(event.payload, WorkerPayload::Closed));
    group.post(key, "anyone?");
    group.quiet();
}

#[test]
fn a_terminated_worker_is_never_heard_from_again() {
    let group = Group::new();
    let key = group.start("onmessage = (event) => postMessage(event.data);");
    group.post(key, "one");
    assert_eq!(group.message(), "[\"one\"]");
    group.tell(WorkerCommand::Terminate { key });
    group.post(key, "two");
    group.quiet();
}

#[test]
fn terminating_a_worker_whose_script_is_still_in_flight_leaves_nothing_behind() {
    let group = Group::new();
    let key = group.construct(0, "");
    group.tell(WorkerCommand::Terminate { key });
    group.answer(key, "app:///w.js", "postMessage(\"too late\");");
    group.quiet();
}

#[test]
fn releasing_a_view_ends_the_workers_it_created() {
    let group = Group::new();
    let key = group.start("onmessage = (event) => postMessage(event.data);");
    group.post(key, "before");
    assert_eq!(group.message(), "[\"before\"]");

    group.tell(WorkerCommand::ReleaseView(Group::view(0)));
    group.post(key, "after");

    // Another view's worker still answers, which is what makes the silence
    // above a release rather than a stopped thread.
    let survivor = group.construct(1, "");
    group.answer(survivor, "app:///other.js", "postMessage(\"alive\");");
    let event = group.next();
    assert_eq!(event.key, survivor);
    assert_eq!(event.view, Group::view(1));
}

#[test]
fn a_worker_keeps_its_own_timers() {
    let group = Group::new();
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
    assert_eq!(group.message(), "[3]");
}

#[test]
fn two_views_over_one_url_each_run_their_own_bytes() {
    let group = Group::new();
    // Every view has its own `ResourceFetcher`, so one URL can resolve to two
    // different scripts in one group. Each worker must run the bytes its own
    // view answered with — which nothing has to arrange, because a worker's
    // script is evaluated into its realm rather than registered on the
    // runtime under a name a second view could reach.
    let first = group.construct(0, "");
    let second = group.construct(1, "");
    group.answer(first, "app:///shared.js", "postMessage(\"first view\");");
    assert_eq!(group.message(), "[\"first view\"]");
    group.answer(second, "app:///shared.js", "postMessage(\"second view\");");
    let event = group.next();
    assert_eq!(event.key, second);
    assert_eq!(
        event.view,
        Group::view(1),
        "an event is routed by the view whose realm created the worker"
    );
    let WorkerPayload::Message(data) = event.payload else {
        panic!("the second view's worker runs the second view's script")
    };
    assert_eq!(data, "[\"second view\"]");
}

#[test]
fn one_worker_realm_shares_no_global_with_another_on_the_same_runtime() {
    let group = Group::new();
    let first = group.start(
        "globalThis.marker = \"first\";
onmessage = () => postMessage(globalThis.marker);",
    );
    let second = group.start("postMessage(String(globalThis.marker));");
    let event = group.next();
    assert_eq!(event.key, second);
    let WorkerPayload::Message(data) = event.payload else {
        panic!("the second worker starts on a global of its own")
    };
    assert_eq!(data, "[\"undefined\"]");
    group.post(first, "ask");
    assert_eq!(group.message(), "[\"first\"]");
}
