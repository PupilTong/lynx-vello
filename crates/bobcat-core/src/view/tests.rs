//! Routing and lifetime invariants for the group's shared return FIFO.

use std::time::Duration;

use super::*;

struct Links {
    first: PainterLink,
    second: PainterLink,
    notify_first: ToPainterSender<NoWakeup>,
    notify_second: ToPainterSender<NoWakeup>,
    commands: Mailbox<ToMain>,
}

fn links() -> Links {
    let (commands, receiver) = Mailbox::channel();
    let (notifications, inbox) = Mailbox::channel();
    let inbox = Rc::new(inbox);
    let requester = Arc::new(NoWakeup);
    let link = |id| {
        let view = ViewId(id);
        let (painter, frames) = view_link(
            view,
            &commands,
            Rc::clone(&inbox),
            event_waker(Arc::clone(&requester)),
            Arc::new(StartupControl::default()),
        );
        let notify =
            ToPainterSender::new(view, notifications.clone(), frames, Arc::clone(&requester));
        (painter, notify)
    };
    let (first, notify_first) = link(0);
    let (second, notify_second) = link(1);
    Links {
        first,
        second,
        notify_first,
        notify_second,
        commands: receiver,
    }
}

#[test]
fn a_turn_preserves_other_views_messages_and_their_order() {
    let mut links = links();
    links
        .notify_second
        .send(ToPainter::ListenerAvailable(Arc::from("tap")));
    links
        .notify_first
        .send(ToPainter::Engine(EngineEvent::ScriptFinished));
    links
        .notify_second
        .send(ToPainter::ListenerUnavailable(Arc::from("tap")));
    links
        .notify_second
        .send(ToPainter::RequestImages(vec![Arc::from("photo.png")]));
    links
        .notify_first
        .send(ToPainter::RequestSource(SourceRequest::Entry(
            "entry.js".into(),
        )));
    links
        .notify_second
        .send(ToPainter::Engine(EngineEvent::ScriptFinished));

    assert!(matches!(links.first.drain().as_slice(), [
        ToPainter::Engine(EngineEvent::ScriptFinished),
        ToPainter::RequestSource(SourceRequest::Entry(url)),
    ] if url == "entry.js"));
    // New arrivals follow the sibling's buffered batch, even after the last
    // sender disconnects.
    links
        .notify_second
        .send(ToPainter::ListenerAvailable(Arc::from("longpress")));
    drop(links.notify_first);
    drop(links.notify_second);
    assert!(matches!(links.second.drain().as_slice(), [
        ToPainter::ListenerAvailable(available),
        ToPainter::ListenerUnavailable(unavailable),
        ToPainter::RequestImages(images),
        ToPainter::Engine(EngineEvent::ScriptFinished),
        ToPainter::ListenerAvailable(last),
    ] if &**available == "tap" && &**unavailable == "tap" && &*images[0] == "photo.png" && &**last == "longpress"));
    assert!(links.first.drain().is_empty());
    assert!(links.second.drain().is_empty());
}

#[test]
fn dropping_a_view_releases_buffered_and_late_notifications() {
    let mut links = links();
    let buffered: Arc<str> = Arc::from("buffered");
    let buffered_weak = Arc::downgrade(&buffered);
    links
        .notify_second
        .send(ToPainter::ListenerAvailable(buffered));
    links.first.sync();
    assert!(
        buffered_weak.upgrade().is_some(),
        "the other view has not taken its turn"
    );
    drop(links.second);
    assert!(
        buffered_weak.upgrade().is_none(),
        "removing the view drops its buffer"
    );

    let late: Arc<str> = Arc::from("late");
    let late_weak = Arc::downgrade(&late);
    links.notify_second.send(ToPainter::ListenerAvailable(late));
    links
        .notify_first
        .send(ToPainter::Engine(EngineEvent::ScriptFinished));
    links.first.sync();
    assert!(
        late_weak.upgrade().is_none(),
        "late messages cannot recreate a dead view"
    );
    assert!(matches!(
        links.first.take_events().as_slice(),
        [EngineEvent::ScriptFinished]
    ));
}

#[test]
fn an_offscreen_wait_routes_sibling_messages_without_accepting_their_ack() {
    let mut links = links();
    let seq = links
        .first
        .begin_frame(0.0)
        .expect("the group accepts a frame");
    // A sibling's acknowledgement cannot satisfy this view's frame.
    links.notify_second.send(ToPainter::BeginFrameServiced(seq));
    assert!(!links.first.wait_begin_frame(seq, Duration::ZERO));

    let sender = std::thread::spawn(move || {
        assert!(matches!(
            links.commands.recv(None).unwrap(),
            (Some(_), ToMain::BeginFrame { .. })
        ));
        links
            .notify_second
            .send(ToPainter::Engine(EngineEvent::ScriptFinished));
        links.notify_first.send(ToPainter::BeginFrameServiced(seq));
    });
    assert!(links.first.wait_begin_frame(seq, Duration::from_secs(10)));
    links.second.sync();
    assert!(matches!(
        links.second.take_events().as_slice(),
        [EngineEvent::ScriptFinished]
    ));
    sender.join().unwrap();
}

#[test]
fn a_failure_ends_an_offscreen_wait_while_the_group_stays_connected() {
    let mut links = links();
    let seq = links
        .first
        .begin_frame(0.0)
        .expect("the group accepts a frame");
    links
        .notify_first
        .send(ToPainter::Engine(EngineEvent::StartupFailed(
            EngineError::UnknownFontFamily("missing".into()).into(),
        )));
    assert!(!links.first.wait_begin_frame(seq, Duration::from_secs(10)));
    assert!(matches!(
        links.first.take_events().as_slice(),
        [EngineEvent::StartupFailed(_)]
    ));
    links
        .notify_second
        .send(ToPainter::Engine(EngineEvent::ScriptFinished));
    links.second.sync();
    assert!(matches!(
        links.second.take_events().as_slice(),
        [EngineEvent::ScriptFinished]
    ));
}
