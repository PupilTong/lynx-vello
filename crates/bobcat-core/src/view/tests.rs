//! What one view's link is, now that it is one view's: no addressing, no
//! sibling buffering, and a goodbye that is the channels themselves.

use std::time::{Duration, Instant};

use super::*;
use crate::paint::{DetachedEnds, TestPainter};
use crate::resource::{LoadedSource, SourceCompletion, SourceRequest};
use crate::test_support::TestViewSpec;

fn viewport() -> Viewport {
    Viewport::new(393.0, 727.0)
}

fn frame_size() -> FrameSize {
    FrameSize::for_viewport(393.0, 727.0, 1.0).expect("the test viewport is valid")
}

/// Two views, each with its own link and nothing between them.
fn two_views() -> [(TestPainter, DetachedEnds); 2] {
    [
        TestPainter::detached(viewport(), frame_size(), Arc::new(NoWakeup)),
        TestPainter::detached(viewport(), frame_size(), Arc::new(NoWakeup)),
    ]
}

/// The property the old addressed FIFO had to buy with a per-view buffer:
/// one view's turn neither consumes nor delays anything belonging to
/// another, whatever order the two published in.
#[test]
fn a_views_notices_and_frames_are_independent_of_a_siblings() {
    let [(mut first, first_end), (mut second, second_end)] = two_views();

    second_end.outbox.listener_edge(Arc::from("tap"), true);
    first_end.outbox.engine_event(EngineEvent::ScriptFinished);
    let _asked = second_end
        .outbox
        .request_source(SourceRequest::Entry("second.js".into()));
    second_end.outbox.engine_event(EngineEvent::ScriptFinished);

    // The first view's turn sees exactly its own one event, and takes it
    // once.
    assert!(matches!(
        first.pump().as_slice(),
        [EngineEvent::ScriptFinished]
    ));
    assert!(first.pump().is_empty());
    assert!(!first.link.has_listener("tap"), "and none of the sibling's");

    // The sibling's is still there afterwards, in the order it was
    // published, and still there after the last sender is gone.
    drop(first_end);
    drop(second_end);
    assert!(matches!(
        second.pump().as_slice(),
        [EngineEvent::ScriptFinished]
    ));
    assert!(second.link.has_listener("tap"));
    assert!(second.pump().is_empty());
}

/// The goodbye is structural: nothing sends it, and nothing can forget to.
///
/// That this test returns at all is half the assertion. Dropping the last
/// view drops the last handle on its group, which joins `bobcat-main`, and
/// that thread cannot return while a view task is still serving.
#[test]
fn dropping_the_last_embedder_sender_ends_the_view_and_cancels_its_sources() {
    let view = TestViewSpec::new(
        r"
        globalThis.renderPage = function () {
          __CreatePage('card', 0);
        };
        ",
    )
    .create(Arc::new(NoWakeup));
    // A source the host is still holding when the view goes.
    let (completion, answer) = SourceCompletion::new(view.cancel().clone());
    drop(view);
    assert!(
        completion.is_cancelled(),
        "the flag is set before the painter releases the host's fetcher"
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

/// An offscreen wait is this view's own: nothing another view acknowledges
/// can satisfy it, and a fatal event ends it rather than letting it wait out
/// its whole deadline.
#[test]
fn an_offscreen_wait_takes_only_its_own_acknowledgement_and_ends_on_a_failure() {
    let [(mut first, first_end), (_second, second_end)] = two_views();
    let mut first_end = first_end;
    let seq = first
        .link
        .begin_frame(0.0)
        .expect("the view's task is listening");
    assert!(matches!(
        first_end.commands.blocking_recv(),
        Some(ToMain::BeginFrame { .. })
    ));

    second_end.outbox.begin_frame_serviced(seq);
    assert!(
        !first.link.wait_begin_frame(seq, Duration::ZERO),
        "a sibling's acknowledgement is not this view's"
    );

    first_end.outbox.engine_event(EngineEvent::StartupFailed(
        EngineError::UnknownFontFamily("missing".into()).into(),
    ));
    let started = Instant::now();
    assert!(
        !first.link.wait_begin_frame(seq, Duration::from_secs(10)),
        "and a failure ends the wait rather than outlasting it"
    );
    // Ended by the event rather than by the deadline: nothing will service
    // the round after a fatal one, so waiting the ten seconds out would be
    // ten seconds of a host's own thread.
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the wait outlasted the failure that ended it"
    );
    assert!(matches!(
        first.pump().as_slice(),
        [EngineEvent::StartupFailed(_)]
    ));

    // The acknowledgement it was owed does arrive, and satisfies the next
    // wait: the failure ended the wait, not the link.
    first_end.outbox.begin_frame_serviced(seq);
    assert!(first.link.wait_begin_frame(seq, Duration::ZERO));
}
