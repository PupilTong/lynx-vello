//! What a view is on its own: a running page with no painter anywhere near
//! it, and a goodbye that is the channels themselves.

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
    completion.complete(Ok(LoadedSource::Entry {
        source: "throw new Error('a cancelled source must not run')".into(),
        url: "app:///late.js".into(),
    }));
    assert!(
        answer.blocking_recv().is_err(),
        "and a late answer reaches nobody"
    );
}

/// A view with no painter is a running view: it boots, its realm's timers
/// come due on `bobcat-main`, and their commits publish — with no host call
/// between the boot and the frame beyond the turns the host takes anyway.
#[test]
fn a_view_with_no_painter_boots_and_runs_its_own_timers() {
    let mut view = TestViewSpec::new(
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
    .create_view(Arc::new(NoWakeup));

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut booted = false;
    while !booted {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry failed: {error}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "a painterless view never booted");
        std::thread::yield_now();
    }
    let booted_commit = view
        .published_frame()
        .expect("boot's flush published a frame")
        .commit_id();

    // Nothing in this loop drives the timer: `published_frame` reads the
    // watch and sends nothing at all.
    loop {
        let commit = view
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
        view.probe_document(|tree| {
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

#[test]
fn global_events_require_observed_readiness_and_rejected_events_are_not_replayed() {
    let mut view = TestViewSpec::new(
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
    .create_view(Arc::new(NoWakeup));
    assert!(!view.is_ready());
    assert!(matches!(
        view.send_global_event("event", vec!["early".into()]),
        Err(EngineError::NotReady)
    ));
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut messages = Vec::new();
    loop {
        let events = view.pump();
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
                view.is_ready(),
                "pump records readiness before returning the event"
            );
            break;
        }
        assert!(!view.is_ready());
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
    view.send_global_event("event", vec!["accepted".into()])
        .unwrap();
    while messages.is_empty() {
        for event in view.pump() {
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
    view.cancel.cancel();
    assert!(!view.is_ready());
    assert!(matches!(
        view.send_global_event("event", vec![]),
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
            view.send_global_event("event", vec![]),
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
