use std::sync::Arc;
use std::time::Duration;

use super::{Output, TestPainter};
use crate::main::tree::{LynxDocument, PageConfig, Viewport, new_document};
use crate::view::{
    DetachedLink, EngineEvent, EventRequester, FrameSize, NoWakeup, ToMain, ToPainter,
};

/// A phone-shaped document, ready for a main thread to be started over it.
fn document() -> LynxDocument {
    new_document(Viewport::new(393.0, 727.0), PageConfig::default())
}

/// Starts a view over `document` and `entry`, the IO-free half of
/// construction.
fn view_over<R: EventRequester>(
    events: Arc<R>,
    build_document: impl FnOnce() -> LynxDocument + Send + 'static,
    entry: &str,
) -> TestPainter {
    TestPainter::start(
        build_document,
        Viewport::new(393.0, 727.0),
        FrameSize::for_viewport(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        events,
        super::EntryModule {
            source: entry.to_owned(),
            url: "app:///main.js".to_owned(),
        },
        // Nowhere to draw: nothing here reads pixels, and a GPU device per
        // test is a third of a second each.
        Output::None,
    )
    .expect("the test view starts")
}

/// A painter with every seam built but no main thread: the other end of its
/// link is handed back so a test can play the main thread's whole side of
/// it. Probes answer `None` and `BeginFrame`s are withheld — nobody would
/// ever service them.
fn detached() -> (TestPainter, DetachedLink<NoWakeup>) {
    TestPainter::with_link(
        Viewport::new(393.0, 727.0),
        FrameSize::for_viewport(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        Arc::new(NoWakeup),
        Output::None,
    )
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

/// One drain of the notification FIFO applies every kind of thing that
/// rides it — a lifecycle event `pump` will hand back, a listener edge, a
/// `BeginFrame` acknowledgement, and the redraw an announced frame asks
/// for — and applies each of them exactly once.
#[test]
fn one_drain_applies_every_kind_of_notification() {
    let (mut view, main) = detached();
    assert!(!view.link.take_redraw(), "a fresh link owes no frame");
    assert!(view.pump().is_empty(), "and has nothing to report");

    for notification in [
        ToPainter::ListenerAvailable(Arc::from("tap")),
        ToPainter::Engine(EngineEvent::ScriptFinished),
        ToPainter::BeginFrameServiced(7),
        ToPainter::FrameChanged,
    ] {
        main.notify.send(notification);
    }

    let events = view.pump();
    assert!(matches!(events.as_slice(), [EngineEvent::ScriptFinished]));
    assert!(view.link.has_listener("tap"));
    assert!(
        view.link.wait_begin_frame(7, Duration::ZERO),
        "the acknowledgement rode the same drain, so the wait never blocks"
    );
    assert!(
        view.link.take_redraw(),
        "an announced frame asks for a draw"
    );

    assert!(view.pump().is_empty(), "an event is handed back once");
    assert!(!view.link.take_redraw(), "and a request is taken once");
}

/// Frames do not queue: the mailbox holds one slot, so a painting side
/// that syncs after several commits sees the newest and never the ones it
/// slept through — however many announcements arrived for them.
#[test]
fn the_frame_mailbox_keeps_only_the_newest_commit() {
    let (mut view, main) = detached();
    let mut document = document();
    let first = document.commit();
    document.set_viewport(320.0, 640.0);
    let second = document.commit();
    assert_ne!(first.commit_id(), second.commit_id());

    main.notify.publish_frame(Arc::clone(&first));
    main.notify.publish_frame(Arc::clone(&second));
    view.link.sync();

    let published = view.link.frame().expect("the sync adopted a frame");
    assert_eq!(
        published.commit_id(),
        second.commit_id(),
        "the second publish overwrote the first rather than queueing behind it"
    );
}

/// An emit decision costs one name-table lookup and stops there unless a
/// listener exists; when it crosses, it crosses as plain data — the
/// target id, not a path. Liveness is the main thread's to check at
/// delivery.
#[test]
fn an_emit_decision_crosses_only_when_a_listener_wants_it() {
    use super::gesture::{EmitEvent, InputDecision, InputDecisions, TAP_EVENT};

    let (mut view, main) = detached();
    // The permanent page element's packed handle, as script would name it.
    let target = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    let emit = |view: &mut TestPainter| {
        let mut decisions = InputDecisions::new();
        decisions.push(InputDecision::Emit(EmitEvent {
            name: TAP_EVENT,
            target,
            position: dom::Point2D::new(1.0, 1.0),
            wheel: None,
        }));
        view.execute_decisions(&mut decisions, None);
        assert!(decisions.is_empty(), "the queue is always drained");
    };

    let publish = |notification| main.notify.send(notification);
    let commands = &main;

    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "an empty listener set sends nothing"
    );

    publish(ToPainter::ListenerAvailable(Arc::from("pointerup")));
    view.link.sync();
    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "a listener on another name sends nothing"
    );

    // An update that has not been synced yet is not yet visible: the
    // replica moves at pass boundaries, which is the one pass of
    // staleness this design accepts in exchange for the lock.
    publish(ToPainter::ListenerAvailable(Arc::from(TAP_EVENT)));
    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "an unsynced registration does not open the name mid-pass"
    );

    view.link.sync();
    emit(&mut view);
    let command = commands.try_recv().expect("the listened-for name crosses");
    let ToMain::DispatchEvent {
        name, target: sent, ..
    } = command
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(name, TAP_EVENT);
    assert_eq!(sent, target);

    // And the edge closes the name again: the main thread publishes the
    // last removal, and from the next sync nothing crosses.
    publish(ToPainter::ListenerUnavailable(Arc::from(TAP_EVENT)));
    view.link.sync();
    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "the closing edge stops the crossing"
    );
}

/// A sync applies whatever arrived, in order, and stops at the last of
/// it — never blocking on a main thread that may be mid-registration.
///
/// The ordering matters because the two edges for one name are a pair:
/// a name registered and unregistered between two passes must leave the
/// replica closed, not open.
#[test]
fn a_sync_applies_arrived_edges_in_order_and_does_not_block() {
    let (mut view, main) = detached();

    view.link.sync();
    assert!(
        !view.link.has_listener("tap"),
        "nothing has been published yet"
    );

    for edge in [
        ToPainter::ListenerAvailable(Arc::from("tap")),
        ToPainter::ListenerAvailable(Arc::from("scroll")),
        ToPainter::ListenerUnavailable(Arc::from("tap")),
    ] {
        main.notify.send(edge);
    }
    view.link.sync();
    assert!(
        !view.link.has_listener("tap"),
        "the closing edge follows the opening one and wins"
    );
    assert!(
        view.link.has_listener("scroll"),
        "the other name stays open"
    );

    // The main thread going away is not an unregistration: what it
    // published last is still the last true answer.
    drop(main);
    view.link.sync();
    assert!(
        view.link.has_listener("scroll"),
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

    let (mut view, main) = detached();
    let node = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: node,
        delta: dom::Vector2D::new(0.0, 5.0),
    });
    view.execute_decisions(&mut decisions, None);
    assert!(
        main.try_recv().is_err(),
        "a windowed scroll never crosses the command channel"
    );
    assert!(view.scroll_intents.offsets.is_empty());
}

/// A requester that records every wake, so a test can wait on the host
/// loop actually being asked to run.
struct WakeSignal(flume::Sender<()>);

impl EventRequester for WakeSignal {
    fn request_event(&self) {
        let _ = self.0.send(());
    }
}

/// A view with nowhere to present owes the display nothing, whatever else
/// is true of it.
///
/// The rule that keeps a host from waiting on a frame clock for a view that
/// has no display to keep up with: an offscreen view's frames are asked for
/// through `tick`, and a painter with no target at all has none to give.
#[test]
fn a_view_that_presents_to_no_window_owes_no_frame() {
    let (painter, _main) = detached();
    painter.refresh();
    assert!(
        painter.link.redraw_owed(),
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
    let (painter, main) = TestPainter::with_link(
        Viewport::new(393.0, 727.0),
        FrameSize::for_viewport(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        Arc::new(WakeSignal(wake_sender)),
        Output::None,
    );

    painter.refresh();
    assert!(
        wakes.try_recv().is_err(),
        "a painter-local request is answered by the turn it was made in"
    );
    assert!(painter.link.take_redraw(), "and the frame is still owed");

    main.notify.publish_frame(document().commit());
    assert!(
        wakes.try_recv().is_ok(),
        "a commit from the other thread is the one thing that must wake"
    );
    assert!(wakes.try_recv().is_err(), "and it wakes exactly once");
}

/// Boot's final flush is a commit: by the time `ScriptFinished` is
/// pumped, a frame is published and the document — owned by the main
/// thread — answers probes.
#[test]
fn a_booted_view_commits_and_publishes() {
    use std::time::{Duration, Instant};

    use crate::view::EngineEvent;

    let (wake_sender, wake_receiver) = flume::unbounded();
    let mut view = view_over(
        Arc::new(WakeSignal(wake_sender)),
        document,
        r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              __AppendElement(page, __CreateView(0));
              __FlushElementTree();
              __AppendElement(page, __CreateView(0));
            };
            ",
    );

    // One wakeup carries either kind of engine work: the boot commit's
    // frame or a lifecycle event. The law under test is the ordering —
    // whichever wakeup carries the event, `pump` observes it right then,
    // with nothing polled for and nothing slept on.
    let deadline = Instant::now() + Duration::from_secs(5);
    let finished = loop {
        wake_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("script completion must wake the host event loop");
        if let Some(event) = view.pump().into_iter().find(|event| {
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

    let frame = view
        .published_frame()
        .expect("the boot's flush published a committed frame");
    assert!(frame.commit_id() > 0);

    let (views, connected, laid_out) = view
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
        .expect("the main thread answers probes");
    assert_eq!(views, 2, "the boot script appends two views");
    assert!(connected, "both views are attached");
    assert!(laid_out, "the boot's final flush laid the page out");
}

/// A startup outcome that arrives while a host fetch is still pending must
/// still end construction.
///
/// This is the hang that shipped in the first draft of the message-driven
/// startup: while the painter awaited a host future it read nothing, and an
/// outcome already sitting in the FIFO was never observed. `bobcat-main`
/// mounts each pushed source while the painter's next fetch is already in
/// flight, so a failure it decides there lands in exactly this window.
///
/// Natively no ordinary failure lands in that window; on wasm32 under
/// `panic = "abort"` a trapping main thread's `ScriptRunError` is exactly it,
/// and it is unreachable from an integration test. So the invariant is pinned
/// here: whatever the painter is waiting on, it is also watching the link.
#[tokio::test]
async fn an_outcome_arriving_during_a_pending_fetch_ends_startup() {
    let (mut painter, main) = detached();

    // The outcome is sent from another thread *after* the painter has had
    // time to suspend on the entry fetch — which is the whole point: an
    // outcome already in the FIFO would be seen before the fetch began and
    // would never exercise the wait. `NeverAnswers` keeps the fetch pending
    // forever, as a host is entitled to.
    let notify = main.notify.clone();
    let decided = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        notify.send(ToPainter::Started(Err(crate::view::EngineError::Thread {
            name: "script",
            message: "boot failed while a fetch was in flight".to_owned(),
        }
        .into())));
    });

    let error = tokio::time::timeout(
        Duration::from_secs(10),
        painter.serve_startup(Vec::new(), "app:///main.js".to_owned()),
    )
    .await
    .expect("a decided outcome must not wait on a fetch that never answers")
    .expect_err("the decided failure ends construction");
    assert!(
        format!("{error}").contains("boot failed while a fetch was in flight"),
        "and it is the failure the main thread decided: {error}"
    );
    decided.join().expect("the deciding thread finishes");
}
