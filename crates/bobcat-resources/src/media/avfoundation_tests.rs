//! The macOS player against `tests/fixtures/video/rgb.mp4`: 64 × 48, ten
//! frames a second, red for 0.4 s, green for 0.4 s, blue for 0.4 s, H.264
//! in MP4 with no audio track (`tests/fixtures/video/README.md` has the
//! command that made it).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::{ImageDescriptor, PictureFrame};
use bytes::Bytes;
use url::Url;
use vant::media::{
    Media, MediaErrorKind, MediaEvent, MediaPlayer, MediaSettings, MediaSink, MediaSource,
};

use super::{AvMedia, AvPlayer};

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/video/rgb.mp4")
}

/// What a player handed over, in order.
#[derive(Debug)]
enum Seen {
    Frame([u8; 4]),
    Event(MediaEvent),
}

/// A sink that records everything, and wants frames while `wants` says so.
#[derive(Default)]
struct Recorder {
    wants: AtomicBool,
    seen: Mutex<Vec<(Instant, Seen)>>,
}

#[derive(Clone)]
struct Sink(Arc<Recorder>);

impl MediaSink<std::convert::Infallible> for Sink {
    fn wants_frames(&self) -> bool {
        self.0.wants.load(Ordering::SeqCst)
    }

    fn frame(&self, frame: PictureFrame, descriptor: ImageDescriptor) {
        let pixels = frame.into_pixels();
        assert_eq!(ImageDescriptor::of(&pixels), descriptor);
        let data = pixels.data.data();
        let centre = ((pixels.height / 2 * pixels.width + pixels.width / 2) * 4) as usize;
        let texel = [
            data[centre],
            data[centre + 1],
            data[centre + 2],
            data[centre + 3],
        ];
        self.0
            .seen
            .lock()
            .unwrap()
            .push((Instant::now(), Seen::Frame(texel)));
    }

    fn event(&self, event: MediaEvent) {
        self.0
            .seen
            .lock()
            .unwrap()
            .push((Instant::now(), Seen::Event(event)));
    }
}

impl Sink {
    fn new(wants: bool) -> Self {
        let recorder = Recorder::default();
        recorder.wants.store(wants, Ordering::SeqCst);
        Self(Arc::new(recorder))
    }

    fn open(&self, source: MediaSource, looping: bool) -> AvPlayer {
        Media::<std::convert::Infallible>::open(
            &AvMedia,
            source,
            MediaSettings {
                looping,
                muted: true,
            },
            self.clone(),
        )
        .expect("AVFoundation opens a player")
    }

    /// Waits until `done` holds, or fails after ten seconds.
    fn until(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if done() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what}: {:?}",
                self.0.seen.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn frames(&self) -> Vec<[u8; 4]> {
        self.0
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, seen)| match seen {
                Seen::Frame(texel) => Some(*texel),
                Seen::Event(_) => None,
            })
            .collect()
    }

    fn events(&self) -> Vec<MediaEvent> {
        self.0
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, seen)| match seen {
                Seen::Event(event) => Some(event.clone()),
                Seen::Frame(_) => None,
            })
            .collect()
    }

    fn ended(&self) -> usize {
        self.events()
            .iter()
            .filter(|event| **event == MediaEvent::Ended)
            .count()
    }
}

/// Which of the clip's three colours `texel` is, within H.264's rounding.
fn colour(texel: [u8; 4]) -> Option<&'static str> {
    let near = |expected: [u8; 3]| {
        texel
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 24)
    };
    assert_eq!(texel[3], 255, "a video frame is opaque");
    if near([255, 0, 0]) {
        Some("red")
    } else if near([0, 255, 0]) {
        Some("green")
    } else if near([0, 0, 255]) {
        Some("blue")
    } else {
        None
    }
}

fn file() -> MediaSource {
    MediaSource::Url(Url::from_file_path(fixture()).unwrap())
}

/// An open video shows its first frame paused, before any `play`, and
/// reports it with the clip's natural size and duration; nothing more comes
/// while it is paused.
#[test]
fn an_open_video_shows_its_first_frame_paused() {
    let sink = Sink::new(true);
    let _player = sink.open(file(), false);
    sink.until("the first frame", || {
        sink.events()
            .iter()
            .any(|event| matches!(event, MediaEvent::FirstFrame { .. }))
    });
    let events = sink.events();
    let [
        MediaEvent::FirstFrame {
            width,
            height,
            duration,
        },
    ] = events.as_slice()
    else {
        panic!("one first frame: {events:?}");
    };
    assert_eq!((*width, *height), (64, 48));
    assert!(
        duration.abs_diff(Duration::from_millis(1200)) < Duration::from_millis(50),
        "{duration:?}"
    );
    assert_eq!(
        sink.frames()
            .iter()
            .map(|texel| colour(*texel))
            .collect::<Vec<_>>(),
        [Some("red")]
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        sink.frames().len(),
        1,
        "a paused video publishes nothing new"
    );
}

/// `play` plays the clip through, its frames in order and on the player's
/// clock, and reports its end once; a `play` after the end plays it again
/// from the start.
#[test]
fn a_played_video_shows_its_frames_and_ends() {
    let sink = Sink::new(true);
    let player = sink.open(file(), false);
    sink.until("the first frame", || !sink.frames().is_empty());
    let started = Instant::now();
    player.play();
    sink.until("the end", || sink.ended() == 1);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(3),
        "the clip is 1.2 s long and took {took:?}"
    );
    let mut colours: Vec<&str> = sink.frames().into_iter().filter_map(colour).collect();
    colours.dedup();
    assert_eq!(colours, ["red", "green", "blue"]);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sink.ended(), 1, "ended once");

    let before = sink.frames().len();
    player.play();
    sink.until("the second end", || sink.ended() == 2);
    let mut again: Vec<&str> = sink.frames()[before..]
        .iter()
        .copied()
        .filter_map(colour)
        .collect();
    again.dedup();
    assert_eq!(again.first(), Some(&"red"), "from the start: {again:?}");
}

/// A looping video starts over at its end and never reports one; turning
/// `loop` off lets it end.
#[test]
fn a_looping_video_starts_over_and_does_not_end() {
    let sink = Sink::new(true);
    let player = sink.open(file(), true);
    sink.until("the first frame", || !sink.frames().is_empty());
    player.play();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(sink.ended(), 0, "a looping video does not end");
    let mut colours: Vec<&str> = sink.frames().into_iter().filter_map(colour).collect();
    colours.dedup();
    assert!(
        colours.len() >= 6 && colours[..6] == ["red", "green", "blue", "red", "green", "blue"],
        "it played twice over: {colours:?}"
    );
    player.set_loop(false);
    sink.until("the end once looping stops", || sink.ended() == 1);
}

/// A paused player hands over no frame, and one whose sink wants no frames
/// hands over none beyond its first while it plays on, and then the frame it
/// stops on: a picture drawn again after its video ended shows the end.
#[test]
fn a_paused_or_unwanted_player_publishes_nothing() {
    let sink = Sink::new(true);
    let player = sink.open(file(), true);
    sink.until("the first frame", || !sink.frames().is_empty());
    player.play();
    sink.until("a second frame", || sink.frames().len() >= 2);
    player.pause();
    std::thread::sleep(Duration::from_millis(100));
    let paused = sink.frames().len();
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        sink.frames().len(),
        paused,
        "a paused player publishes nothing"
    );

    let unwanted = Sink::new(false);
    let player = unwanted.open(file(), false);
    unwanted.until("the first frame", || !unwanted.frames().is_empty());
    player.play();
    unwanted.until("the end", || unwanted.ended() == 1);
    assert_eq!(
        unwanted
            .frames()
            .iter()
            .map(|texel| colour(*texel))
            .collect::<Vec<_>>(),
        [Some("red"), Some("blue")],
        "the first frame, and the one it ended on"
    );
    let seen = unwanted.0.seen.lock().unwrap();
    assert!(
        matches!(seen.last(), Some((_, Seen::Event(MediaEvent::Ended)))),
        "the last frame is handed over before the end is reported: {seen:?}"
    );
}

/// Bytes the resource system holds play as a file does.
#[test]
fn bytes_play_as_a_file_does() {
    let sink = Sink::new(true);
    let bytes = Bytes::from(std::fs::read(fixture()).unwrap());
    let player = sink.open(
        MediaSource::Bytes {
            url: Url::parse("app:///clip.mp4").unwrap(),
            bytes,
            media_type: Some("video/mp4".to_owned()),
        },
        false,
    );
    sink.until("the first frame", || !sink.frames().is_empty());
    player.play();
    sink.until("the end", || sink.ended() == 1);
    drop(player);
}

/// A file `AVFoundation` cannot read is a failure of the player, and nothing
/// else is reported.
#[test]
fn an_unreadable_file_fails() {
    let sink = Sink::new(true);
    let _player = sink.open(
        MediaSource::Bytes {
            url: Url::parse("app:///broken.mp4").unwrap(),
            bytes: Bytes::from_static(b"not a video at all"),
            media_type: Some("video/mp4".to_owned()),
        },
        false,
    );
    sink.until("the failure", || !sink.events().is_empty());
    std::thread::sleep(Duration::from_millis(200));
    let events = sink.events();
    let [MediaEvent::Failed(error)] = events.as_slice() else {
        panic!("one failure: {events:?}");
    };
    assert_eq!(error.kind, MediaErrorKind::Playback);
    assert!(!error.message.is_empty());
    assert!(sink.frames().is_empty());
}
