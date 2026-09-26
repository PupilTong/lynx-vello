//! What a view is, past the document: readiness, its own clock, and a goodbye
//! that is the channels themselves.
//!
//! The painters here are never pumped and draw nowhere. One is attached
//! wherever a test waits past boot's first `__FlushElementTree`, because that
//! flush parks until a painter binds the view.

use std::time::{Duration, Instant};

use super::*;
use crate::resource::{LoadedSource, SourceCompletion};
use crate::test_support::TestViewSpec;

/// The goodbye is structural: nothing sends it, and nothing can forget to.
///
/// That this test returns at all is half the assertion. Dropping the last
/// view drops the last handle on its group, which joins `bobcat-main`, and
/// that thread cannot return while a view task is still serving.
#[test]
fn dropping_the_view_ends_its_task_and_cancels_its_sources() {
    let view = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          __CreatePage('card', 0);
        };
        ",
    )
    .create_view(Arc::new(NoWakeup));
    // A source the host is still holding when the view goes.
    let (completion, answer) = SourceCompletion::new(view.cancel().clone());
    drop(view);
    assert!(
        completion.is_cancelled(),
        "the flag is set before the view releases the host's fetcher"
    );
    completion.complete(Ok(LoadedSource::Module {
        source: "throw new Error('a cancelled source must not run')".into(),
        url: "app:///late.js".into(),
    }));
    assert!(
        answer.blocking_recv().is_err(),
        "and a late answer reaches nobody"
    );
}

/// A view runs its own clock: once it has booted, its realm's timers come due
/// on `bobcat-main` and their commits publish — with no host call between the
/// boot and the frame beyond the turns the host takes anyway.
///
/// The painter is here only to bind the view. It is never pumped, so nothing
/// in the loop below drives the timer.
#[test]
fn a_view_runs_its_own_timers_with_no_host_call_behind_them() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const box = __CreateView(0);
          __AppendElement(page, box);
          globalThis.held = [page, box];
          setTimeout(() => {
            __SetAttribute(box, 'ticked', 'yes');
            __FlushElementTree();
          }, 20);
          __FlushElementTree();
        };
        ",
    )
    .create(Arc::new(NoWakeup));

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut booted = false;
    while !booted {
        for event in engine.pump() {
            match event {
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry failed: {error}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the view never booted");
        std::thread::yield_now();
    }
    let booted_commit = engine
        .published_frame()
        .expect("boot's flush published a frame")
        .commit_id();

    // Nothing in this loop drives the timer: `published_frame` reads the
    // watch and sends nothing at all.
    loop {
        let commit = engine
            .published_frame()
            .expect("a frame stays published")
            .commit_id();
        if commit != booted_commit {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the timer never came due on the engine's own clock"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        engine
            .probe_document(|tree| {
                let page = tree.document_element().id();
                let box_id = tree.get(page).expect("the page is live").child_ids()[0];
                tree.get(box_id)
                    .and_then(|live| live.attribute("ticked").map(str::to_owned))
            })
            .expect("the view's task answers probes")
            .as_deref(),
        Some("yes"),
        "and the entry the timer ran in committed its mutation"
    );
}

/// A view boots, commits and publishes with no host turn behind it, which is
/// what makes its whole startup overlap the painter the embedder builds next.
///
/// Nothing here pumps the view before the assertion, and nothing could: the
/// author sheets and the entry were handed to the fetcher inside
/// `create_lynx_view`, and this host answers them in that same call, so the
/// realm opens, the entry runs and its flush commits on `bobcat-main` alone.
/// Attaching the painter is not a turn either — it writes the metrics watch,
/// which is what releases that first flush. The host's first `pump` only
/// collects what already happened.
#[test]
fn a_view_boots_and_publishes_before_the_host_takes_a_turn() {
    let mut engine = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          __AppendElement(page, __CreateView(0));
        };
        ",
    )
    .create(Arc::new(NoWakeup));

    let deadline = Instant::now() + Duration::from_secs(30);
    while engine.published_frame().is_none() {
        assert!(
            Instant::now() < deadline,
            "boot never published a frame without a host turn"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    let mut booted = false;
    while !booted {
        for event in engine.pump() {
            match event {
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry failed: {error}"),
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "the first turn never reported the boot it found finished"
        );
        std::thread::yield_now();
    }
}

#[test]
fn global_events_require_observed_readiness_and_rejected_events_are_not_replayed() {
    let mut engine = TestViewSpec::new(
        r"
        import {Worker} from 'bobcat-internal';
        const post = Worker.prototype.postMessage;
        Worker.prototype.postMessage = function (message) {
            if (message.method === 'sendGlobalEvent') console.log(message.args);
            return post.call(this, message);
        };
        __CreatePage();
    ",
    )
    .create(Arc::new(NoWakeup));
    assert!(!engine.view.is_ready());
    assert!(matches!(
        engine
            .view
            .send_global_event("event", r#"["early"]"#.into()),
        Err(EngineError::NotReady)
    ));
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut messages = Vec::new();
    loop {
        let events = engine.pump();
        for event in &events {
            if let EngineEvent::ConsoleMessage { message, .. } = event {
                messages.push(message.clone());
            }
        }
        if events
            .iter()
            .any(|event| matches!(event, EngineEvent::ScriptFinished))
        {
            assert!(
                engine.view.is_ready(),
                "pump records readiness before returning the event"
            );
            break;
        }
        assert!(!engine.view.is_ready());
        assert!(
            Instant::now() < deadline,
            "view never became ready: {events:?}"
        );
        std::thread::yield_now();
    }
    assert!(
        messages.is_empty(),
        "rejected event was replayed during boot"
    );
    engine
        .view
        .send_global_event("event", r#"["accepted"]"#.into())
        .unwrap();
    while messages.is_empty() {
        for event in engine.pump() {
            if let EngineEvent::ConsoleMessage { message, .. } = event {
                messages.push(message);
            }
        }
        assert!(
            Instant::now() < deadline,
            "accepted event did not reach the Worker"
        );
        std::thread::yield_now();
    }
    assert_eq!(messages, [r#"["accepted"]"#]);
    engine.view.cancel.cancel();
    assert!(!engine.view.is_ready());
    assert!(matches!(
        engine.view.send_global_event("event", "[]".into()),
        Err(EngineError::NotReady)
    ));
}

#[test]
fn failed_startup_never_makes_a_view_ready() {
    let mut view =
        TestViewSpec::new("throw Error('cannot start');").create_view(Arc::new(NoWakeup));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let events = view.pump();
        assert!(!view.is_ready());
        assert!(matches!(
            view.send_global_event("event", "[]".into()),
            Err(EngineError::NotReady)
        ));
        if events
            .iter()
            .any(|event| matches!(event, EngineEvent::StartupFailed(_)))
        {
            break;
        }
        assert!(Instant::now() < deadline, "startup never failed");
        std::thread::yield_now();
    }
}
