use std::sync::Arc;
use std::time::{Duration, Instant};

use terminus::link::event::EventDetail;
use terminus::link::painter::ScrollIntentsTestUtils as _;

use super::{FarEnd, Painter, ToMain};
use crate::link::{FramePost, ScrollEntry, ScrollMailbox};
use crate::main::tree::{
    Autoplays, ComponentEvents, ImageSources, LynxDocument, PageConfig, Videos, Viewport,
    new_document,
};
use crate::resource::SourceRequest;
use crate::test_support::TestViewSpec;
use crate::view::{EngineEvent, EventRequester, FrameSize, NoWakeup};

fn document() -> LynxDocument {
    new_document(
        Viewport::new(393.0, 727.0),
        PageConfig::default(),
        ComponentEvents::default(),
        Autoplays::default(),
        ImageSources::default(),
        Videos::default(),
    )
}

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
        crate::background::WorkerMessage::Vsync { .. }
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

#[test]
fn one_poll_adopts_every_kind_of_published_state() {
    let (mut painter, main) = detached();
    assert!(!painter.take_redraw(), "a fresh painter owes no frame");

    main.outbox.set_name_routed(Arc::from("tap"), true);
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

fn tap_detail() -> EventDetail {
    EventDetail::Pointer {
        position: terminus::link::Point2D::new(1.0, 1.0),
        id: 1,
        device: terminus::link::input::PointerKind::Touch,
        button: 0,
        buttons: 0,
    }
}

#[test]
fn an_emit_decision_crosses_only_when_a_listener_wants_it() {
    use terminus::link::painter::{EmitEvent, InputDecision, InputDecisions};

    use super::TAP_EVENT;

    let (mut painter, mut main) = detached();
    let target = terminus::link::NodeId::from_bits(2).expect("a well-formed packed handle");
    let emit = |painter: &mut Painter| {
        let mut decisions = InputDecisions::new();
        decisions.push(InputDecision::Emit(EmitEvent {
            name: TAP_EVENT,
            target,
            detail: tap_detail(),
            release: None,
        }));
        painter.execute_decisions(&mut decisions, None, 0.25);
        assert!(decisions.is_empty(), "the queue is always drained");
    };

    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "an empty listener set sends nothing"
    );

    main.outbox.set_name_routed(Arc::from("pointerup"), true);
    painter.poll_link();
    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "a listener on another name sends nothing"
    );

    main.outbox.set_name_routed(Arc::from(TAP_EVENT), true);
    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "an unadopted registration does not route the name mid-pass"
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
        ..
    } = command
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(name, TAP_EVENT);
    assert_eq!(sent, target);
    assert_eq!(payload.detail, tap_detail());
    assert!((payload.timestamp - 250.0).abs() < f64::EPSILON);

    main.outbox.set_name_routed(Arc::from(TAP_EVENT), false);
    painter.poll_link();
    emit(&mut painter);
    assert!(
        main.commands.try_recv().is_err(),
        "a name no longer routed stops the crossing"
    );
}

#[test]
fn a_wheel_delta_and_touch_points_cross_as_values() {
    use terminus::link::event::{
        TOUCH_ACTIVE, TOUCH_CHANGED, TOUCH_TARGET, TouchPoint, TouchPoints,
    };
    use terminus::link::painter::{EmitEvent, InputDecision, InputDecisions};

    let (mut painter, mut main) = detached();
    let target = terminus::link::NodeId::from_bits(2).expect("a well-formed packed handle");
    main.outbox.set_name_routed(Arc::from("wheel"), true);
    main.outbox.set_name_routed(Arc::from("touchmove"), true);
    painter.poll_link();

    let mut touches = TouchPoints::new();
    touches.push(TouchPoint {
        identifier: 1,
        position: terminus::link::Point2D::new(10.0, 20.0),
        flags: TOUCH_ACTIVE,
    });
    touches.push(TouchPoint {
        identifier: 2,
        position: terminus::link::Point2D::new(30.5, 40.0),
        flags: TOUCH_ACTIVE | TOUCH_TARGET | TOUCH_CHANGED,
    });
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Emit(EmitEvent {
        name: "wheel",
        target,
        detail: EventDetail::Wheel {
            position: terminus::link::Point2D::new(5.0, 6.0),
            delta: terminus::link::Vector2D::new(0.0, 30.0),
        },
        release: None,
    }));
    decisions.push(InputDecision::Emit(EmitEvent {
        name: "touchmove",
        target,
        detail: EventDetail::Touch {
            position: terminus::link::Point2D::new(30.5, 40.0),
            points: touches.clone(),
        },
        release: None,
    }));
    painter.execute_decisions(&mut decisions, None, 0.5);

    let ToMain::DispatchEvent { payload, .. } = main
        .commands
        .try_recv()
        .expect("the wheel event crosses first")
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(
        payload.detail,
        EventDetail::Wheel {
            position: terminus::link::Point2D::new(5.0, 6.0),
            delta: terminus::link::Vector2D::new(0.0, 30.0),
        }
    );

    let ToMain::DispatchEvent { payload, .. } =
        main.commands.try_recv().expect("then the touch event")
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(payload.detail.touch_points(), touches.as_slice());
    assert_eq!(
        payload.detail.position(),
        Some(terminus::link::Point2D::new(30.5, 40.0))
    );
    assert!((payload.timestamp - 500.0).abs() < f64::EPSILON);
}

#[test]
fn a_poll_adopts_arrived_route_changes_in_order_and_does_not_block() {
    let (mut painter, main) = detached();

    painter.poll_link();
    assert!(
        !painter.published.listeners.contains("tap"),
        "nothing has been published yet"
    );

    main.outbox.set_name_routed(Arc::from("tap"), true);
    main.outbox.set_name_routed(Arc::from("scroll"), true);
    main.outbox.set_name_routed(Arc::from("tap"), false);
    painter.poll_link();
    assert!(
        !painter.published.listeners.contains("tap"),
        "the name unrouted after it was routed is not in the set"
    );
    assert!(
        painter.published.listeners.contains("scroll"),
        "the other name stays routed"
    );

    drop(main);
    painter.poll_link();
    assert!(
        painter.published.listeners.contains("scroll"),
        "a closed channel leaves the snapshot standing"
    );
}

#[test]
fn a_scroll_decision_sends_no_command() {
    use terminus::link::painter::{InputDecision, InputDecisions};

    let (mut painter, mut main) = detached();
    let node = terminus::link::NodeId::from_bits(2).expect("a well-formed packed handle");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: node,
        delta: terminus::link::Vector2D::new(0.0, 5.0),
    });
    painter.execute_decisions(&mut decisions, None, 0.0);
    painter.publish_scroll(0.0, None);
    assert!(
        main.commands.try_recv().is_err(),
        "nothing moved, so nothing crosses"
    );
    assert!(!painter.scroll_intents().has_live_offsets());
}

struct WakeSignal(flume::Sender<()>);

impl EventRequester for WakeSignal {
    fn request_event(&self) {
        let _ = self.0.send(());
    }
}

#[test]
fn a_painter_that_presents_to_no_window_owes_no_frame() {
    let (painter, _main) = detached();
    painter.refresh();
    assert!(
        painter.redraw_pending(),
        "the refresh did leave a frame owed"
    );
    assert!(
        !painter.owes_frame(),
        "but nothing owes the display a frame it cannot present"
    );
}

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
    let size = engine.painter.frame_size();
    engine.tick(true).expect("the committed frame renders");
    assert!(engine.capture().is_ok(), "and reads back");

    engine.painter.set_frame_size(FrameSize {
        width: 0,
        height: 0,
    });
    let error = engine
        .tick(true)
        .expect_err("a zero-sized target is refused");
    assert!(matches!(error, EngineError::Gpu(_)), "{error}");

    engine.painter.detach();
    engine.painter.set_frame_size(size);
    let error = engine
        .capture()
        .expect_err("the failed render left nothing to capture");
    assert!(
        matches!(error, EngineError::Render(message) if message.contains("no frame has been rendered")),
        "the refused render cleared the record, so the capture is refused before any readback"
    );
}

fn two_painters() -> [(Painter, FarEnd); 2] {
    [detached(), detached()]
}

#[test]
fn a_views_published_state_is_independent_of_a_siblings() {
    let [(mut first, first_end), (mut second, second_end)] = two_painters();

    second_end.outbox.set_name_routed(Arc::from("tap"), true);
    let _asked = second_end
        .outbox
        .request_source(SourceRequest::Module("app:///second.js".into()));
    first_end.outbox.publish_frame(document().commit());

    first.poll_link();
    assert!(first.published_frame().is_some());
    assert!(!first.published.listeners.contains("tap"));

    drop(first_end);
    drop(second_end);
    second.poll_link();
    assert!(second.published.listeners.contains("tap"));
    assert!(
        second.published_frame().is_none(),
        "and no frame reached it, because none was published to it"
    );
}

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

fn scrolling_frame(css: &str) -> (Arc<terminus::link::CommittedFrame>, terminus::link::NodeId) {
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

fn scrolling(css: &str) -> (Painter, FarEnd, terminus::link::NodeId) {
    let (mut painter, main) = detached();
    let (frame, scroller) = scrolling_frame(css);
    main.outbox.publish_frame(frame);
    painter.poll_link();
    painter.clock_mut().pin(0.0);
    (painter, main, scroller)
}

fn touch(painter: &mut Painter, phase: terminus::link::input::PointerPhase, y: f32) {
    painter.dispatch_input(terminus::link::input::InputEvent::pointer(
        terminus::link::Point2D::new(100.0, y),
        1,
        terminus::link::input::PointerKind::Touch,
        phase,
    ));
}

fn markers(main: &mut FarEnd) -> usize {
    std::iter::from_fn(|| main.commands.try_recv().ok())
        .map(|command| assert!(matches!(command, ToMain::Posted), "only markers"))
        .count()
}

fn take(
    main: &FarEnd,
) -> (
    Vec<(terminus::link::NodeId, ScrollEntry)>,
    Option<FramePost>,
) {
    let mut spare = rustc_hash::FxHashMap::default();
    let frame = main.scroll.take(&mut spare);
    (spare.into_iter().collect(), frame)
}

#[test]
fn a_drag_queues_one_marker_until_main_takes() {
    use terminus::link::input::PointerPhase::{Down, Move};

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
        .scroll_intents()
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
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::zero(),
            }
        )],
        "one entry at the latest offset, held by the finger"
    );

    touch(&mut painter, Move, 100.0);
    assert_eq!(markers(&mut main), 1, "the take re-armed the marker");
}

#[test]
fn a_drag_released_without_a_fling_posts_its_rest() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("");
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    let _ = take(&main);
    touch(&mut painter, Up, 120.0);
    let (entries, _) = take(&main);
    let offset = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].1.rest,
        Some(offset),
        "at rest where it was let go"
    );
}

#[test]
fn a_spent_fling_posts_its_rest() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("");
    for (at, phase, y) in [
        (0.0, Down, 190.0),
        (0.016, Move, 170.0),
        (0.032, Move, 150.0),
        (0.048, Move, 130.0),
        (0.05, Up, 130.0),
    ] {
        painter.clock_mut().pin(at);
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
            assert_eq!(Some(rest), painter.scroll_intents().offset_for(scroller));
            return;
        }
        moving += 1;
    }
    panic!("the fling never came to rest");
}

#[test]
#[allow(
    clippy::float_cmp,
    reason = "a fling into the end rests exactly on it, and x never moves"
)]
fn a_fling_posts_its_remaining_travel_as_its_reach() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    for (step, reaches_the_end) in [(20.0, false), (40.0, true)] {
        let (mut painter, main, scroller) = scrolling("");
        for (at, phase, y) in [
            (0.0, Down, 190.0),
            (0.016, Move, 190.0 - step),
            (0.032, Move, 190.0 - 2.0 * step),
            (0.048, Move, 190.0 - 3.0 * step),
            (0.05, Up, 190.0 - 3.0 * step),
        ] {
            painter.clock_mut().pin(at);
            touch(&mut painter, phase, y);
        }
        let _ = take(&main);
        let mut posts = Vec::new();
        let mut rest = None;
        for frame in 1..600_u16 {
            let now = 0.05 + f64::from(frame) / 60.0;
            painter.service_gesture_clock(now);
            painter.publish_scroll(now, None);
            let (entries, _) = take(&main);
            let Some((_, entry)) = entries.into_iter().find(|(node, _)| *node == scroller) else {
                continue;
            };
            if let Some(at) = entry.rest {
                assert_eq!(
                    entry.reach,
                    terminus::link::Vector2D::zero(),
                    "a post at rest reaches nowhere"
                );
                rest = Some(at);
                break;
            }
            posts.push(entry);
        }
        let rest = rest.expect("the fling came to rest");
        assert!(posts.len() > 2, "the fling moved over several frames");
        assert_eq!(
            rest.y == 800.0,
            reaches_the_end,
            "step {step}: landed at {rest:?}"
        );
        for entry in &posts {
            assert_eq!(entry.reach.x, 0.0, "nothing moves on x");
            assert!(
                entry.reach.y > 0.0 || entry.offset.y == 800.0,
                "a forward fling reaches ahead"
            );
            let landing = entry.offset.y + entry.reach.y;
            assert!(
                landing <= 800.0,
                "clamped to the range: {} + {}",
                entry.offset.y,
                entry.reach.y
            );
            assert!(
                (landing - rest.y).abs() < 1.5,
                "step {step}: {} + {} lands at {}, the fling at {}",
                entry.offset.y,
                entry.reach.y,
                landing,
                rest.y
            );
        }
    }
}

#[test]
fn a_wheel_step_posts_no_reach() {
    let (mut painter, main, scroller) = scrolling("");
    wheel(&mut painter, 30.0);
    let (entries, _) = take(&main);
    let (_, entry) = entries
        .into_iter()
        .find(|(node, _)| *node == scroller)
        .expect("the wheel moved the scroller");
    assert_eq!(entry.offset, terminus::link::Vector2D::new(0.0, 30.0));
    assert_eq!(entry.reach, terminus::link::Vector2D::zero());
}

#[test]
fn a_stretched_offset_posts_clamped() {
    use terminus::link::input::PointerPhase::{Down, Move};

    let (mut painter, main, scroller) = scrolling("overscroll-behavior:contain-bounce");
    touch(&mut painter, Down, 50.0);
    touch(&mut painter, Move, 150.0);
    let live = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the drag stretched the scroller");
    assert!(live.y < 0.0, "stretched past the start, got {live:?}");
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.offset, terminus::link::Vector2D::zero());
}

#[test]
fn a_wrapped_offset_posts_through_the_seam_unclamped() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("overscroll-behavior:circular");
    painter.dispatch_input(terminus::link::input::InputEvent::wheel(
        terminus::link::Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 800.0),
    ));
    let _ = take(&main);

    touch(&mut painter, Down, 190.0);
    touch(&mut painter, Move, 82.0);
    let live = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    assert!((live.y - 900.0).abs() < 0.01, "got {live:?}");
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.offset,
        terminus::link::Vector2D::new(0.0, 900.0),
        "across the seam, as the screen shows it"
    );
    touch(&mut painter, Up, 82.0);
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.rest,
        Some(terminus::link::Vector2D::new(0.0, 900.0))
    );

    touch(&mut painter, Down, 190.0);
    touch(&mut painter, Move, 32.0);
    let live = painter
        .scroll_intents()
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
        painter.clock_mut().pin(0.0);

        document.scroll_to_with(
            scroller,
            terminus::link::Vector2D::new(0.0, from),
            terminus::link::scroll::ScrollBehavior::Instant,
        );
        main.outbox.publish_frame(document.commit());
        painter.poll_link();
        let frame = painter.frame().cloned().expect("the new frame is adopted");
        painter.scroll_intents_mut().rebase(&frame, 0.0);
        let slot = &frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
        assert_eq!(slot.wrap_period(), (None, Some(1000.0)), "{label}");
        let standing = painter
            .scroll_intents()
            .offset_for(scroller)
            .unwrap_or(slot.offset);
        assert_eq!(
            standing,
            terminus::link::Vector2D::new(0.0, from),
            "{label}"
        );

        document.scroll_to_with(
            scroller,
            terminus::link::Vector2D::new(0.0, to),
            terminus::link::scroll::ScrollBehavior::Smooth,
        );
        main.outbox.publish_frame(document.commit());
        painter.poll_link();
        let frame = painter.frame().cloned().expect("the new frame is adopted");
        painter.scroll_intents_mut().rebase(&frame, 0.0);
        assert!(painter.scroll_intents().is_gliding(scroller), "{label}");

        let mut rested = None;
        let mut travelled = false;
        for step in 1..240_u16 {
            let now = f64::from(step) / 60.0;
            painter.scroll_intents_mut().tick(&frame, now);
            let live = painter
                .scroll_intents()
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
            Some(terminus::link::Vector2D::new(0.0, to)),
            "{label}: at rest"
        );
    }
}

#[test]
fn a_rebase_wraps_a_held_offset_and_its_drag_origin_together() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

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
    painter.clock_mut().pin(0.0);

    touch(&mut painter, Down, 30.0);
    touch(&mut painter, Move, 180.0);
    let live = painter.scroll_intents().offset_for(scroller);
    assert_eq!(live, Some(terminus::link::Vector2D::new(0.0, -142.0)));

    document.set_inline_style(root, "background-color: teal");
    let frame = document.commit();
    main.outbox.publish_frame(Arc::clone(&frame));
    painter.poll_link();
    painter.scroll_intents_mut().rebase(&frame, 0.0);
    assert_eq!(
        painter.scroll_intents().offset_for(scroller),
        Some(terminus::link::Vector2D::new(0.0, 858.0)),
        "normalized into the period"
    );
    let origins: Vec<_> = painter.scroll_intents().drag_origins(scroller).collect();
    assert_eq!(
        origins,
        [terminus::link::Vector2D::new(0.0, 1000.0)],
        "the origin moved with it"
    );

    touch(&mut painter, Up, 180.0);
    let mut now = 0.0;
    while painter.scroll_intents().is_animating() {
        now += 1.0 / 60.0;
        assert!(now < 3.0, "the glide never landed");
        painter.service_gesture_clock(now);
    }
    let rest = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the release left the scroller off its committed offset");
    assert!(
        (rest.y.rem_euclid(1000.0) - 800.0).abs() < f32::EPSILON,
        "on the last card, got {rest:?}"
    );
}

#[test]
fn an_anchoring_shift_moves_the_held_offset_and_its_drag_origin() {
    use terminus::link::input::PointerPhase::{Down, Move};

    let mut document = document();
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
    let (mut painter, main) = detached();
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    painter.clock_mut().pin(0.0);

    touch(&mut painter, Down, 190.0);
    touch(&mut painter, Move, 40.0);
    let held = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    let origin: Vec<_> = painter.scroll_intents().drag_origins(scroller).collect();

    document.scroll_to(scroller, held);
    document.set_inline_style(rows[0], "flex-shrink:0;width:200px;height:150px");
    let frame = document.commit();
    let shift = terminus::link::Vector2D::new(0.0, 50.0);
    assert_eq!(document.scroll_anchor_shift(scroller), shift);
    main.outbox.publish_frame(Arc::clone(&frame));
    painter.poll_link();
    painter.scroll_intents_mut().rebase(&frame, 0.0);
    let slot = &frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
    assert_eq!(slot.offset, held + shift);
    assert_eq!(
        painter
            .scroll_intents()
            .offset_for(scroller)
            .unwrap_or(slot.offset),
        held + shift,
        "the held offset moved with the shift"
    );
    let moved: Vec<_> = painter.scroll_intents().drag_origins(scroller).collect();
    assert_eq!(
        moved,
        origin
            .iter()
            .map(|origin| *origin + shift)
            .collect::<Vec<_>>(),
        "and the drag origin with it"
    );

    let _ = take(&main);
    touch(&mut painter, Move, 30.0);
    let (entries, _) = take(&main);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].1.anchor_shift, shift,
        "a post after the rebase names the shift it applied"
    );
}

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

#[test]
fn a_dispatch_in_a_scrolling_pass_follows_the_marker() {
    use terminus::link::painter::{EmitEvent, InputDecision, InputDecisions};

    use super::TAP_EVENT;

    let (mut painter, mut main, scroller) = scrolling("");
    main.outbox.set_name_routed(Arc::from(TAP_EVENT), true);
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: scroller,
        delta: terminus::link::Vector2D::new(0.0, 30.0),
    });
    decisions.push(InputDecision::Emit(EmitEvent {
        name: TAP_EVENT,
        target: scroller,
        detail: tap_detail(),
        release: None,
    }));
    painter.execute_decisions(&mut decisions, Some(&frame), 0.5);
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Posted)));
    assert!(matches!(
        main.commands.try_recv(),
        Ok(ToMain::DispatchEvent { .. })
    ));
    assert_eq!(
        take(&main).0[0].1.offset,
        terminus::link::Vector2D::new(0.0, 30.0)
    );
}

#[test]
fn a_vsync_follows_the_marker_of_its_pass() {
    use terminus::link::painter::{InputDecision, InputDecisions};

    let (mut painter, mut main, scroller) = scrolling("");
    main.seat.frame_demand.borrow_mut().set(None, true);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: scroller,
        delta: terminus::link::Vector2D::new(0.0, 30.0),
    });
    painter.execute_decisions(&mut decisions, Some(&frame), 0.5);
    assert!(main.commands.try_recv().is_err(), "nothing sent yet");
    painter.deliver_vsync(0.5);
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Posted)));
    assert!(matches!(main.commands.try_recv(), Ok(ToMain::Vsync(_))));
    assert_eq!(
        take(&main).0[0].1.offset,
        terminus::link::Vector2D::new(0.0, 30.0)
    );
}

#[test]
fn a_frame_post_is_fenced_behind_every_command_sent_before_it() {
    use terminus::link::painter::{EmitEvent, InputDecision, InputDecisions};

    use super::TAP_EVENT;

    let (mut painter, mut main, scroller) = scrolling("");
    main.outbox.set_name_routed(Arc::from(TAP_EVENT), true);
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: scroller,
        delta: terminus::link::Vector2D::new(0.0, 30.0),
    });
    decisions.push(InputDecision::Emit(EmitEvent {
        name: TAP_EVENT,
        target: scroller,
        detail: tap_detail(),
        release: None,
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

#[test]
fn an_untaken_rest_survives_a_later_post() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    let (mut painter, main, scroller) = scrolling("");
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    touch(&mut painter, Up, 120.0);
    let released = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the drag moved the scroller");
    touch(&mut painter, Down, 120.0);
    touch(&mut painter, Move, 100.0);
    let live = painter
        .scroll_intents()
        .offset_for(scroller)
        .expect("the second drag moved the scroller");
    assert_ne!(live, released);
    let (entries, _) = take(&main);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1.offset, live, "the latest offset");
    assert_eq!(entries[0].1.rest, Some(released), "and the untaken rest");
}

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

    let stretched = terminus::link::Vector2D::new(0.0, 840.0);
    painter.scroll_intents_mut().write(scroller, stretched);
    painter.scroll_intents_mut().start_bounce_backs(&frame, 0.0);
    assert!(painter.scroll_intents().is_bouncing(scroller));
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.offset,
        terminus::link::Vector2D::new(0.0, 800.0)
    );
    assert_eq!(entries[0].1.rest, None, "still bouncing");

    document.set_inline_style(filler, "flex-shrink:0;width:200px;height:1200px");
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let grown = painter.frame().cloned().expect("the new frame is adopted");
    painter.scroll_intents_mut().rebase(&grown, 0.016);
    assert!(!painter.scroll_intents().is_bouncing(scroller));
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
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::zero(),
            }
        )],
        "posted in range, at rest"
    );
}

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
    painter.clock_mut().pin(0.0);

    painter.dispatch_input(terminus::link::input::InputEvent::wheel(
        terminus::link::Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, 30.0),
    ));
    let (entries, _) = take(&main);
    assert_eq!(entries[0].1.request, None);

    document.scroll_to_with(
        scroller,
        terminus::link::Vector2D::new(0.0, 300.0),
        terminus::link::scroll::ScrollBehavior::Smooth,
    );
    let serial = document
        .pending_scroll_request(scroller)
        .expect("the request is pending");
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("the new frame is adopted");
    painter.scroll_intents_mut().rebase(&frame, 0.0);
    assert!(painter.scroll_intents().is_gliding(scroller));
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: terminus::link::Vector2D::new(0.0, 30.0),
                at: 0.0,
                rest: None,
                request: Some(serial),
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::new(0.0, 270.0),
            }
        )],
        "posted at once, from where the wheel left it, naming the request, \
         reaching the glide's target"
    );

    for step in 1..120_u16 {
        let now = f64::from(step) / 60.0;
        painter.scroll_intents_mut().tick(&frame, now);
        painter.publish_scroll(now, None);
        let (entries, _) = take(&main);
        let Some((_, entry)) = entries.into_iter().find(|(node, _)| *node == scroller) else {
            continue;
        };
        assert_eq!(entry.request, Some(serial));
        if let Some(rest) = entry.rest {
            assert_eq!(
                rest,
                terminus::link::Vector2D::new(0.0, 300.0),
                "landed exactly"
            );
            assert!(!painter.scroll_intents().is_animating());
            assert_eq!(entry.reach, terminus::link::Vector2D::zero());
            return;
        }
        assert!(entry.offset.y > 30.0 && entry.offset.y < 300.0);
        assert!(
            (entry.offset.y + entry.reach.y - 300.0).abs() < 1.0e-3,
            "a glide reaches its target: {entry:?}"
        );
    }
    panic!("the glide never came to rest");
}

fn paged(css: &str) -> (LynxDocument, terminus::link::NodeId) {
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

fn paged_style(css: &str, height: u32) -> String {
    format!(
        "display:flex;flex-direction:column;overflow:scroll;width:200px;height:{height}px;\
         scroll-snap-type:y mandatory;{css}"
    )
}

fn adopt(painter: &mut Painter, main: &FarEnd, document: &mut LynxDocument) {
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("the frame is adopted");
    painter.scroll_intents_mut().rebase(&frame, 0.0);
}

fn shown(painter: &Painter, node: terminus::link::NodeId) -> terminus::link::Vector2D<f32> {
    painter
        .scroll_intents()
        .offset_for(node)
        .unwrap_or_else(|| {
            let frame = painter.frame().expect("a frame is adopted");
            frame.scroll_slots()[frame.slot_of(node).expect("a scroll container") as usize].offset
        })
}

fn wheel(painter: &mut Painter, dy: f32) {
    painter.dispatch_input(terminus::link::input::InputEvent::wheel(
        terminus::link::Point2D::new(100.0, 100.0),
        terminus::link::Vector2D::new(0.0, dy),
    ));
}

#[test]
fn a_resized_scroller_stays_on_the_page_the_painter_showed() {
    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    painter.clock_mut().pin(0.0);
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    wheel(&mut painter, 30.0);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 400.0)
    );
    let frame = painter.frame().cloned().expect("a frame is adopted");
    painter.scroll_intents_mut().tick(&frame, 1.0);
    let _ = take(&main);

    document.set_inline_style(scroller, &paged_style("", 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 600.0)
    );
    assert!(
        !painter.scroll_intents().is_gliding(scroller),
        "a jump, not a glide"
    );
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: terminus::link::Vector2D::new(0.0, 600.0),
                at: 0.0,
                rest: Some(terminus::link::Vector2D::new(0.0, 600.0)),
                request: None,
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::zero(),
            }
        )],
    );

    document.set_inline_style(scroller, &paged_style("", 1000));
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    document.set_inline_style(scroller, &paged_style("", 250));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 500.0)
    );
}

#[test]
fn a_resized_scroller_agrees_with_the_document_that_followed_it() {
    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    painter.clock_mut().pin(0.0);
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    wheel(&mut painter, 30.0);
    let (entries, _) = take(&main);
    document.scroll_to(scroller, entries[0].1.offset);

    document.set_inline_style(scroller, &paged_style("", 300));
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let slot = &frame.scroll_slots()[frame.slot_of(scroller).expect("a slot") as usize];
    assert_eq!(slot.offset, terminus::link::Vector2D::new(0.0, 600.0));
    assert_eq!(shown(&painter, scroller), slot.offset);
}

#[test]
fn a_resized_circular_scroller_stays_on_the_page_the_painter_showed() {
    let circular = "overscroll-behavior:circular";
    let (mut document, scroller) = paged(circular);
    let (mut painter, main) = detached();
    painter.clock_mut().pin(0.0);
    adopt(&mut painter, &main, &mut document);
    for _ in 0..6 {
        wheel(&mut painter, 30.0);
    }
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 1200.0)
    );

    document.set_inline_style(scroller, &paged_style(circular, 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 300.0)
    );
}

#[test]
fn a_proximity_scroller_follows_its_area_only_when_it_stood_on_one() {
    let proximity = "scroll-snap-type:y proximity";
    let (mut document, scroller) = paged(proximity);
    let (mut painter, main) = detached();
    painter.clock_mut().pin(0.0);
    adopt(&mut painter, &main, &mut document);
    painter
        .scroll_intents_mut()
        .write(scroller, terminus::link::Vector2D::new(0.0, 400.0));
    document.set_inline_style(scroller, &paged_style(proximity, 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 600.0)
    );

    let between = terminus::link::Vector2D::new(0.0, 750.0);
    painter.scroll_intents_mut().write(scroller, between);
    document.set_inline_style(scroller, &paged_style(proximity, 320));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), between);
}

#[test]
fn a_commit_moves_no_scroller_a_drag_or_a_glide_has() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    let (mut document, scroller) = paged("");
    let (mut painter, main) = detached();
    painter.clock_mut().pin(0.0);
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    wheel(&mut painter, 30.0);

    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 422.0)
    );
    touch(&mut painter, Move, 142.0);
    let held = terminus::link::Vector2D::new(0.0, 400.0);
    assert_eq!(shown(&painter, scroller), held);
    document.set_inline_style(scroller, &paged_style("", 300));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), held, "under the finger");

    painter.clock_mut().pin(1.0);
    touch(&mut painter, Up, 142.0);
    assert!(painter.scroll_intents().is_gliding(scroller));
    painter.service_gesture_clock(1.05);
    let gliding = shown(&painter, scroller);
    assert!(gliding.y > 300.0 && gliding.y < 400.0, "got {gliding:?}");
    document.set_inline_style(scroller, &paged_style("", 250));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(shown(&painter, scroller), gliding, "under the glide");
    assert!(painter.scroll_intents().is_gliding(scroller));

    let mut now = 1.05;
    while painter.scroll_intents().is_animating() {
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

fn auto_scrolled(content: u32) -> (LynxDocument, terminus::link::NodeId, terminus::link::NodeId) {
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

fn tick_at(painter: &mut Painter, from: f64, hz: f64, count: u32) -> f64 {
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let mut now = from;
    for _ in 0..count {
        now += 1.0 / hz;
        painter.scroll_intents_mut().tick(&frame, now);
    }
    now
}

#[test]
fn an_auto_scroll_moves_at_its_rate_and_ends_on_its_edge() {
    let (mut document, scroller, _) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    assert!(!painter.scroll_intents().is_auto_scrolling(&frame));

    document.auto_scroll(scroller, 600.0, false, true);
    let serial = document
        .pending_auto_scroll(scroller)
        .expect("the change is pending")
        .serial;
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("the new frame is adopted");
    assert!(painter.scroll_intents().has_auto_scroll(scroller));
    assert!(painter.scroll_intents().is_auto_scrolling(&frame));
    assert!(
        !painter.scroll_intents().is_animating(),
        "no fling, bounce back or glide"
    );
    painter.publish_scroll(0.0, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries,
        [(
            scroller,
            ScrollEntry {
                offset: terminus::link::Vector2D::zero(),
                at: 0.0,
                rest: None,
                request: Some(serial),
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::zero(),
            }
        )],
        "posted at once, unmoved, naming the change"
    );

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
        terminus::link::Vector2D::new(0.0, 800.0),
        "on the edge exactly"
    );
    assert!(!painter.scroll_intents().has_auto_scroll(scroller), "ended");
    assert!(!painter.scroll_intents().is_auto_scrolling(&frame));
    painter.publish_scroll(now, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.rest,
        Some(terminus::link::Vector2D::new(0.0, 800.0))
    );

    document.auto_scroll(scroller, -1200.0, false, true);
    adopt(&mut painter, &main, &mut document);
    tick_at(&mut painter, now, 60.0, 60);
    assert_eq!(shown(&painter, scroller), terminus::link::Vector2D::zero());
    assert!(!painter.scroll_intents().has_auto_scroll(scroller));
}

#[test]
fn an_auto_scroll_writes_no_more_often_than_its_interval_and_a_pixel() {
    use terminus::link::painter::AUTO_SCROLL_MIN_INTERVAL_SECONDS;

    const {
        assert!(1.0 / 120.0 < AUTO_SCROLL_MIN_INTERVAL_SECONDS);
        assert!(AUTO_SCROLL_MIN_INTERVAL_SECONDS <= 2.0 / 120.0);
        assert!(AUTO_SCROLL_MIN_INTERVAL_SECONDS <= 1.0 / 60.0);
    }

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
            painter.scroll_intents_mut().tick(&frame, now);
            let offset = shown(&painter, scroller).y;
            if offset > last {
                assert!(offset - last >= 1.0, "a step of {}", offset - last);
                written += 1;
                last = offset;
            }
            assert!(
                painter.scroll_intents().is_auto_scrolling(&frame),
                "a tick that writes nothing still owes the next frame"
            );
        }
        (written, last)
    };

    let (written, offset) = writes(600.0, 120.0, 121);
    assert_eq!(written, 60);
    assert!((offset - 600.0).abs() < 0.01, "got {offset}");

    let (written, offset) = writes(600.0, 60.0, 61);
    assert_eq!(written, 60);
    assert!((offset - 600.0).abs() < 0.01, "got {offset}");

    let (written, offset) = writes(6.0, 60.0, 121);
    assert!((11..=12).contains(&written), "got {written}");
    assert!((offset - 12.0).abs() < 1.01, "got {offset}");
}

#[test]
fn an_auto_scroll_that_does_not_stop_waits_at_its_edge_for_a_longer_range() {
    let (mut document, scroller, filler) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    wheel(&mut painter, 30.0);
    let _ = take(&main);

    document.auto_scroll(scroller, 600.0, false, false);
    document.scroll_to_with(
        scroller,
        terminus::link::Vector2D::new(0.0, 700.0),
        terminus::link::scroll::ScrollBehavior::Instant,
    );
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 700.0),
        "the scroll request of the same turn was carried out"
    );
    assert!(
        painter.scroll_intents().has_auto_scroll(scroller),
        "and so was the older auto-scroll change"
    );

    let now = tick_at(&mut painter, 0.0, 60.0, 30);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 800.0)
    );
    assert!(painter.scroll_intents().has_auto_scroll(scroller), "kept");
    assert!(
        !painter.scroll_intents().is_auto_scrolling(&frame),
        "at its edge it owes no frame"
    );
    painter.publish_scroll(now, None);
    let (entries, _) = take(&main);
    assert_eq!(
        entries[0].1.rest,
        Some(terminus::link::Vector2D::new(0.0, 800.0)),
        "at rest on the edge"
    );

    document.set_inline_style(filler, &filler_style(1200));
    adopt(&mut painter, &main, &mut document);
    let grown = painter.frame().cloned().expect("the new frame is adopted");
    assert!(painter.scroll_intents().is_auto_scrolling(&grown));
    let now = tick_at(&mut painter, now + 10.0, 60.0, 1);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 800.0),
        "the wait is not travelled"
    );
    tick_at(&mut painter, now, 60.0, 6);
    let offset = shown(&painter, scroller).y;
    assert!((offset - 860.0).abs() < 0.01, "got {offset}");
}

#[test]
fn an_auto_scroll_yields_to_a_drag_and_goes_on_after_it() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};

    let (mut document, scroller, _) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    document.auto_scroll(scroller, 600.0, false, true);
    adopt(&mut painter, &main, &mut document);
    let frame = painter.frame().cloned().expect("a frame is adopted");
    let now = tick_at(&mut painter, 0.0, 60.0, 31);
    assert!((shown(&painter, scroller).y - 300.0).abs() < 0.01);

    painter.clock_mut().pin(now);
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    let held = shown(&painter, scroller);
    assert!(held.y > 310.0, "the finger moved it, got {held:?}");
    assert!(painter.scroll_intents().is_held(scroller));
    assert!(
        !painter.scroll_intents().is_auto_scrolling(&frame),
        "a held container owes no frame"
    );
    let now = tick_at(&mut painter, now, 60.0, 120);
    assert_eq!(shown(&painter, scroller), held, "under the finger");

    painter.clock_mut().pin(now);
    touch(&mut painter, Up, 120.0);
    assert!(!painter.scroll_intents().is_held(scroller));
    assert!(
        painter.scroll_intents().has_auto_scroll(scroller),
        "not ended"
    );
    assert!(painter.scroll_intents().is_auto_scrolling(&frame));
    let now = tick_at(&mut painter, now, 60.0, 1);
    assert_eq!(shown(&painter, scroller), held, "the hold is not travelled");
    tick_at(&mut painter, now, 60.0, 6);
    let offset = shown(&painter, scroller).y;
    assert!((offset - held.y - 60.0).abs() < 0.01, "got {offset}");
}

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
        terminus::link::Vector2D::new(0.0, 500.0),
        terminus::link::scroll::ScrollBehavior::Smooth,
    );
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("a frame is adopted");
    painter.scroll_intents_mut().rebase(&frame, now);
    assert!(painter.scroll_intents().is_gliding(scroller));
    assert!(painter.scroll_intents().has_auto_scroll(scroller));
    assert!(
        !painter.scroll_intents().is_auto_scrolling(&frame),
        "the glide has the container"
    );
    let mut now = now;
    while painter.scroll_intents().is_gliding(scroller) {
        now = tick_at(&mut painter, now, 60.0, 1);
        assert!(
            shown(&painter, scroller).y <= 500.0,
            "never past the target"
        );
    }
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 500.0)
    );
    assert!(painter.scroll_intents().is_auto_scrolling(&frame));
    let now = tick_at(&mut painter, now, 60.0, 60);
    let offset = shown(&painter, scroller).y;
    assert!((offset - 560.0).abs() < 1.01, "got {offset}");

    document.auto_scroll(scroller, 0.0, false, true);
    let stop = document
        .pending_auto_scroll(scroller)
        .expect("the stop is pending")
        .serial;
    adopt(&mut painter, &main, &mut document);
    assert!(!painter.scroll_intents().has_auto_scroll(scroller));
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
        terminus::link::Vector2D::new(0.0, 0.0),
        "stopped, it settles on the nearest page"
    );
}

#[test]
fn a_request_that_yields_to_a_pointer_is_dropped_under_a_finger() {
    use terminus::link::input::PointerPhase::{Down, Move, Up};
    use terminus::link::scroll::ScrollBehavior::{Instant, Smooth};

    let (mut document, scroller, _) = auto_scrolled(1000);
    let (mut painter, main) = detached();
    adopt(&mut painter, &main, &mut document);
    painter.clock_mut().pin(0.0);
    touch(&mut painter, Down, 150.0);
    touch(&mut painter, Move, 120.0);
    let held = shown(&painter, scroller);
    assert!(held.y > 0.0 && painter.scroll_intents().is_held(scroller));
    let _ = take(&main);

    for behavior in [Smooth, Instant] {
        assert!(document.scroll_to_yielding(
            scroller,
            terminus::link::Vector2D::new(0.0, 400.0),
            behavior
        ));
        let serial = document
            .pending_scroll_request(scroller)
            .expect("the request is pending");
        adopt(&mut painter, &main, &mut document);
        assert_eq!(
            shown(&painter, scroller),
            held,
            "{behavior:?}: under the finger"
        );
        assert!(
            !painter.scroll_intents().is_gliding(scroller),
            "{behavior:?}"
        );
        assert!(
            painter.scroll_intents().is_held(scroller),
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

    touch(&mut painter, Up, 120.0);
    assert!(!painter.scroll_intents().is_held(scroller));
    assert!(document.scroll_to_yielding(
        scroller,
        terminus::link::Vector2D::new(0.0, 400.0),
        Instant
    ));
    adopt(&mut painter, &main, &mut document);
    assert_eq!(
        shown(&painter, scroller),
        terminus::link::Vector2D::new(0.0, 400.0),
        "free, it is carried out"
    );
    assert!(document.scroll_to_yielding(
        scroller,
        terminus::link::Vector2D::new(0.0, 600.0),
        Smooth
    ));
    adopt(&mut painter, &main, &mut document);
    assert!(painter.scroll_intents().is_gliding(scroller), "and glides");
}

fn adopt_at(painter: &mut Painter, main: &FarEnd, document: &mut LynxDocument, now: f64) {
    main.outbox.publish_frame(document.commit());
    painter.poll_link();
    let frame = painter.frame().cloned().expect("the frame is adopted");
    painter.scroll_intents_mut().rebase(&frame, now);
}

#[test]
fn a_repeated_smooth_request_to_where_the_container_glides_keeps_the_glide() {
    use terminus::link::scroll::ScrollBehavior::Smooth;

    const HZ: f64 = 60.0;
    const FRAMES: u32 = 6;
    let run = |repeat: bool| {
        let (mut document, scroller, _) = auto_scrolled(1000);
        let (mut painter, main) = detached();
        adopt(&mut painter, &main, &mut document);
        assert!(document.scroll_to_yielding(
            scroller,
            terminus::link::Vector2D::new(0.0, 400.0),
            Smooth
        ));
        adopt(&mut painter, &main, &mut document);
        let now = tick_at(&mut painter, 0.0, HZ, FRAMES);
        let midway = shown(&painter, scroller);
        if repeat {
            assert!(document.scroll_to_yielding(
                scroller,
                terminus::link::Vector2D::new(0.0, 400.0),
                Smooth
            ));
            let serial = document
                .pending_scroll_request(scroller)
                .expect("the request is pending");
            adopt_at(&mut painter, &main, &mut document, now);
            assert_eq!(
                shown(&painter, scroller),
                midway,
                "nothing moved by the request"
            );
            painter.publish_scroll(now, None);
            let (entries, _) = take(&main);
            assert_eq!(entries[0].1.request, Some(serial), "posted as handled");
        }
        assert!(painter.scroll_intents().is_gliding(scroller));
        tick_at(&mut painter, now, HZ, FRAMES);
        (midway, shown(&painter, scroller))
    };
    let (midway, once) = run(false);
    assert!(
        midway.y > 0.0 && once.y > midway.y && once.y < 400.0,
        "{midway:?} {once:?}"
    );
    assert_eq!(run(true), (midway, once));
}

#[test]
fn a_wheel_rests_only_after_a_quiet_interval() {
    use terminus::link::painter::WHEEL_REST_SECONDS;

    let (mut painter, main, scroller) = scrolling("");
    let wheel = |painter: &mut Painter, at: f64| {
        painter.clock_mut().pin(at);
        painter.dispatch_input(terminus::link::input::InputEvent::wheel(
            terminus::link::Point2D::new(100.0, 100.0),
            terminus::link::Vector2D::new(0.0, 20.0),
        ));
    };
    let rest_of = |main: &FarEnd| {
        let (entries, _) = take(main);
        entries
            .iter()
            .find(|(node, _)| *node == scroller)
            .map(|(_, entry)| entry.rest)
    };
    let tick = |painter: &mut Painter, at: f64| {
        let frame = painter.frame().cloned().expect("a frame is adopted");
        painter.scroll_intents_mut().tick(&frame, at);
        painter.publish_scroll(at, None);
    };

    wheel(&mut painter, 0.0);
    assert_eq!(rest_of(&main), Some(None), "a wheel step posts it moving");
    assert!(painter.is_animating(), "the quiet interval owes frames");

    wheel(&mut painter, 0.06);
    assert_eq!(rest_of(&main), Some(None));
    tick(&mut painter, 0.1);
    assert_eq!(rest_of(&main), None, "nothing changed, nothing posted");

    tick(&mut painter, 0.061 + WHEEL_REST_SECONDS);
    assert_eq!(
        rest_of(&main),
        Some(Some(terminus::link::Vector2D::new(0.0, 40.0))),
        "the quiet interval ended the gesture at rest"
    );
    assert!(!painter.is_animating());
}

#[cfg_attr(coverage_nightly, coverage(off))]
mod frame_images {

    #[test]
    fn every_describe_of_one_image_keeps_the_same_buffer_identity() {
        use terminus::link::FrameImages as _;

        let images = prime_radiant::TestImages::new();
        let pixels = prime_radiant::rgba8(1, 1, vec![1, 2, 3, 255]);
        let published = pixels.data.id();
        images.insert("app:///pixel.png", pixels);

        let pixels_of = || {
            images
                .describe(
                    &terminus::link::PictureKey::Url(std::sync::Arc::from("app:///pixel.png")),
                    terminus::link::ImageSizeHint::UNBOUNDED,
                )
                .and_then(|image| image.pixels)
                .map(terminus::link::PictureFrame::into_pixels)
                .expect("a published image is described with its pixels")
        };
        let first = pixels_of();
        let second = pixels_of();
        assert_eq!(
            first.data.id(),
            published,
            "the buffer that comes out is the one that went in — no copy"
        );
        assert_eq!(
            first.data.id(),
            second.data.id(),
            "and every later describe is the same GPU resource"
        );
    }

    #[test]
    fn a_released_source_is_described_without_pixels_until_a_want_restores_it() {
        use terminus::link::{FrameImages as _, ImageSizeHint};

        let source = "app:///pixel.png";
        let key = terminus::link::PictureKey::Url(std::sync::Arc::from(source));
        let keeping = prime_radiant::TestImages::new();
        keeping.insert(source, prime_radiant::rgba8(1, 1, vec![1, 2, 3, 255]));
        keeping.release(std::slice::from_ref(&key));
        assert!(
            keeping
                .describe(&key, ImageSizeHint::UNBOUNDED)
                .is_some_and(|image| image.pixels.is_some()),
            "a keeping store ignores the release"
        );
        assert_eq!(keeping.reads().len(), 1, "and logs the describe");

        let images = prime_radiant::TestImages::releasing();
        let published = prime_radiant::rgba8(1, 1, vec![1, 2, 3, 255]);
        let weak = published.data.downgrade();
        images.insert(source, published);
        let described = images
            .describe(&key, ImageSizeHint::UNBOUNDED)
            .expect("a published image is described");
        assert!(described.pixels.is_some(), "with its pixels while held");
        drop(described.pixels);
        images.release(std::slice::from_ref(&key));
        assert!(weak.upgrade().is_none(), "the release dropped the blob");
        let absent = images
            .describe(&key, ImageSizeHint::UNBOUNDED)
            .expect("still described");
        assert!(absent.pixels.is_none());
        assert_eq!(absent.descriptor, described.descriptor);
        assert!(images.reads().is_empty(), "a describe restores nothing");

        images.want(&key, ImageSizeHint::UNBOUNDED);
        assert_eq!(images.reads().len(), 1, "the want restored it");
        let restored = images
            .describe(&key, ImageSizeHint::UNBOUNDED)
            .and_then(|image| image.pixels)
            .map(terminus::link::PictureFrame::into_pixels)
            .expect("described with its pixels again");
        assert_ne!(restored.data.id(), described.descriptor.blob, "a new blob");
        assert_eq!(restored.data.as_ref(), &[1, 2, 3, 255]);
        assert!(images.holds_pixels(source));
    }

    #[test]
    fn an_unpublished_source_is_described_as_nothing() {
        use terminus::link::FrameImages as _;

        let images = prime_radiant::TestImages::new();
        assert!(
            images
                .describe(
                    &terminus::link::PictureKey::Url(std::sync::Arc::from("app:///missing.png")),
                    terminus::link::ImageSizeHint::UNBOUNDED
                )
                .is_none()
        );
    }
}
