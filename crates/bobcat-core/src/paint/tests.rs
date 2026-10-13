use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{FarEnd, Painter, ToMain};
use crate::link::{FramePost, ScrollEntry, ScrollMailbox};
use crate::main::tree::{
    Autoplays, ComponentEvents, LynxDocument, PageConfig, Viewport, new_document,
};
use crate::resource::SourceRequest;
use crate::test_support::TestViewSpec;
use crate::view::{EngineEvent, EventRequester, FrameSize, NoWakeup};

/// A phone-shaped document, for the tests that publish a real frame.
fn document() -> LynxDocument {
    new_document(
        Viewport::new(393.0, 727.0),
        PageConfig::default(),
        ComponentEvents::default(),
        Autoplays::default(),
    )
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
fn script_frame_demand_waits_for_host_vsync_even_before_the_first_commit() {
    let (mut painter, mut main) = detached();
    main.seat.frame_demand.borrow_mut().set(None, true);
    assert!(painter.is_animating());
    painter.pump().unwrap();
    painter.begin_frame(1.0, true);
    assert!(matches!(main.commands.try_recv().unwrap(), ToMain::Posted));
    assert!(main.commands.try_recv().is_err());
    assert!(
        main.scroll
            .take(&mut rustc_hash::FxHashMap::default())
            .is_some(),
        "the marker stands for a frame post"
    );

    painter.vsync();
    assert!(matches!(
        main.commands.try_recv().unwrap(),
        ToMain::Vsync(_)
    ));
    assert!(!painter.is_animating());

    main.seat.frame_demand.borrow_mut().set(None, true);
    main.seat.frame_demand.borrow_mut().set(None, false);
    assert!(
        !painter.is_animating(),
        "cancellation withdraws the vsync request"
    );
    painter.vsync();
    assert!(main.commands.try_recv().is_err());
}

#[test]
fn worker_frame_demand_notifies_only_that_worker_and_does_not_keep_it_alive() {
    let (mut painter, mut main) = detached();
    let (messages, mut worker) = tokio::sync::mpsc::unbounded_channel();
    let key = crate::background::WorkerKey::new(1);
    main.seat
        .frame_demand
        .borrow_mut()
        .register_worker(key, messages.downgrade());
    main.seat.frame_demand.borrow_mut().set(Some(key), true);
    assert!(painter.is_animating());
    painter.vsync();
    assert!(matches!(
        worker.try_recv().unwrap(),
        crate::background::WorkerMessage::Vsync(_)
    ));
    assert!(
        main.commands.try_recv().is_err(),
        "BTS rAF needs no MTS message"
    );
    assert!(!painter.is_animating());
    main.seat.frame_demand.borrow_mut().set(Some(key), true);
    drop(messages);
    assert!(
        !painter.is_animating(),
        "frame demand does not keep a Worker handle alive"
    );
    assert!(matches!(
        worker.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
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
/// listener edge, a frame post acknowledgement, and the redraw a new frame
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
    use super::gesture::{EmitEvent, InputDecision, InputDecisions, TAP_EVENT, TouchPoints};

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
            touches: TouchPoints::new(),
        }));
        painter.execute_decisions(&mut decisions, None, 0.25);
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
        name,
        target: sent,
        payload,
    } = command
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(name, TAP_EVENT);
    assert_eq!(sent, target);
    // The decision's own facts, as values: nothing is encoded on this side.
    assert_eq!(payload.position, dom::Point2D::new(1.0, 1.0));
    assert_eq!(payload.wheel, None, "a tap carries no wheel delta");
    assert!(payload.touches.is_empty(), "a tap carries no touch points");
    // The pass's clock reading, in milliseconds.
    assert!((payload.timestamp - 250.0).abs() < f64::EPSILON);

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

/// The two payloads a plain pointer event does not have — the wheel delta and
/// the touch points — cross as the values the router decided, in the order it
/// decided them. The realm is the side that gives them a shape.
#[test]
fn a_wheel_delta_and_touch_points_cross_as_values() {
    use super::gesture::{
        EmitEvent, InputDecision, InputDecisions, TOUCH_ACTIVE, TOUCH_CHANGED, TOUCH_TARGET,
        TouchPoint, TouchPoints,
    };

    let (mut painter, mut main) = detached();
    let target = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    main.outbox.listener_edge(Arc::from("wheel"), true);
    main.outbox.listener_edge(Arc::from("touchmove"), true);
    painter.poll_link();

    let mut touches = TouchPoints::new();
    touches.push(TouchPoint {
        identifier: 1,
        position: dom::Point2D::new(10.0, 20.0),
        flags: TOUCH_ACTIVE,
    });
    touches.push(TouchPoint {
        identifier: 2,
        position: dom::Point2D::new(30.5, 40.0),
        flags: TOUCH_ACTIVE | TOUCH_TARGET | TOUCH_CHANGED,
    });
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Emit(EmitEvent {
        name: "wheel",
        target,
        position: dom::Point2D::new(5.0, 6.0),
        wheel: Some(dom::Vector2D::new(0.0, 30.0)),
        touches: TouchPoints::new(),
    }));
    decisions.push(InputDecision::Emit(EmitEvent {
        name: "touchmove",
        target,
        position: dom::Point2D::new(30.5, 40.0),
        wheel: None,
        touches: touches.clone(),
    }));
    painter.execute_decisions(&mut decisions, None, 0.5);

    let ToMain::DispatchEvent { payload, .. } = main
        .commands
        .try_recv()
        .expect("the wheel event crosses first")
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(payload.wheel, Some(dom::Vector2D::new(0.0, 30.0)));
    assert_eq!(payload.position, dom::Point2D::new(5.0, 6.0));

    let ToMain::DispatchEvent { payload, .. } =
        main.commands.try_recv().expect("then the touch event")
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(payload.touches.as_slice(), touches.as_slice());
    assert_eq!(payload.wheel, None);
    assert!((payload.timestamp - 500.0).abs() < f64::EPSILON);
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

/// A scroll decision with no published frame has no geometry to consume
/// against, so it evaporates entirely: no intent, nothing posted, no
/// marker.
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
    painter.execute_decisions(&mut decisions, None, 0.0);
    painter.publish_scroll(0.0, None);
    assert!(
        main.commands.try_recv().is_err(),
        "nothing moved, so nothing crosses"
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
/// for through `post_frame`, and one with no target at all has none to give.
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
                EngineEvent::ScriptFinished
                    | EngineEvent::ScriptRunError(_)
                    | EngineEvent::Panicked(_)
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
        .request_source(SourceRequest::Module("app:///second.js".into()));
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

/// What a host's own wait reads between posting a frame and drawing it:
/// whether the view has serviced the post, answered at once either way. It is
/// `true` as well when nothing will ever service it — nothing was posted, or
/// the view has gone — so that wait is never one on a view that is not there.
#[test]
fn a_posted_frame_reads_as_serviced_once_acknowledged_or_once_nothing_will() {
    let (mut painter, end) = detached();
    let seq = painter
        .begin_frame(0.0, true)
        .expect("the view's task is listening");
    let posted = super::PostedFrame {
        now: 0.0,
        seq: Some(seq),
    };

    let started = Instant::now();
    assert!(
        !painter.frame_serviced(&posted),
        "nothing has serviced the post yet"
    );
    end.outbox.begin_frame_serviced(seq - 1);
    assert!(
        !painter.frame_serviced(&posted),
        "an earlier post's acknowledgement is not this one's"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "asking is not waiting"
    );

    end.outbox.begin_frame_serviced(seq);
    assert!(painter.frame_serviced(&posted));

    let nothing_posted = super::PostedFrame {
        now: 0.0,
        seq: None,
    };
    assert!(
        painter.frame_serviced(&nothing_posted),
        "a frame that posted nothing has nothing to wait for"
    );

    let unserviced = super::PostedFrame {
        now: 0.1,
        seq: Some(seq + 1),
    };
    assert!(!painter.frame_serviced(&unserviced));
    drop(end);
    assert!(
        painter.frame_serviced(&unserviced),
        "nothing will service a post once its view has gone"
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
        Some(ToMain::Posted)
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

    // A frame post nobody will service: the view is gone, so nothing will
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

/// A frame over one 200px column scroller of 1000px content at the origin,
/// its style extended by `css`, and the scroller.
fn scrolling_frame(css: &str) -> (Arc<dom::CommittedFrame>, dom::NodeId) {
    let mut document = document();
    let root = document.document_element().id();
    let scroller = document.create_element("view", ());
    document.set_inline_style(
        scroller,
        &format!(
            "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px;{css}"
        ),
    );
    document.append_child(root, scroller);
    let filler = document.create_element("view", ());
    document.set_inline_style(filler, "flex-shrink:0;width:200px;height:1000px");
    document.append_child(scroller, filler);
    let frame = document.commit();
    assert!(frame.slot_of(scroller).is_some(), "the scroller has a slot");
    (frame, scroller)
}

/// A detached painter that has adopted [`scrolling_frame`], at clock 0.
fn scrolling(css: &str) -> (Painter, FarEnd, dom::NodeId) {
    let (mut painter, main) = detached();
    let (frame, scroller) = scrolling_frame(css);
    main.outbox.publish_frame(frame);
    painter.poll_link();
    painter.clock.pin(0.0);
    (painter, main, scroller)
}

fn touch(painter: &mut Painter, phase: dom::input::PointerPhase, y: f32) {
    painter.dispatch_input(dom::input::InputEvent::pointer(
        dom::Point2D::new(100.0, y),
        1,
        dom::input::PointerKind::Touch,
        phase,
    ));
}

/// How many markers the FIFO holds; anything else in it fails.
fn markers(main: &mut FarEnd) -> usize {
    std::iter::from_fn(|| main.commands.try_recv().ok())
        .map(|command| assert!(matches!(command, ToMain::Posted), "only markers"))
        .count()
}

/// What main takes at a marker.
fn take(main: &FarEnd) -> (Vec<(dom::NodeId, ScrollEntry)>, Option<FramePost>) {
    let mut spare = rustc_hash::FxHashMap::default();
    let frame = main.scroll.take(&mut spare);
    (spare.into_iter().collect(), frame)
}

/// A drag posts its container once and sends one marker; however many steps
/// follow before main takes, the entry is overwritten and no second marker
/// queues. A take re-arms it.
#[test]
fn a_drag_queues_one_marker_until_main_takes() {
    use dom::input::PointerPhase::{Down, Move};

    let (mut painter, mut main, scroller) = scrolling("");
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    assert_eq!(markers(&mut main), 1, "the first step sends the marker");
    for step in 1..=10_u8 {
        touch(&mut painter, Move, 120.0 - f32::from(step));
    }
    assert_eq!(markers(&mut main), 0, "later steps ride the same marker");

    let (entries, frame) = take(&main);
    let live = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    assert_eq!(frame, None);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: live,
                at: 0.0,
                rest: None,
                request: None,
            }
        )],
        "one entry at the latest offset, held by the finger"
    );

    touch(&mut painter, Move, 100.0);
    assert_eq!(markers(&mut main), 1, "the take re-armed the marker");
}

/// A drag let go without a fling comes to rest where it stands.
#[test]
fn a_drag_released_without_a_fling_posts_its_rest() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("");
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    let _ = take(&main);
    // Every step at one clock reading: no velocity to fling with.
    touch(&mut painter, Up, 120.0);
    let (entries, _) = take(&main);
    let offset = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].1.rest,
        Some(offset),
        "at rest where it was let go"
    );
}

/// A fling posts its steps without a rest, and its last one with it.
#[test]
fn a_spent_fling_posts_its_rest() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("");
    for (at, phase, y) in [
        (0.0, Down, 190.0),
        (0.016, Move, 170.0),
        (0.032, Move, 150.0),
        (0.048, Move, 130.0),
        (0.05, Up, 130.0),
    ] {
        painter.clock.pin(at);
        touch(&mut painter, phase, y);
    }
    let (entries, _) = take(&main);
    assert!(
        entries.iter().all(|(_, entry)| entry.rest.is_none()),
        "the release flings"
    );
    let mut moving = 0;
    for frame in 1..600_u16 {
        let now = 0.05 + f64::from(frame) / 60.0;
        painter.service_gesture_clock(now);
        painter.publish_scroll(now, None);
        let (entries, _) = take(&main);
        let Some((_, entry)) = entries.into_iter().find(|(node, _)| *node == scroller) else {
            continue;
        };
        assert_eq!(
            entry.at.to_bits(),
            now.to_bits(),
            "stamped with the pass's clock"
        );
        if let Some(rest) = entry.rest {
            assert!(moving > 0, "the fling moved before it rested");
            assert_eq!(Some(rest), painter.scroll_intents.offset_for(scroller));
            return;
        }
        moving += 1;
    }
    panic!("the fling never came to rest");
}

/// A `contain-bounce` stretch is the painter's alone: what it posts is the
/// edge the stretch stands past.
#[test]
fn a_stretched_offset_posts_clamped() {
    use dom::input::PointerPhase::{Down, Move};

    let (mut painter, main, scroller) = scrolling("overscroll-behavior:contain-bounce");
    touch(&mut painter, Down, 50.0);
    touch(&mut painter, Move, 150.0);
    let live = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the drag stretched the scroller");
    assert!(live.y < 0.0, "stretched past the start, got {live:?}");
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.offset, dom::Vector2D::zero());
}

/// An `overscroll-behavior: circular` offset is posted modulo the period
/// (1000 here: `max_offset` 800 plus the 200px scrollport) and unclamped,
/// since the document admits the whole period on such an axis. Wheeled to
/// 800, a drag of 108px (100 after the 8px slop) puts the live offset at
/// 900 — the scrollport straddles the seam, and main is told 900, past
/// `max_offset`. A second drag of 158px (150 after the slop) puts it at
/// 1050, past the seam, and main is told 50. A bounded axis still posts
/// clamped (`a_stretched_offset_posts_clamped`).
#[test]
fn a_wrapped_offset_posts_through_the_seam_unclamped() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("overscroll-behavior:circular");
    painter.dispatch_input(dom::input::InputEvent::wheel(
        dom::Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 800.0),
    ));
    let _ = take(&main);

    touch(&mut painter, Down, 190.0);
    touch(&mut painter, Move, 82.0);
    let live = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    assert!((live.y - 900.0).abs() < 0.01, "got {live:?}");
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.offset,
        dom::Vector2D::new(0.0, 900.0),
        "across the seam, as the screen shows it"
    );
    // Let go at one clock reading, so without velocity: at rest where it
    // stands.
    touch(&mut painter, Up, 82.0);
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.rest, Some(dom::Vector2D::new(0.0, 900.0)));

    touch(&mut painter, Down, 190.0);
    touch(&mut painter, Move, 32.0);
    let live = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    assert!(
        (live.y.rem_euclid(1000.0) - 50.0).abs() < 0.01,
        "got {live:?}"
    );
    let (entries, _) = take(&main);
    let posted = entries[0].1.offset;
    assert!(
        posted.x == 0.0 && (posted.y - 50.0).abs() < 0.01,
        "past the seam main sees the start again, got {posted:?}"
    );
}

/// A smooth request on a circular axis glides along the shorter arc of the
/// circle (period 1000: `max_offset` 800 plus the 200px scrollport), across
/// the seam when that is shorter. Standing at 900, a request to 50 runs
/// forward through 1000, 150px, rather than back 850px, and rests at 50;
/// standing at 50, a request to 900 runs back through 0, 150px, and rests
/// at 900. Each glide moves one way only, so the live offset never leaves
/// the arc between its start and its target.
#[test]
fn a_smooth_request_on_a_circular_axis_glides_along_the_shorter_arc() {
    for (from, to, arc) in [
        (900.0_f32, 50.0_f32, 900.0..=1050.0),
        (50.0, 900.0, -100.0..=50.0),
    ] {
        let label = format!("{from} to {to}");
        let mut document = document();
        let root = document.document_element().id();
        let scroller = document.create_element("view", ());
        document.set_inline_style(
            scroller,
            "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px;\
             overscroll-behavior:circular",
        );
        document.append_child(root, scroller);
        let filler = document.create_element("view", ());
        document.set_inline_style(filler, "flex-shrink:0;width:200px;height:1000px");
        document.append_child(scroller, filler);
        let (mut painter, main) = detached();
        main.outbox.publish_frame(document.commit());
        painter.poll_link();
        painter.clock.pin(0.0);

        // Stand at `from`: an instant request, which the painter carries out
        // when it adopts the frame.
        document.scroll_to_with(
            scroller,
            dom::Vector2D::new(0.0, from),
            dom::scroll::ScrollBehavior::Instant,
        );
        main.outbox.publish_frame(document.commit());
        painter.poll_link();
        let frame = painter.frame().cloned().expect("the new frame is adopted");
        painter.scroll_intents.rebase(&frame, 0.0);
        let slot = &frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
        assert_eq!(slot.wrap_period(), (None, Some(1000.0)), "{label}");
        let standing = painter
            .scroll_intents
            .offset_for(scroller)
            .unwrap_or(slot.offset);
        assert_eq!(standing, dom::Vector2D::new(0.0, from), "{label}");

        document.scroll_to_with(
            scroller,
            dom::Vector2D::new(0.0, to),
            dom::scroll::ScrollBehavior::Smooth,
        );
        main.outbox.publish_frame(document.commit());
        painter.poll_link();
        let frame = painter.frame().cloned().expect("the new frame is adopted");
        painter.scroll_intents.rebase(&frame, 0.0);
        assert!(painter.scroll_intents.is_gliding(scroller), "{label}");

        let mut rested = None;
        let mut travelled = false;
        for step in 1..240_u16 {
            let now = f64::from(step) / 60.0;
            painter.scroll_intents.tick(&frame, now);
            let live = painter
                .scroll_intents
                .offset_for(scroller)
                .expect("the glide moved the scroller")
                .y;
            assert!(
                arc.contains(&live),
                "{label}: {live} left the shorter arc {arc:?}"
            );
            travelled |= !(0.0..=1000.0).contains(&live);
            painter.publish_scroll(now, None);
            let (entries, _) = take(&main);
            if let Some(rest) = entries
                .into_iter()
                .find(|(node, _)| *node == scroller)
                .and_then(|(_, entry)| entry.rest)
            {
                rested = Some(rest);
                break;
            }
        }
        assert!(travelled, "{label}: the glide crossed the seam");
        assert_eq!(
            rested,
            Some(dom::Vector2D::new(0.0, to)),
            "{label}: at rest"
        );
    }
}

/// A rebase brings a circular axis's live offset back into its period and
/// moves the drag's origin by the same whole period, so the release still
/// settles from where the drag found the container. Five 200px cards that
/// each snap `mandatory` with `scroll-snap-stop: always`, in a 200px
/// scroller: `max_offset` 800, period 1000. A drag back from 0 by 142px
/// (150 less the 8px slop) stands at −142; a commit landing under the
/// finger normalizes it to 858 and the origin from 0 to 1000. Released,
/// the nearest position is the last card, 800, and no stop lies between
/// 1000 and 858 — an origin left at 0 would instead find the stop at 200
/// between 0 and 858, and glide the wrong way round.
#[test]
fn a_rebase_wraps_a_held_offset_and_its_drag_origin_together() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let mut document = document();
    let root = document.document_element().id();
    let scroller = document.create_element("view", ());
    document.set_inline_style(
        scroller,
        "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px;\
         overscroll-behavior:circular;scroll-snap-type:y mandatory",
    );
    document.append_child(root, scroller);
    for _ in 0..5 {
        let card = document.create_element("view", ());
        document.set_inline_style(
            card,
            "flex-shrink:0;width:200px;height:200px;scroll-snap-align:start;\
             scroll-snap-stop:always",
        );
        document.append_child(scroller, card);
    }
    let (mut painter, main) = detached();
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    painter.clock.pin(0.0);

    touch(&mut painter, Down, 30.0);
    touch(&mut painter, Move, 180.0);
    let live = painter.scroll_intents.offset_for(scroller);
    assert_eq!(live, Some(dom::Vector2D::new(0.0, -142.0)));

    document.set_inline_style(root, "background-color: teal");
    let frame = document.commit();
    main.outbox.publish_frame(Arc::clone(&frame));
    painter.poll_link();
    painter.scroll_intents.rebase(&frame, 0.0);
    assert_eq!(
        painter.scroll_intents.offset_for(scroller),
        Some(dom::Vector2D::new(0.0, 858.0)),
        "normalized into the period"
    );
    let origins: Vec<_> = painter
        .scroll_intents
        .gesture_origins
        .iter()
        .filter(|((_, node), _)| *node == scroller)
        .map(|(_, origin)| *origin)
        .collect();
    assert_eq!(
        origins,
        [dom::Vector2D::new(0.0, 1000.0)],
        "the origin moved with it"
    );

    touch(&mut painter, Up, 180.0);
    let mut now = 0.0;
    while painter.scroll_intents.is_animating() {
        now += 1.0 / 60.0;
        assert!(now < 3.0, "the glide never landed");
        painter.service_gesture_clock(now);
    }
    let rest = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the release left the scroller off its committed offset");
    assert!(
        (rest.y.rem_euclid(1000.0) - 800.0).abs() < f32::EPSILON,
        "on the last card, got {rest:?}"
    );
}

/// Frame posts coalesce to the latest clock and the greatest sequence, and
/// every post answers its own sequence number whether or not it sent the
/// marker.
#[test]
fn frame_posts_coalesce_to_the_latest_clock_and_greatest_sequence() {
    let (mut painter, mut main) = detached();
    assert_eq!(painter.begin_frame(1.0, true), Some(1));
    assert_eq!(painter.begin_frame(2.0, true), Some(2));
    assert_eq!(markers(&mut main), 1, "one marker for both");
    assert_eq!(
        take(&main).1,
        Some(FramePost {
            now: 2.0,
            seq: 2,
            fence: 1
        }),
        "fenced behind the first post's marker"
    );
    assert_eq!(
        main.scroll.clock().to_bits(),
        2.0_f64.to_bits(),
        "the clock rode along"
    );
    assert_eq!(painter.begin_frame(3.0, true), Some(3));
    assert_eq!(markers(&mut main), 1, "the take re-armed the marker");
}

/// A tap decided in the same pass as a scroll step reaches main behind the
/// marker, so its listener never sees a document older than its pass.
#[test]
fn a_dispatch_in_a_scrolling_pass_follows_the_marker() {
    use super::gesture::{EmitEvent, InputDecision, InputDecisions, TAP_EVENT, TouchPoints};

    let (mut painter, mut main, scroller) = scrolling("");
    main.outbox.listener_edge(Arc::from(TAP_EVENT), true);
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: scroller,
        delta: dom::Vector2D::new(0.0, 30.0),
    });
    decisions.push(InputDecision::Emit(EmitEvent {
        name: TAP_EVENT,
        target: scroller,
        position: dom::Point2D::new(1.0, 1.0),
        wheel: None,
        touches: TouchPoints::new(),
    }));
    painter.execute_decisions(&mut decisions, Some(&frame), 0.5);
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Posted)));
    assert!(matches!(
        main.commands.try_recv(),
        Ok(ToMain::DispatchEvent { .. })
    ));
    assert_eq!(take(&main).0[0].1.offset, dom::Vector2D::new(0.0, 30.0));
}

/// The animation frame callbacks run after the scroll state of their pass:
/// HTML's scroll steps before `requestAnimationFrame`. In `post_frame`'s order —
/// an offset moves, then the vsync is delivered — the vsync's own send
/// publishes the move ahead of it.
#[test]
fn a_vsync_follows_the_marker_of_its_pass() {
    use super::gesture::{InputDecision, InputDecisions};

    let (mut painter, mut main, scroller) = scrolling("");
    main.seat.frame_demand.borrow_mut().set(None, true);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: scroller,
        delta: dom::Vector2D::new(0.0, 30.0),
    });
    painter.execute_decisions(&mut decisions, Some(&frame), 0.5);
    assert!(main.commands.try_recv().is_err(), "nothing sent yet");
    painter.deliver_vsync(0.5);
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Posted)));
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Vsync(_))));
    assert_eq!(take(&main).0[0].1.offset, dom::Vector2D::new(0.0, 30.0));
}

/// A frame post that rides a marker already queued is fenced behind the
/// commands sent after that marker: main applies it only once it has
/// applied them, so the acknowledgement implies the tap below.
#[test]
fn a_frame_post_is_fenced_behind_every_command_sent_before_it() {
    use super::gesture::{EmitEvent, InputDecision, InputDecisions, TAP_EVENT, TouchPoints};

    let (mut painter, mut main, scroller) = scrolling("");
    main.outbox.listener_edge(Arc::from(TAP_EVENT), true);
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: scroller,
        delta: dom::Vector2D::new(0.0, 30.0),
    });
    decisions.push(InputDecision::Emit(EmitEvent {
        name: TAP_EVENT,
        target: scroller,
        position: dom::Point2D::new(1.0, 1.0),
        wheel: None,
        touches: TouchPoints::new(),
    }));
    painter.execute_decisions(&mut decisions, Some(&frame), 0.5);
    let sent = main.seat.commands.sent();
    assert_eq!(painter.begin_frame(0.5, true), Some(1));
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Posted)));
    assert!(matches!(
        main.commands.try_recv(),
        Ok(ToMain::DispatchEvent { .. })
    ));
    assert!(
        main.commands.try_recv().is_err(),
        "the post rides the queued marker"
    );
    assert_eq!(
        take(&main).1.map(|post| post.fence),
        Some(sent),
        "behind the marker and the tap"
    );
}

/// Posts that land before one take merge to the latest clock and the
/// greatest sequence and fence, whatever order they arrive in: a painter
/// reattached behind an untaken post restarts below its sequence.
#[test]
fn a_frame_post_merge_keeps_the_greatest_sequence() {
    let mailbox = ScrollMailbox::default();
    assert!(mailbox.post(
        [],
        Some(FramePost {
            now: 2.0,
            seq: 5,
            fence: 7
        })
    ));
    assert!(!mailbox.post(
        [],
        Some(FramePost {
            now: 3.0,
            seq: 4,
            fence: 6
        })
    ));
    assert_eq!(
        mailbox.take(&mut rustc_hash::FxHashMap::default()),
        Some(FramePost {
            now: 3.0,
            seq: 5,
            fence: 7
        })
    );
}

/// An untaken `rest` survives the posts that follow it: a drag let go and
/// touched again before main takes still reports where it came to rest.
#[test]
fn an_untaken_rest_survives_a_later_post() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("");
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    touch(&mut painter, Up, 120.0);
    let released = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    touch(&mut painter, Down, 120.0);
    touch(&mut painter, Move, 100.0);
    let live = painter
        .scroll_intents
        .offset_for(scroller)
        .expect("the second drag moved the scroller");
    assert_ne!(live, released);
    let (entries, _) = take(&main);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1.offset, live, "the latest offset");
    assert_eq!(entries[0].1.rest, Some(released), "and the untaken rest");
}

/// A commit that grows the content under a bounce back lands the stretched
/// offset in range, which ends the bounce back without a step: the offset
/// the document held clamped is posted again, at rest.
#[test]
fn a_bounce_back_a_commit_ends_posts_its_rest() {
    let mut document = document();
    let root = document.document_element().id();
    let scroller = document.create_element("view", ());
    document.set_inline_style(
        scroller,
        "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px;\
         overscroll-behavior:contain-bounce",
    );
    document.append_child(root, scroller);
    let filler = document.create_element("view", ());
    document.set_inline_style(filler, "flex-shrink:0;width:200px;height:1000px");
    document.append_child(scroller, filler);
    let (mut painter, main) = detached();
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");

    // Stretched 40px past the end, and let go: the bounce back starts.
    let stretched = dom::Vector2D::new(0.0, 840.0);
    painter.scroll_intents.write(scroller, stretched);
    painter.scroll_intents.start_bounce_backs(&frame, 0.0);
    assert!(painter.scroll_intents.is_bouncing(scroller));
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.offset, dom::Vector2D::new(0.0, 800.0));
    assert_eq!(entries[0].1.rest, None, "still bouncing");

    document.set_inline_style(filler, "flex-shrink:0;width:200px;height:1200px");
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let grown = painter.frame().cloned().expect("the new frame is adopted");
    painter.scroll_intents.rebase(&grown, 0.016);
    assert!(!painter.scroll_intents.is_bouncing(scroller));
    painter.publish_scroll(0.016, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: stretched,
                at: 0.016,
                rest: Some(stretched),
                request: None,
            }
        )],
        "posted in range, at rest"
    );
}

/// A smooth request's glide posts each step naming the request, and its
/// landing at rest; a post made before the painter saw the request names
/// none, which is what lets main drop it.
#[test]
fn a_smooth_request_posts_its_serial_and_lands_at_rest() {
    let mut document = document();
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
    let (mut painter, main) = detached();
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    painter.clock.pin(0.0);

    // A wheel before the request: its post names no request.
    painter.dispatch_input(dom::input::InputEvent::wheel(
        dom::Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, 30.0),
    ));
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.request, None);

    document.scroll_to_with(
        scroller,
        dom::Vector2D::new(0.0, 300.0),
        dom::scroll::ScrollBehavior::Smooth,
    );
    let serial = document
        .pending_scroll_request(scroller)
        .expect("the request is pending");
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("the new frame is adopted");
    painter.scroll_intents.rebase(&frame, 0.0);
    assert!(painter.scroll_intents.is_gliding(scroller));
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: dom::Vector2D::new(0.0, 30.0),
                at: 0.0,
                rest: None,
                request: Some(serial),
            }
        )],
        "posted at once, from where the wheel left it, naming the request"
    );

    for step in 1..120_u16 {
        let now = f64::from(step) / 60.0;
        painter.scroll_intents.tick(&frame, now);
        painter.publish_scroll(now, None);
        let (entries, _) = take(&main);
        let Some((_, entry)) = entries.into_iter().find(|(node, _)| *node == scroller) else {
            continue;
        };
        assert_eq!(entry.request, Some(serial));
        if let Some(rest) = entry.rest {
            assert_eq!(rest, dom::Vector2D::new(0.0, 300.0), "landed exactly");
            assert!(!painter.scroll_intents.is_animating());
            return;
        }
        assert!(entry.offset.y > 30.0 && entry.offset.y < 300.0);
    }
    panic!("the glide never came to rest");
}

/// A 200px column scroller of five pages in a document of its own, each
/// page as tall as the scrollport and snapping at its start, the scroller
/// carrying `css`: positions 0, 200, 400, 600 and 800, and one scrollport
/// apart at any other height.
fn paged(css: &str) -> (LynxDocument, dom::NodeId) {
    let mut document = document();
    let root = document.document_element().id();
    let scroller = document.create_element("view", ());
    document.set_inline_style(scroller, &paged_style(css, 200));
    document.append_child(root, scroller);
    for _ in 0..5 {
        let page = document.create_element("view", ());
        document.set_inline_style(
            page,
            "flex-shrink:0;width:200px;height:100%;scroll-snap-align:start",
        );
        document.append_child(scroller, page);
    }
    (document, scroller)
}

/// [`paged`]'s scroller style at `height` px.
fn paged_style(css: &str, height: u32) -> String {
    format!(
        "display:flex;flex-direction:column;overflow:scroll;width:200px;height:{height}px;\
         scroll-snap-type:y mandatory;{css}"
    )
}

/// Publishes the document's newest commit and brings the painter onto it,
/// as a display frame does.
fn adopt(painter: &mut Painter, main: &FarEnd, document: &mut LynxDocument) {
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("the frame is adopted");
    painter.scroll_intents.rebase(&frame, 0.0);
}

/// Where the painter shows `node`: its live offset, else the committed one.
fn shown(painter: &Painter, node: dom::NodeId) -> dom::Vector2D<f32> {
    painter.scroll_intents.offset_for(node).unwrap_or_else(|| {
        let frame = painter.frame().expect("a frame is adopted");
        frame.scroll_slots()[frame.slot_of(node).expect("a scroll container") as usize].offset
    })
}

fn wheel(painter: &mut Painter, dy: f32) {
    painter.dispatch_input(dom::input::InputEvent::wheel(
        dom::Point2D::new(100.0, 100.0),
        dom::Vector2D::new(0.0, dy),
    ));
}

/// css-scroll-snap-1 §4.1.3 on the painter's own offset. Two wheel ticks
/// put the scroller on its third page, 400, which main never adopts here:
/// the document still stands at 0, so nothing in the commit says where the
/// painter is. At 300px the third page is at 600, and the painter moves
/// there at once — the position nearest 400 would have been the second
/// page's 300 — and posts it at rest. Two more commits adopted with no
/// rebase between them are followed from the frame the offset stands on:
/// the third page at 250px is 500.
#[test]
fn a_resized_scroller_stays_on_the_page_the_painter_showed() {
    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    painter.clock.pin(0.0);
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    wheel(&mut painter, 30.0);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 400.0));
    let _ = take(&main);

    document.set_inline_style(scroller, &paged_style("", 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 600.0));
    assert!(
        !painter.scroll_intents.is_animating(),
        "a jump, not a glide"
    );
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: dom::Vector2D::new(0.0, 600.0),
                at: 0.0,
                rest: Some(dom::Vector2D::new(0.0, 600.0)),
                request: None,
            }
        )],
    );

    document.set_inline_style(scroller, &paged_style("", 1000));
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    document.set_inline_style(scroller, &paged_style("", 250));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 500.0));
}

/// The same when main had adopted the painter's offset: the document
/// re-snaps its own in the commit, and the painter, following its own
/// reading of the frame it stood on, arrives at the offset the slot
/// carries.
#[test]
fn a_resized_scroller_agrees_with_the_document_that_followed_it() {
    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    painter.clock.pin(0.0);
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    wheel(&mut painter, 30.0);
    let (entries, _) = take(&main);
    document.scroll_to(scroller, entries[0].1.offset);

    document.set_inline_style(scroller, &paged_style("", 300));
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let slot = &frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
    assert_eq!(slot.offset, dom::Vector2D::new(0.0, 600.0));
    assert_eq!(shown(&painter, scroller), slot.offset);
}

/// A circular axis: six ticks from the first of five pages stand on the
/// second page's next copy, 1200 on a period of 1000. At 300px the period
/// is 1500 and the second page is at 300; the offset left where it was
/// would read 1200 there, the fifth page.
#[test]
fn a_resized_circular_scroller_stays_on_the_page_the_painter_showed() {
    let circular = "overscroll-behavior:circular";
    let (mut document, scroller) = paged(circular);
    let (mut painter, main) = detached();
    painter.clock.pin(0.0);
    adopt(&mut painter, &main, &mut document);
    for _ in 0..6 {
        wheel(&mut painter, 30.0);
    }
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 1200.0));

    document.set_inline_style(scroller, &paged_style(circular, 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 300.0));
}

/// `proximity`, where the two rules differ. A container at rest on the
/// third page's position, 400, follows the page to 600 at 300px, though
/// 600 is two proximity ranges from where it stood: the range decides
/// whether a container snaps, not whether a snapped one stays snapped. One
/// at rest on no position — 750, more than a range from 600 and from 900 —
/// follows nothing, and at 320px, more than a range from 640 and from 960,
/// it keeps its offset, where `mandatory` would have taken it to 640.
#[test]
fn a_proximity_scroller_follows_its_area_only_when_it_stood_on_one() {
    let proximity = "scroll-snap-type:y proximity";
    let (mut document, scroller) = paged(proximity);
    let (mut painter, main) = detached();
    painter.clock.pin(0.0);
    adopt(&mut painter, &main, &mut document);
    painter
        .scroll_intents
        .write(scroller, dom::Vector2D::new(0.0, 400.0));
    document.set_inline_style(scroller, &paged_style(proximity, 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 600.0));

    let between = dom::Vector2D::new(0.0, 750.0);
    painter.scroll_intents.write(scroller, between);
    document.set_inline_style(scroller, &paged_style(proximity, 320));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), between);
}

/// A commit moves no container a drag holds or a glide is moving, whatever
/// it stands on. The drag brings the scroller back onto the third page's
/// position, 400, and holds it there: the resize, which would carry a
/// scroller at rest to 600, leaves it under the finger. Let go, it glides
/// from there toward a position of the new frame, and a second resize
/// landing mid-glide leaves the glide its offset too. It comes to rest on a
/// position of the last frame.
#[test]
fn a_commit_moves_no_scroller_a_drag_or_a_glide_has() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    painter.clock.pin(0.0);
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    wheel(&mut painter, 30.0);

    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 422.0));
    touch(&mut painter, Move, 142.0);
    let held = dom::Vector2D::new(0.0, 400.0);
    assert_eq!(shown(&painter, scroller), held);
    document.set_inline_style(scroller, &paged_style("", 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), held, "under the finger");

    painter.clock.pin(1.0);
    touch(&mut painter, Up, 142.0);
    assert!(painter.scroll_intents.is_gliding(scroller));
    painter.service_gesture_clock(1.05);
    let gliding = shown(&painter, scroller);
    assert!(gliding.y > 300.0 && gliding.y < 400.0, "got {gliding:?}");
    document.set_inline_style(scroller, &paged_style("", 250));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), gliding, "under the glide");
    assert!(painter.scroll_intents.is_gliding(scroller));

    let mut now = 1.05;
    while painter.scroll_intents.is_animating() {
        now += 1.0 / 60.0;
        assert!(now < 4.0, "the glide never landed");
        painter.service_gesture_clock(now);
    }
    let rest = shown(&painter, scroller).y;
    assert!(
        rest % 250.0 == 0.0,
        "on a page of the last frame, got {rest}"
    );
}

/// A document holding one 200px column scroller of `content` px at the
/// origin, with the scroller and its filler: a range of `content - 200`.
fn auto_scrolled(content: u32) -> (LynxDocument, dom::NodeId, dom::NodeId) {
    let mut document = document();
    let root = document.document_element().id();
    let scroller = document.create_element("view", ());
    document.set_inline_style(
        scroller,
        "display:flex;flex-direction:column;overflow:scroll;width:200px;height:200px",
    );
    document.append_child(root, scroller);
    let filler = document.create_element("view", ());
    document.set_inline_style(filler, &filler_style(content));
    document.append_child(scroller, filler);
    (document, scroller, filler)
}

fn filler_style(content: u32) -> String {
    format!("flex-shrink:0;width:200px;height:{content}px")
}

/// Ticks the scroll intents over the painter's adopted frame `count` times,
/// `hz` to the second, from `from`, and returns the clock it ended on.
fn tick_at(painter: &mut Painter, from: f64, hz: f64, count: u32) -> f64 {
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut now = from;
    for _ in 0..count {
        now += 1.0 / hz;
        painter.scroll_intents.tick(&frame, now);
    }
    now
}

/// An auto-scroll moves its axis at its rate on the painter's clock, posts
/// each step naming the change that started it, and ends on the edge it
/// was to stop at, posted at rest; a container with none owes no frame.
#[test]
fn an_auto_scroll_moves_at_its_rate_and_ends_on_its_edge() {
    let (mut document, scroller, _) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    assert!(!painter.scroll_intents.is_auto_scrolling(&frame));

    document.auto_scroll(scroller, 600.0, false, true);
    let serial = document
        .pending_auto_scroll(scroller)
        .expect("the change is pending")
        .serial;
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("the new frame is adopted");
    assert!(painter.scroll_intents.has_auto_scroll(scroller));
    assert!(painter.scroll_intents.is_auto_scrolling(&frame));
    assert!(
        !painter.scroll_intents.is_animating(),
        "no fling, bounce back or glide"
    );
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: dom::Vector2D::zero(),
                at: 0.0,
                rest: None,
                request: Some(serial),
            }
        )],
        "posted at once, unmoved, naming the change"
    );

    // The first tick is where the travel counts from; thirty more at 60 Hz
    // are half a second, 300px.
    let now = tick_at(&mut painter, 0.0, 60.0, 31);
    let offset = shown(&painter, scroller).y;
    assert!((offset - 300.0).abs() < 0.01, "got {offset}");
    painter.publish_scroll(now, None);
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.request, Some(serial));
    assert_eq!(entries[0].1.rest, None, "still moving");

    let now = tick_at(&mut painter, now, 60.0, 60);
    assert_eq!(
        shown(&painter, scroller),
        dom::Vector2D::new(0.0, 800.0),
        "on the edge exactly"
    );
    assert!(!painter.scroll_intents.has_auto_scroll(scroller), "ended");
    assert!(!painter.scroll_intents.is_auto_scrolling(&frame));
    painter.publish_scroll(now, None);
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.rest, Some(dom::Vector2D::new(0.0, 800.0)));

    // Back the other way, to the start edge.
    document.auto_scroll(scroller, -1200.0, false, true);
    adopt(&mut painter, &main, &mut document);
    tick_at(&mut painter, now, 60.0, 60);
    assert_eq!(shown(&painter, scroller), dom::Vector2D::zero());
    assert!(!painter.scroll_intents.has_auto_scroll(scroller));
}

/// A write is at least [`AUTO_SCROLL_MIN_INTERVAL_SECONDS`] after the last
/// and at least one physical pixel: a 120 Hz display writes every second
/// frame, a slow rate once per pixel, and neither loses any travel.
///
/// [`AUTO_SCROLL_MIN_INTERVAL_SECONDS`]: super::inertia::AUTO_SCROLL_MIN_INTERVAL_SECONDS
#[test]
fn an_auto_scroll_writes_no_more_often_than_its_interval_and_a_pixel() {
    let writes = |rate: f32, hz: f64, ticks: u32| {
        let (mut document, scroller, _) = auto_scrolled(1000);
        let (mut painter, main) = detached();
        adopt(&mut painter, &main, &mut document);
        document.auto_scroll(scroller, rate, false, true);
        adopt(&mut painter, &main, &mut document);
        let frame = painter.frame().cloned().expect("a frame is adopted");
        let (mut written, mut last, mut now) = (0_u32, 0.0_f32, 0.0_f64);
        for _ in 0..ticks {
            now += 1.0 / hz;
            painter.scroll_intents.tick(&frame, now);
            let offset = shown(&painter, scroller).y;
            if offset > last {
                assert!(offset - last >= 1.0, "a step of {}", offset - last);
                written += 1;
                last = offset;
            }
            assert!(
                painter.scroll_intents.is_auto_scrolling(&frame),
                "a tick that writes nothing still owes the next frame"
            );
        }
        (written, last)
    };

    // One second at 120 Hz: the first tick starts the count, and every
    // second one after it writes 10px.
    let (written, offset) = writes(600.0, 120.0, 121);
    assert_eq!(written, 60);
    assert!((offset - 600.0).abs() < 0.01, "got {offset}");

    // One second at 60 Hz: every tick writes.
    let (written, offset) = writes(600.0, 60.0, 61);
    assert_eq!(written, 60);
    assert!((offset - 600.0).abs() < 0.01, "got {offset}");

    // Two seconds at 6px a second: one write per pixel, not per frame.
    let (written, offset) = writes(6.0, 60.0, 121);
    assert!((11..=12).contains(&written), "got {written}");
    assert!((offset - 12.0).abs() < 1.01, "got {offset}");
}

/// Without `stop_at_edge` an auto-scroll stays at its edge, at rest and
/// owing no frame, and goes on when a commit brings a longer range, from
/// that commit and not from the time it waited. The instant scroll made in
/// the same turn arrives with it, and the auto-scroll starts from there.
#[test]
fn an_auto_scroll_that_does_not_stop_waits_at_its_edge_for_a_longer_range() {
    let (mut document, scroller, filler) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    // A wheel first, so the painter holds an offset of its own that the
    // document's instant scroll has to replace.
    wheel(&mut painter, 30.0);
    let _ = take(&main);

    document.auto_scroll(scroller, 600.0, false, false);
    document.scroll_to_with(
        scroller,
        dom::Vector2D::new(0.0, 700.0),
        dom::scroll::ScrollBehavior::Instant,
    );
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        dom::Vector2D::new(0.0, 700.0),
        "the scroll request of the same turn was carried out"
    );
    assert!(
        painter.scroll_intents.has_auto_scroll(scroller),
        "and so was the older auto-scroll change"
    );

    let now = tick_at(&mut painter, 0.0, 60.0, 30);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 800.0));
    assert!(painter.scroll_intents.has_auto_scroll(scroller), "kept");
    assert!(
        !painter.scroll_intents.is_auto_scrolling(&frame),
        "at its edge it owes no frame"
    );
    painter.publish_scroll(now, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.rest,
        Some(dom::Vector2D::new(0.0, 800.0)),
        "at rest on the edge"
    );

    // Ten seconds later the content grows by 200px.
    document.set_inline_style(filler, &filler_style(1200));
    adopt(&mut painter, &main, &mut document);
    let grown = painter.frame().cloned().expect("the new frame is adopted");
    assert!(painter.scroll_intents.is_auto_scrolling(&grown));
    let now = tick_at(&mut painter, now + 10.0, 60.0, 1);
    assert_eq!(
        shown(&painter, scroller),
        dom::Vector2D::new(0.0, 800.0),
        "the wait is not travelled"
    );
    tick_at(&mut painter, now, 60.0, 6);
    let offset = shown(&painter, scroller).y;
    assert!((offset - 860.0).abs() < 0.01, "got {offset}");
}

/// A drag has the container while it holds it: the auto-scroll adds nothing
/// and owes no frame, and after the release it goes on from where the
/// finger left the container, the hold not travelled.
#[test]
fn an_auto_scroll_yields_to_a_drag_and_goes_on_after_it() {
    use dom::input::PointerPhase::{Down, Move, Up};

    let (mut document, scroller, _) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    document.auto_scroll(scroller, 600.0, false, true);
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let now = tick_at(&mut painter, 0.0, 60.0, 31);
    assert!((shown(&painter, scroller).y - 300.0).abs() < 0.01);

    painter.clock.pin(now);
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    let held = shown(&painter, scroller);
    assert!(held.y > 310.0, "the finger moved it, got {held:?}");
    assert!(painter.scroll_intents.is_held(scroller));
    assert!(
        !painter.scroll_intents.is_auto_scrolling(&frame),
        "a held container owes no frame"
    );
    let now = tick_at(&mut painter, now, 60.0, 120);
    assert_eq!(shown(&painter, scroller), held, "under the finger");

    // Every step at one clock reading: no velocity to fling with.
    painter.clock.pin(now);
    touch(&mut painter, Up, 120.0);
    assert!(!painter.scroll_intents.is_held(scroller));
    assert!(
        painter.scroll_intents.has_auto_scroll(scroller),
        "not ended"
    );
    assert!(painter.scroll_intents.is_auto_scrolling(&frame));
    let now = tick_at(&mut painter, now, 60.0, 1);
    assert_eq!(shown(&painter, scroller), held, "the hold is not travelled");
    tick_at(&mut painter, now, 60.0, 6);
    let offset = shown(&painter, scroller).y;
    assert!((offset - held.y - 60.0).abs() < 0.01, "got {offset}");
}

/// A scroll request moves an auto-scrolling container and does not end its
/// auto-scroll: a smooth one glides first and the auto-scroll goes on from
/// its landing. Only a change with no rate ends it, where it stands.
#[test]
fn a_scroll_request_does_not_end_an_auto_scroll_and_a_stop_does() {
    let (mut document, scroller, _) = auto_scrolled(2000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    document.auto_scroll(scroller, 60.0, false, true);
    adopt(&mut painter, &main, &mut document);
    let now = tick_at(&mut painter, 0.0, 60.0, 61);
    assert!((shown(&painter, scroller).y - 60.0).abs() < 0.01);

    document.scroll_to_with(
        scroller,
        dom::Vector2D::new(0.0, 500.0),
        dom::scroll::ScrollBehavior::Smooth,
    );
    // Adopted at the clock the ticks run on, so the glide starts there.
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");
    painter.scroll_intents.rebase(&frame, now);
    assert!(painter.scroll_intents.is_gliding(scroller));
    assert!(painter.scroll_intents.has_auto_scroll(scroller));
    assert!(
        !painter.scroll_intents.is_auto_scrolling(&frame),
        "the glide has the container"
    );
    let mut now = now;
    while painter.scroll_intents.is_gliding(scroller) {
        now = tick_at(&mut painter, now, 60.0, 1);
        assert!(
            shown(&painter, scroller).y <= 500.0,
            "never past the target"
        );
    }
    assert_eq!(shown(&painter, scroller), dom::Vector2D::new(0.0, 500.0));
    assert!(painter.scroll_intents.is_auto_scrolling(&frame));
    let now = tick_at(&mut painter, now, 60.0, 60);
    let offset = shown(&painter, scroller).y;
    assert!((offset - 560.0).abs() < 1.01, "got {offset}");

    document.auto_scroll(scroller, 0.0, false, true);
    let stop = document
        .pending_auto_scroll(scroller)
        .expect("the stop is pending")
        .serial;
    adopt(&mut painter, &main, &mut document);
    assert!(!painter.scroll_intents.has_auto_scroll(scroller));
    let stopped = shown(&painter, scroller);
    painter.publish_scroll(now, None);
    let (entries, _) = take(&main);
    let (_, entry) = entries
        .into_iter()
        .find(|(node, _)| *node == scroller)
        .expect("the stop is posted");
    assert_eq!(entry.request, Some(stop), "naming the stop");
    assert_eq!(entry.rest, Some(stopped), "at rest where it stood");
    tick_at(&mut painter, now, 60.0, 30);
    assert_eq!(shown(&painter, scroller), stopped);
}

/// An auto-scrolling container is no container at rest: a commit does not
/// snap it, and the at-rest rule applies once the auto-scroll has ended.
#[test]
fn a_commit_does_not_snap_an_auto_scrolling_container() {
    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    document.auto_scroll(scroller, 600.0, false, true);
    adopt(&mut painter, &main, &mut document);
    tick_at(&mut painter, 0.0, 60.0, 8);
    let moving = shown(&painter, scroller);
    assert!((moving.y - 70.0).abs() < 0.01, "got {moving:?}");

    document.set_inline_style(scroller, &paged_style("opacity:0.5", 200));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), moving, "between two pages, kept");

    document.auto_scroll(scroller, 0.0, false, true);
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        dom::Vector2D::new(0.0, 0.0),
        "stopped, it settles on the nearest page"
    );
}

/// A request that yields to a pointer is not carried out while a drag holds
/// its container, and is posted as handled all the same; once the finger is
/// off, the next one is carried out like any other request.
#[test]
fn a_request_that_yields_to_a_pointer_is_dropped_under_a_finger() {
    use dom::input::PointerPhase::{Down, Move, Up};
    use dom::scroll::ScrollBehavior::{Instant, Smooth};

    let (mut document, scroller, _) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    painter.clock.pin(0.0);
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    let held = shown(&painter, scroller);
    assert!(held.y > 0.0 && painter.scroll_intents.is_held(scroller));
    let _ = take(&main);

    for behavior in [Smooth, Instant] {
        assert!(document.scroll_to_yielding(scroller, dom::Vector2D::new(0.0, 400.0), behavior));
        let serial = document
            .pending_scroll_request(scroller)
            .expect("the request is pending");
        adopt(&mut painter, &main, &mut document);
        assert_eq!(
            shown(&painter, scroller),
            held,
            "{behavior:?}: under the finger"
        );
        assert!(!painter.scroll_intents.is_gliding(scroller), "{behavior:?}");
        assert!(
            painter.scroll_intents.is_held(scroller),
            "{behavior:?}: still held"
        );
        painter.publish_scroll(0.0, None);
        let (entries, _) = take(&main);
        assert_eq!(
            entries[0].1.request,
            Some(serial),
            "{behavior:?}: posted as handled"
        );
        assert_eq!(
            entries[0].1.offset, held,
            "{behavior:?}: where the finger has it"
        );
    }

    // Every step at one clock reading: no velocity to fling with.
    touch(&mut painter, Up, 120.0);
    assert!(!painter.scroll_intents.is_held(scroller));
    assert!(document.scroll_to_yielding(scroller, dom::Vector2D::new(0.0, 400.0), Instant));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        dom::Vector2D::new(0.0, 400.0),
        "free, it is carried out"
    );
    assert!(document.scroll_to_yielding(scroller, dom::Vector2D::new(0.0, 600.0), Smooth));
    adopt(&mut painter, &main, &mut document);
    assert!(painter.scroll_intents.is_gliding(scroller), "and glides");
}
