use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{FarEnd, Painter, ToMain};
use crate::main::tree::{LynxDocument, PageConfig, Viewport, new_document};
use crate::resource::SourceRequest;
use crate::test_support::TestViewSpec;
use crate::view::{EngineEvent, EventRequester, FrameSize, NoWakeup};

/// A phone-shaped document, for the tests that publish a real frame.
fn document() -> LynxDocument {
    new_document(Viewport::new(393.0, 727.0), PageConfig::default())
}

/// A painter with every seam built but no view task: the view's whole side of
/// its link is handed back so a test can play it.
fn detached() -> (Painter, FarEnd) {
    detached_waking(Arc::new(NoWakeup))
}

fn detached_waking<R: EventRequester>(requester: Arc<R>) -> (Painter, FarEnd) {
    Painter::detached(393.0, 727.0, requester)
}

#[test]
fn frame_size_applies_the_device_scale_once() {
    let size = FrameSize::for_viewport(393.0, 727.0, 2.0).unwrap();
    assert_eq!((size.width, size.height), (786, 1_454));
}

#[test]
fn frame_size_rejects_unbounded_targets() {
    let error = FrameSize::for_viewport(20_000.0, 100.0, 1.0).unwrap_err();
    assert!(error.to_string().contains("16384"));
}

/// One poll applies every kind of thing that reaches the painting side — a
/// listener edge, a `BeginFrame` acknowledgement, and the redraw a new frame
/// asks for — and applies each of them exactly once.
#[test]
fn one_poll_adopts_every_kind_of_published_state() {
    let (mut painter, main) = detached();
    assert!(!painter.take_redraw(), "a fresh painter owes no frame");

    main.outbox.listener_edge(Arc::from("tap"), true);
    main.outbox.begin_frame_serviced(7);
    main.outbox.publish_frame(document().commit());

    painter.poll_link();
    assert!(painter.published.listeners.contains("tap"));
    assert!(
        painter.wait_begin_frame(7, Duration::ZERO),
        "the acknowledgement rode the same state, so the wait never blocks"
    );
    assert!(painter.take_redraw(), "an announced frame asks for a draw");
    assert!(!painter.take_redraw(), "and the request is taken once");
}

/// Frames do not queue: the published state holds one, so a painter that
/// polls after several commits sees the newest and never the ones it slept
/// through.
#[test]
fn the_published_state_keeps_only_the_newest_commit() {
    let (mut painter, main) = detached();
    let mut document = document();
    let first = document.commit();
    document.set_viewport(320.0, 640.0);
    let second = document.commit();
    assert_ne!(first.commit_id(), second.commit_id());

    main.outbox.publish_frame(Arc::clone(&first));
    main.outbox.publish_frame(Arc::clone(&second));
    painter.poll_link();

    let published = painter.frame().expect("the poll adopted a frame");
    assert_eq!(
        published.commit_id(),
        second.commit_id(),
        "the second publish overwrote the first rather than queueing behind it"
    );
}

/// The last frame a view published is still the frame to draw once its task
/// has ended: a closed channel is not an empty one.
///
/// What the painter had to have done first is read that commit's pixels,
/// which is what adopting one is — the poll below the publish. Afterwards
/// nothing of the view is needed: the frame, its bitmaps and everything
/// composed from them are the painter's.
#[test]
fn a_frame_published_before_the_task_ended_is_still_adopted() {
    let (mut painter, main) = detached();
    let frame = document().commit();
    main.outbox.publish_frame(Arc::clone(&frame));
    painter.poll_link();
    drop(main);

    painter.poll_link();
    assert_eq!(
        painter
            .frame()
            .expect("the last frame outlives its publisher")
            .commit_id(),
        frame.commit_id()
    );
    assert!(painter.take_redraw(), "and it still asks to be drawn");
    assert!(
        !painter.is_attached(),
        "the released view detached the painter on that same poll"
    );
}

/// A commit and the pixels it draws are adopted together, so a commit whose
/// pixels can no longer be read is not adopted at all.
///
/// The store is the view's and goes when the view does. A frame indexes that
/// store's bitmaps by draw order, so taking the newer frame over the older
/// one's table would draw the older commit's images — the painter keeps the
/// last commit it did read, whole, instead.
#[test]
fn a_commit_whose_pixels_can_no_longer_be_read_is_not_adopted() {
    let (mut painter, main) = detached();
    let mut document = document();
    let first = document.commit();
    document.set_viewport(320.0, 640.0);
    let second = document.commit();

    main.outbox.publish_frame(Arc::clone(&first));
    painter.poll_link();
    assert_eq!(
        painter
            .frame()
            .expect("the first commit is adopted with its pixels")
            .commit_id(),
        first.commit_id()
    );

    // Published, and then the view released before the painter took a turn:
    // the frame is still on the watch, but nothing can answer for its images.
    main.outbox.publish_frame(Arc::clone(&second));
    drop(main);
    painter.poll_link();
    assert_eq!(
        painter
            .frame()
            .expect("the painter still has the commit it read")
            .commit_id(),
        first.commit_id(),
        "the newer commit's store went with the view, so it was left behind"
    );
    assert!(
        !painter.is_attached(),
        "and the released view detached the painter on that same poll"
    );
}

/// An emit decision costs one name-table lookup and stops there unless a
/// listener exists; when it crosses, it crosses as plain data — the
/// target id, not a path. Liveness is the main thread's to check at
/// delivery.
#[test]
fn an_emit_decision_crosses_only_when_a_listener_wants_it() {
    use super::gesture::{EmitEvent, InputDecision, InputDecisions, TAP_EVENT};

    let (mut painter, mut main) = detached();
    // The permanent page element's packed handle, as script would name it.
    let target = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    let emit = |painter: &mut Painter| {
        let mut decisions = InputDecisions::new();
        decisions.push(InputDecision::Emit(EmitEvent {
            name: TAP_EVENT,
            target,
            position: dom::Point2D::new(1.0, 1.0),
            wheel: None,
        }));
        painter.execute_decisions(&mut decisions, None);
        assert!(decisions.is_empty(), "the queue is always drained");
    };

    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "an empty listener set sends nothing"
    );

    main.outbox.listener_edge(Arc::from("pointerup"), true);
    painter.poll_link();
    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "a listener on another name sends nothing"
    );

    // An update that has not been polled for yet is not yet visible: the
    // snapshot moves at pass boundaries, which is the one pass of
    // staleness this design accepts in exchange for the lock.
    main.outbox.listener_edge(Arc::from(TAP_EVENT), true);
    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "an unadopted registration does not open the name mid-pass"
    );

    painter.poll_link();
    emit(&mut painter);
    let command = main
        .commands
        .try_recv()
        .expect("the listened-for name crosses");
    let ToMain::DispatchEvent {
        name, target: sent, ..
    } = command
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(name, TAP_EVENT);
    assert_eq!(sent, target);

    // And the edge closes the name again: the main thread publishes the
    // last removal, and from the next poll nothing crosses.
    main.outbox.listener_edge(Arc::from(TAP_EVENT), false);
    painter.poll_link();
    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "the closing edge stops the crossing"
    );
}

/// A poll adopts whatever arrived, in order, and stops at the last of
/// it — never blocking on a main thread that may be mid-registration.
///
/// The ordering matters because the two edges for one name are a pair:
/// a name registered and unregistered between two passes must leave the
/// snapshot closed, not open.
#[test]
fn a_poll_adopts_arrived_edges_in_order_and_does_not_block() {
    let (mut painter, main) = detached();

    painter.poll_link();
    assert!(
        !painter.published.listeners.contains("tap"),
        "nothing has been published yet"
    );

    main.outbox.listener_edge(Arc::from("tap"), true);
    main.outbox.listener_edge(Arc::from("scroll"), true);
    main.outbox.listener_edge(Arc::from("tap"), false);
    painter.poll_link();
    assert!(
        !painter.published.listeners.contains("tap"),
        "the closing edge follows the opening one and wins"
    );
    assert!(
        painter.published.listeners.contains("scroll"),
        "the other name stays open"
    );

    // The main thread going away is not an unregistration: what it
    // published last is still the last true answer.
    drop(main);
    painter.poll_link();
    assert!(
        painter.published.listeners.contains("scroll"),
        "a closed channel leaves the snapshot standing"
    );
}

/// A scroll decision crosses nothing: it lands in the painting side's
/// intents, which are the offsets composition shows, and the main
/// thread hears about scrolling only when a refill writes offsets back.
/// With no published frame there is no geometry to consume against, so
/// the decision evaporates entirely.
#[test]
fn a_scroll_decision_sends_no_command() {
    use super::gesture::{InputDecision, InputDecisions};

    let (mut painter, mut main) = detached();
    let node = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: node,
        delta: dom::Vector2D::new(0.0, 5.0),
    });
    painter.execute_decisions(&mut decisions, None);
    assert!(
        main.commands.try_recv().is_err(),
        "a windowed scroll never crosses the command channel"
    );
    assert!(painter.scroll_intents.offsets.is_empty());
}

/// A requester that records every wake, so a test can wait on the host
/// loop actually being asked to run.
struct WakeSignal(flume::Sender<()>);

impl EventRequester for WakeSignal {
    fn request_event(&self) {
        let _ = self.0.send(());
    }
}

/// A painter with nowhere to present owes the display nothing, whatever else
/// is true of it.
///
/// The rule that keeps a host from waiting on a frame clock for a view that
/// has no display to keep up with: an offscreen painter's frames are asked
/// for through `tick`, and one with no target at all has none to give.
#[test]
fn a_painter_that_presents_to_no_window_owes_no_frame() {
    let (painter, _main) = detached();
    painter.refresh();
    assert!(
        painter.redraw_pending.get(),
        "the refresh did leave a frame owed"
    );
    assert!(
        !painter.owes_frame(),
        "but nothing owes the display a frame it cannot present"
    );
}

/// A frame the painter asks of itself wakes nothing.
///
/// Every caller of `refresh` is the painter, on the host's own thread and
/// inside the host's own call, so the turn that host is already in is what
/// answers it. A wake there would post into the loop that is running — a
/// turn that finds nothing, asks for another, and never stops. Only the main
/// thread's publish wakes.
#[test]
fn a_self_directed_frame_request_wakes_nobody() {
    let (wake_sender, wakes) = flume::unbounded();
    let (painter, main) = detached_waking(Arc::new(WakeSignal(wake_sender)));

    painter.refresh();
    assert!(
        wakes.try_recv().is_err(),
        "a painter-local request is answered by the turn it was made in"
    );
    assert!(painter.take_redraw(), "and the frame is still owed");

    main.outbox.publish_frame(document().commit());
    assert!(
        wakes.try_recv().is_ok(),
        "a commit from the other thread is the one thing that must wake"
    );
    assert!(wakes.try_recv().is_err(), "and it wakes exactly once");
}

/// Boot's final flush is a commit: by the time `ScriptFinished` is
/// pumped, a frame is published and the document — owned by the view's own
/// task — answers probes.
#[test]
fn a_booted_view_commits_and_publishes() {
    let (wake_sender, wake_receiver) = flume::unbounded();
    let mut engine = TestViewSpec::new(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              __AppendElement(page, __CreateView(0));
              __FlushElementTree();
              __AppendElement(page, __CreateView(0));
            };
            ",
    )
    .create(Arc::new(WakeSignal(wake_sender)));

    // One wakeup carries either kind of engine work: the boot commit's
    // frame or a lifecycle event. The law under test is the ordering —
    // whichever wakeup carries the event, `pump` observes it right then,
    // with nothing polled for and nothing slept on.
    let deadline = Instant::now() + Duration::from_secs(30);
    let finished = loop {
        wake_receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("script completion must wake the host event loop");
        if let Some(event) = engine.pump().into_iter().find(|event| {
            matches!(
                event,
                EngineEvent::ScriptFinished | EngineEvent::ScriptRunError(_)
            )
        }) {
            break event;
        }
        assert!(
            Instant::now() < deadline,
            "no wakeup ever carried the script-completion event"
        );
    };
    assert!(matches!(finished, EngineEvent::ScriptFinished));

    let frame = engine
        .published_frame()
        .expect("the boot's flush published a committed frame");
    assert!(frame.commit_id() > 0);

    let (views, connected, laid_out) = engine
        .probe_document(|tree| {
            let page = tree.document_element().id();
            let views = tree
                .get(page)
                .expect("the page is live")
                .child_ids()
                .to_vec();
            let connected = views.iter().all(|&view| tree.is_connected(view));
            (views.len(), connected, tree.rounded_layout(page).is_some())
        })
        .expect("the view's task answers probes");
    assert_eq!(views, 2, "the boot script appends two views");
    assert!(connected, "both views are attached");
    assert!(laid_out, "the boot's final flush laid the page out");
}

/// A refused render clears the painter's record, so nothing is skipped against
/// it and nothing is read out of it afterwards.
///
/// The rule that keeps the one record from outliving the render that made it:
/// an offscreen target gives its texture up when a render fails partway, so a
/// record still naming that frame would skip the next render of the same commit
/// and then hand a reader pixels that are not there. The record is cleared on
/// any refusal, whatever the target still holds.
///
/// The refusal is a zero-sized target, which is the one render failure a test
/// can ask for without a broken GPU.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_failed_render_leaves_the_painter_holding_no_frame() {
    use crate::view::EngineError;

    let mut engine = TestViewSpec::new(
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              const box = __CreateView(0);
              __SetInlineStyles(box, 'width:32px;height:24px;background-color:#ff0000');
              __AppendElement(page, box);
            };
            ",
    )
    .offscreen(32.0, 24.0)
    .boot();
    let size = engine.painter.frame_size;
    engine.tick(true).expect("the committed frame renders");
    assert!(engine.capture().is_ok(), "and reads back");

    engine.painter.frame_size = FrameSize {
        width: 0,
        height: 0,
    };
    let error = engine
        .tick(true)
        .expect_err("a zero-sized target is refused");
    assert!(matches!(error, EngineError::Gpu(_)), "{error}");

    // Detached and back at a size the GPU accepts: there is no frame to
    // re-render now, so what the capture answers is purely what the painter's
    // record says.
    engine.painter.detach();
    engine.painter.frame_size = size;
    let error = engine
        .capture()
        .expect_err("the failed render left nothing to capture");
    assert!(
        matches!(error, EngineError::Render(message) if message.contains("no frame has been rendered")),
        "the refused render cleared the record, so the capture is refused before any readback"
    );
}

/// Two painters, each observing a view of its own and nothing between them.
fn two_painters() -> [(Painter, FarEnd); 2] {
    [detached(), detached()]
}

/// The property the old addressed FIFO had to buy with a per-view buffer:
/// one view's turn neither consumes nor delays anything belonging to
/// another, whatever order the two published in.
#[test]
fn a_views_published_state_is_independent_of_a_siblings() {
    let [(mut first, first_end), (mut second, second_end)] = two_painters();

    second_end.outbox.listener_edge(Arc::from("tap"), true);
    let _asked = second_end
        .outbox
        .request_source(SourceRequest::Entry("second.js".into()));
    first_end.outbox.publish_frame(document().commit());

    // The first painter's poll sees exactly its own view's frame, and none of
    // the sibling's listener names.
    first.poll_link();
    assert!(first.published_frame().is_some());
    assert!(!first.published.listeners.contains("tap"));

    // The sibling's is still there afterwards, and still there once the last
    // sender on its side is gone.
    drop(first_end);
    drop(second_end);
    second.poll_link();
    assert!(second.published.listeners.contains("tap"));
    assert!(
        second.published_frame().is_none(),
        "and no frame reached it, because none was published to it"
    );
}

/// An offscreen wait is this painter's own: nothing another view acknowledges
/// can satisfy it, and a view that has gone ends it rather than letting it
/// wait out its whole deadline.
#[test]
fn an_offscreen_wait_takes_only_its_own_acknowledgement_and_ends_with_its_view() {
    let [(mut first, first_end), (_second, second_end)] = two_painters();
    let mut first_end = first_end;
    let seq = first
        .begin_frame(0.0, true)
        .expect("the view's task is listening");
    assert!(matches!(
        first_end.commands.blocking_recv(),
        Some(ToMain::BeginFrame { .. })
    ));

    second_end.outbox.begin_frame_serviced(seq);
    assert!(
        !first.wait_begin_frame(seq, Duration::ZERO),
        "a sibling's acknowledgement is not this view's"
    );

    first_end.outbox.begin_frame_serviced(seq);
    assert!(
        first.wait_begin_frame(seq, Duration::ZERO),
        "and its own satisfies it without blocking"
    );

    // A `BeginFrame` nobody will service: the view is gone, so nothing will
    // acknowledge it, and waiting the ten seconds out would be ten seconds of
    // a host's own thread.
    drop(first_end);
    let started = Instant::now();
    assert!(!first.wait_begin_frame(seq + 1, Duration::from_secs(10)));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the wait outlasted the view that ended it"
    );
    assert!(
        !first.is_attached(),
        "and the released view detached the painter"
    );
}
