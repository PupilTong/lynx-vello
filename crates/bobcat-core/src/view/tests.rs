use std::sync::{Arc, mpsc};

use super::{ListenerUpdate, MainCommand, frame_size};
use crate::tree::{LynxDocument, PageConfig, Viewport, new_document};

/// A phone-shaped document, ready for a main thread to be started over it.
fn document() -> LynxDocument {
    new_document(Viewport::new(393.0, 727.0), PageConfig::default())
}

/// Starts a view over `document` and `entry`, the IO-free half of
/// construction.
fn view_over(
    events: Arc<dyn super::EventRequester>,
    document: LynxDocument,
    entry: &str,
) -> super::OffscreenLynxView {
    super::LynxView::start(
        document,
        Viewport::new(393.0, 727.0),
        frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        events,
        super::EntryModule {
            source: entry.to_owned(),
            url: "app:///main.js".to_owned(),
        },
    )
    .expect("the test view starts")
}

/// A view with every seam built but no main thread: the command receiver
/// and the listener-update sender are handed back so a test can play
/// both halves of the main thread's side of the seam. Probes answer
/// `None` and `BeginFrame`s are withheld — nobody would ever service
/// them.
fn detached() -> (
    super::OffscreenLynxView,
    mpsc::Receiver<MainCommand>,
    mpsc::Sender<ListenerUpdate>,
) {
    let document = document();
    let store = Arc::clone(document.image_store());
    let (view, receiver, _events, listeners) = super::LynxView::with_channel(
        store,
        Viewport::new(393.0, 727.0),
        frame_size(393.0, 727.0, 1.0).expect("the test viewport is valid"),
        Arc::new(|| {}),
    );
    (view, receiver, listeners)
}

#[test]
fn frame_size_applies_the_device_scale_once() {
    let size = frame_size(393.0, 727.0, 2.0).unwrap();
    assert_eq!((size.width, size.height), (786, 1_454));
}

#[test]
fn frame_size_rejects_unbounded_targets() {
    let error = frame_size(20_000.0, 100.0, 1.0).unwrap_err();
    assert!(error.to_string().contains("16384"));
}

/// The store a view is built over is the one the paint walk reads, and
/// the pixels reach it without a copy: the buffer identity that comes
/// back out of the main-thread document is the one that went in.
#[test]
fn the_installed_image_store_is_the_one_the_document_reads() {
    let mut document = document();
    let images = Arc::new(flashbulb::TestImages::new());
    let pixels = flashbulb::rgba8(1, 1, vec![1, 2, 3, 255]);
    let pixel_id = pixels.data.id();
    images.insert("app:///pixel.png", pixels);
    document.set_image_store(Arc::clone(&images) as Arc<dyn dom::ImageStore>);

    let mut view = view_over(
        Arc::new(|| {}),
        document,
        "globalThis.renderPage = function () { __CreatePage('card', 0); };",
    );
    let (hit, miss) = view
        .probe_document(move |tree| {
            (
                tree.image_store()
                    .peek("app:///pixel.png")
                    .map(|image| image.data.id()),
                tree.image_store().peek("app:///missing.png").is_none(),
            )
        })
        .expect("the main thread answers probes");
    assert_eq!(hit, Some(pixel_id));
    assert!(miss);
}

/// An emit decision costs one name-table lookup and stops there unless a
/// listener exists; when it crosses, it crosses as plain data — the
/// target id, not a path. Liveness is the main thread's to check at
/// delivery.
#[test]
fn an_emit_decision_crosses_only_when_a_listener_wants_it() {
    use crate::gesture::{EmitEvent, InputDecision, InputDecisions, TAP_EVENT};

    let (mut view, commands, listeners) = detached();
    // The permanent page element's packed handle, as script would name it.
    let target = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    let emit = |view: &mut super::OffscreenLynxView| {
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

    let publish = |update| {
        listeners.send(update).expect("the view holds the receiver");
    };

    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "an empty listener set sends nothing"
    );

    publish(ListenerUpdate::Available(Arc::from("pointerup")));
    view.listener_names.sync();
    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "a listener on another name sends nothing"
    );

    // An update that has not been synced yet is not yet visible: the
    // replica moves at pass boundaries, which is the one pass of
    // staleness this design accepts in exchange for the lock.
    publish(ListenerUpdate::Available(Arc::from(TAP_EVENT)));
    emit(&mut view);
    assert!(
        commands.try_recv().is_err(),
        "an unsynced registration does not open the name mid-pass"
    );

    view.listener_names.sync();
    emit(&mut view);
    let command = commands.try_recv().expect("the listened-for name crosses");
    let MainCommand::DispatchEvent {
        name, target: sent, ..
    } = command
    else {
        panic!("an emit decision becomes a dispatch command");
    };
    assert_eq!(name, TAP_EVENT);
    assert_eq!(sent, target);

    // And the edge closes the name again: the main thread publishes the
    // last removal, and from the next sync nothing crosses.
    publish(ListenerUpdate::Unavailable(Arc::from(TAP_EVENT)));
    view.listener_names.sync();
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
    let (mut view, _commands, listeners) = detached();
    let names = &mut view.listener_names;

    names.sync();
    assert!(!names.contains("tap"), "nothing has been published yet");

    for update in [
        ListenerUpdate::Available(Arc::from("tap")),
        ListenerUpdate::Available(Arc::from("scroll")),
        ListenerUpdate::Unavailable(Arc::from("tap")),
    ] {
        listeners.send(update).expect("the view holds the receiver");
    }
    names.sync();
    assert!(
        !names.contains("tap"),
        "the closing edge follows the opening one and wins"
    );
    assert!(names.contains("scroll"), "the other name stays open");

    // The main thread going away is not an unregistration: what it
    // published last is still the last true answer.
    drop(listeners);
    names.sync();
    assert!(
        names.contains("scroll"),
        "a closed channel leaves the snapshot standing"
    );
}

/// A scroll decision crosses nothing: it lands in the presenting side's
/// intents, which are the offsets composition shows, and the main
/// thread hears about scrolling only when a refill writes offsets back.
/// With no published frame there is no geometry to consume against, so
/// the decision evaporates entirely.
#[test]
fn a_scroll_decision_sends_no_command() {
    use crate::gesture::{InputDecision, InputDecisions};

    let (mut view, commands, _listeners) = detached();
    let node = dom::NodeId::from_bits(2).expect("a well-formed packed handle");
    let mut decisions = InputDecisions::new();
    decisions.push(InputDecision::Scroll {
        pointer: None,
        from: node,
        delta: dom::Vector2D::new(0.0, 5.0),
    });
    view.execute_decisions(&mut decisions, None);
    assert!(
        commands.try_recv().is_err(),
        "a windowed scroll never crosses the command channel"
    );
    assert!(view.scroll_intents.offsets.is_empty());
}

/// Boot's final flush is a commit: by the time `ScriptFinished` is
/// pumped, a frame is published and the document — owned by the main
/// thread — answers probes.
#[test]
fn a_booted_view_commits_and_publishes() {
    use std::time::{Duration, Instant};

    use super::EngineEvent;

    let (wake_sender, wake_receiver) = mpsc::channel();
    let mut view = view_over(
        Arc::new(move || {
            let _ = wake_sender.send(());
        }),
        document(),
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
