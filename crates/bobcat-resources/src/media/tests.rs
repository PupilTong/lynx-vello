//! The video store against a test player that records what it is asked and
//! hands over what the test says, through a view's `ResourceFetcher` and
//! `FrameImages` as a host and a painter reach it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bobcat_core::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    FrameImages, ImageDescriptor, ImageEvent, ImageInbox, ImageSizeHint, MediaCommand,
    MediaErrorKind, MediaEvent, MediaSettings, PictureFrame, PictureId, PictureKey, PlaybackReport,
    PlaybackRequest,
};
use url::Url;
use vant::media::{Media, MediaError, MediaPlayer, MediaSink, MediaSource};
use vant::{NoStore, Parts, Platform};

use crate::{NativeDecoder, NativeFetch, Resources, ResourcesConfig, ViewResources};

/// What a test player's sink is reached through, the sink's type erased.
struct Handed {
    frame: Box<dyn Fn(PictureFrame, ImageDescriptor) + Send + Sync>,
    event: Box<dyn Fn(MediaEvent) + Send + Sync>,
    wants: Box<dyn Fn() -> bool + Send + Sync>,
}

/// What the test player was asked, and the sinks it was handed.
#[derive(Default)]
struct Log {
    /// Each open: whether it was handed bytes (and how many), and its
    /// settings.
    opens: Vec<(Option<usize>, MediaSettings)>,
    commands: Vec<MediaCommand>,
    sinks: Vec<Handed>,
    closed: usize,
}

#[derive(Clone, Default)]
struct TestMedia {
    log: Arc<Mutex<Log>>,
    /// Every open fails with this, when set.
    failing: Arc<Mutex<Option<MediaError>>>,
}

struct TestPlayer(Arc<Mutex<Log>>);

impl MediaPlayer for TestPlayer {
    fn play(&self) {
        self.0.lock().unwrap().commands.push(MediaCommand::Play);
    }

    fn pause(&self) {
        self.0.lock().unwrap().commands.push(MediaCommand::Pause);
    }

    fn set_loop(&self, looping: bool) {
        self.0
            .lock()
            .unwrap()
            .commands
            .push(MediaCommand::Loop(looping));
    }

    fn set_muted(&self, muted: bool) {
        self.0
            .lock()
            .unwrap()
            .commands
            .push(MediaCommand::Muted(muted));
    }
}

impl Drop for TestPlayer {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed += 1;
    }
}

impl Media<std::convert::Infallible> for TestMedia {
    type Player = TestPlayer;

    fn open<S: MediaSink<std::convert::Infallible>>(
        &self,
        source: MediaSource,
        settings: MediaSettings,
        sink: S,
    ) -> Result<TestPlayer, MediaError> {
        if let Some(error) = self.failing.lock().unwrap().clone() {
            return Err(error);
        }
        let bytes = match &source {
            MediaSource::Url(_) => None,
            MediaSource::Bytes { bytes, .. } => Some(bytes.len()),
        };
        let sink = Arc::new(sink);
        let mut log = self.log.lock().unwrap();
        log.opens.push((bytes, settings));
        log.sinks.push(Handed {
            frame: Box::new({
                let sink = Arc::clone(&sink);
                move |frame, descriptor| sink.frame(frame, descriptor)
            }),
            event: Box::new({
                let sink = Arc::clone(&sink);
                move |event| sink.event(event)
            }),
            wants: Box::new(move || sink.wants_frames()),
        });
        Ok(TestPlayer(Arc::clone(&self.log)))
    }
}

struct TestPlatform(TestMedia);

impl Platform for TestPlatform {
    type Fetch = NativeFetch;
    type Store = NoStore;
    type AsyncStore = NoStore;
    type Decoder = NativeDecoder;
    type Media = TestMedia;

    fn into_parts(self) -> Parts<NativeFetch, NoStore, NoStore, NativeDecoder, TestMedia> {
        Parts {
            fetch: NativeFetch,
            decoder: NativeDecoder,
            media: self.0,
            bytes: None,
            artifacts: None,
        }
    }
}

const CLIP: &str = "app:///clip.mp4";

/// A resource system over the test player, counting the host's wakeups,
/// with one view of it.
struct Rig {
    media: TestMedia,
    resources: Resources<TestPlatform>,
    view: ViewResources<TestPlatform>,
    inbox: ImageInbox,
    wakeups: Arc<AtomicUsize>,
}

impl Rig {
    fn new() -> Self {
        let media = TestMedia::default();
        let wakeups = Arc::new(AtomicUsize::new(0));
        let resources = Resources::with_platform(
            ResourcesConfig {
                worker_threads: 2,
                log_to_stderr: false,
                ..ResourcesConfig::default()
            },
            TestPlatform(media.clone()),
            Vec::new(),
            {
                let wakeups = Arc::clone(&wakeups);
                move || {
                    wakeups.fetch_add(1, Ordering::SeqCst);
                }
            },
        );
        resources
            .register(CLIP, vec![7_u8; 32], Some("video/mp4"))
            .expect("register");
        let (reports, inbox) = ImageInbox::new();
        let view = resources.for_view(reports);
        Self {
            media,
            resources,
            view,
            inbox,
            wakeups,
        }
    }

    /// Opens `CLIP` into a new picture and waits for its player.
    fn open(&self, settings: MediaSettings) -> PictureId {
        let picture = PictureId::new();
        self.view.request_playback(PlaybackRequest::Open {
            picture,
            source: Arc::from(CLIP),
            settings,
        });
        let opens = self.media.log.lock().unwrap().opens.len();
        self.settle("the player never opened", || {
            self.media.log.lock().unwrap().opens.len() > opens
        });
        picture
    }

    /// Takes host turns until `done` holds.
    fn settle(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.view.service_images();
            if done() {
                return;
            }
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// What the view was reported for its videos since the last call.
    fn reported(&self) -> Vec<(PictureId, MediaEvent)> {
        self.view.service_images();
        self.inbox
            .drain()
            .into_iter()
            .filter_map(|event| match event {
                ImageEvent::Playback {
                    picture,
                    report: PlaybackReport::Media(event),
                } => Some((picture, event)),
                _ => None,
            })
            .collect()
    }

    /// The player's sink, `index`th opened, hands over a 1x1 frame of
    /// `value`.
    fn hand(&self, index: usize, value: u8) {
        let pixels = ImageData {
            data: Blob::new(Arc::new(vec![value, value, value, 255])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::AlphaPremultiplied,
            width: 1,
            height: 1,
        };
        let descriptor = ImageDescriptor::of(&pixels);
        let log = self.media.log.lock().unwrap();
        (log.sinks[index].frame)(PictureFrame::Pixels(pixels), descriptor);
    }

    fn wants(&self, index: usize) -> bool {
        (self.media.log.lock().unwrap().sinks[index].wants)()
    }

    /// The value of the frame `picture`'s slot holds, taken.
    fn take(&self, picture: PictureId) -> Option<u8> {
        self.view
            .take_frame(picture)
            .map(|taken| taken.frame.into_pixels().data.data()[0])
    }
}

/// A registered video is fetched by this crate and handed to the player as
/// its bytes, under the settings asked for; commands made while the bytes
/// were in flight reach the player once it is open, in order.
#[test]
fn a_registered_video_is_handed_over_as_its_bytes() {
    let rig = Rig::new();
    let picture = PictureId::new();
    let settings = MediaSettings {
        looping: true,
        muted: true,
    };
    rig.view.request_playback(PlaybackRequest::Open {
        picture,
        source: Arc::from(CLIP),
        settings,
    });
    for command in [MediaCommand::Play, MediaCommand::Muted(false)] {
        rig.view
            .request_playback(PlaybackRequest::Media { picture, command });
    }
    rig.settle("the player never opened", || {
        !rig.media.log.lock().unwrap().opens.is_empty()
    });
    let log = rig.media.log.lock().unwrap();
    assert_eq!(log.opens, [(Some(32), settings)]);
    assert_eq!(
        log.commands,
        [MediaCommand::Play, MediaCommand::Muted(false)]
    );
}

/// A frame the player hands over while a view draws the picture is
/// published into its slot, wakes the host, and is taken once; a describe is
/// a draw.
#[test]
fn a_drawn_picture_publishes_its_player_frames() {
    let rig = Rig::new();
    let picture = rig.open(MediaSettings::default());
    assert!(!rig.wants(0), "nothing draws it yet");
    let hint = ImageSizeHint::new(1, 1);
    assert!(
        rig.view.describe(&PictureKey::Id(picture), hint).is_none(),
        "nothing published"
    );
    assert!(rig.wants(0), "a describe is a draw");

    let before = rig.wakeups.load(Ordering::SeqCst);
    rig.hand(0, 10);
    assert!(
        rig.wakeups.load(Ordering::SeqCst) > before,
        "the frame woke the host"
    );
    let described = rig
        .view
        .describe(&PictureKey::Id(picture), hint)
        .expect("described");
    assert_eq!(described.generation, 1);
    assert_eq!(described.next_change, None, "a player names no next change");
    assert_eq!(rig.take(picture), Some(10));
    assert_eq!(rig.take(picture), None, "taken once");
}

/// While no view's drawn set includes the picture its player is told so and
/// nothing it hands over is published; the newest frame is kept, and is
/// published again when the painter wants the picture.
#[test]
fn an_undrawn_picture_publishes_nothing_and_keeps_its_newest_frame() {
    let rig = Rig::new();
    let picture = rig.open(MediaSettings::default());
    let key = PictureKey::Id(picture);
    let hint = ImageSizeHint::new(1, 1);
    let _ = rig.view.describe(&key, hint);
    rig.view.retain(std::slice::from_ref(&key));
    rig.hand(0, 10);
    assert_eq!(rig.take(picture), Some(10));

    rig.view.retain(&[]);
    assert!(!rig.wants(0));
    rig.hand(0, 20);
    assert_eq!(rig.take(picture), None, "not published while not drawn");

    rig.view.retain(std::slice::from_ref(&key));
    assert!(rig.wants(0));
    rig.view.want(&key, hint);
    assert_eq!(
        rig.take(picture),
        Some(20),
        "the newest frame, handed over while not drawn, published again"
    );
}

/// What a player reports reaches the view that opened the picture in order,
/// in the host's turn, each report waking it.
#[test]
fn reports_reach_the_view_in_order() {
    let rig = Rig::new();
    let picture = rig.open(MediaSettings::default());
    let before = rig.wakeups.load(Ordering::SeqCst);
    {
        let log = rig.media.log.lock().unwrap();
        (log.sinks[0].event)(MediaEvent::FirstFrame {
            width: 64,
            height: 48,
            duration: Duration::from_millis(1200),
        });
        (log.sinks[0].event)(MediaEvent::Ended);
    }
    assert!(rig.wakeups.load(Ordering::SeqCst) >= before + 2);
    assert_eq!(
        rig.reported(),
        [
            (
                picture,
                MediaEvent::FirstFrame {
                    width: 64,
                    height: 48,
                    duration: Duration::from_millis(1200),
                }
            ),
            (picture, MediaEvent::Ended),
        ]
    );
    assert!(rig.reported().is_empty(), "reported once");
}

/// An open that fails is reported in the turn that asked for it, and a
/// `play` of that picture reports it again; nothing else reaches a player.
#[test]
fn a_failed_open_is_reported_and_reported_again_on_play() {
    let rig = Rig::new();
    let error = MediaError::new(MediaErrorKind::Playback, "the test player refused");
    *rig.media.failing.lock().unwrap() = Some(error.clone());
    let picture = PictureId::new();
    rig.view.request_playback(PlaybackRequest::Open {
        picture,
        source: Arc::from(CLIP),
        settings: MediaSettings::default(),
    });
    let mut reported = Vec::new();
    rig.settle("the failure never arrived", || {
        reported.extend(rig.reported());
        !reported.is_empty()
    });
    assert_eq!(reported, [(picture, MediaEvent::Failed(error.clone()))]);
    rig.view.request_playback(PlaybackRequest::Media {
        picture,
        command: MediaCommand::Pause,
    });
    assert!(rig.reported().is_empty(), "a pause reports nothing");
    rig.view.request_playback(PlaybackRequest::Media {
        picture,
        command: MediaCommand::Play,
    });
    assert_eq!(rig.reported(), [(picture, MediaEvent::Failed(error))]);
}

/// A source that does not resolve is the element's `-1`: a failure of the
/// source, reported at once, with nothing opened.
#[test]
fn a_source_that_cannot_be_fetched_fails_as_a_source() {
    let rig = Rig::new();
    let picture = PictureId::new();
    rig.view.request_playback(PlaybackRequest::Open {
        picture,
        source: Arc::from("app:///missing.mp4"),
        settings: MediaSettings::default(),
    });
    let mut reported = Vec::new();
    rig.settle("the failure never arrived", || {
        reported.extend(rig.reported());
        !reported.is_empty()
    });
    let [(failed, MediaEvent::Failed(error))] = reported.as_slice() else {
        panic!("one failure: {reported:?}");
    };
    assert_eq!(*failed, picture);
    assert_eq!(error.kind, MediaErrorKind::Source);
    assert!(rig.media.log.lock().unwrap().opens.is_empty());
}

/// An end closes the player and lets go of the picture; so does dropping
/// the view, for every picture it opened.
#[test]
fn an_end_or_a_dropped_view_closes_the_player() {
    let rig = Rig::new();
    let first = rig.open(MediaSettings::default());
    let _second = rig.open(MediaSettings::default());
    rig.view
        .request_playback(PlaybackRequest::End { picture: first });
    assert_eq!(rig.media.log.lock().unwrap().closed, 1);
    assert!(rig.view.take_frame(first).is_none());
    assert_eq!(rig.resources.media.borrow().len(), 1);

    let Rig {
        media,
        resources,
        view,
        ..
    } = rig;
    drop(view);
    assert_eq!(media.log.lock().unwrap().closed, 2);
    assert_eq!(resources.media.borrow().len(), 0);
}

/// Linux has no player: the native platform's every open fails, as the
/// `error` the element fires.
#[cfg(not(target_os = "macos"))]
#[test]
fn the_native_platform_without_a_player_fails_every_open() {
    let resources = Resources::new(
        ResourcesConfig {
            worker_threads: 1,
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || {},
    );
    let (reports, inbox) = ImageInbox::new();
    let view = resources.for_view(reports);
    let picture = PictureId::new();
    view.request_playback(PlaybackRequest::Open {
        picture,
        source: Arc::from("https://example.com/clip.mp4"),
        settings: MediaSettings::default(),
    });
    view.service_images();
    let events = inbox.drain();
    let [
        ImageEvent::Playback {
            picture: failed,
            report: PlaybackReport::Media(MediaEvent::Failed(error)),
        },
    ] = events.as_slice()
    else {
        panic!("one failure: {events:?}");
    };
    assert_eq!(*failed, picture);
    assert_eq!(error.kind, MediaErrorKind::Unsupported);
    assert_eq!(error.message, vant::media::NO_MEDIA_MESSAGE);
}

/// On macOS the native platform plays a registered clip with `AVFoundation`
/// through a view: its first frame is reported with the clip's size, and
/// published into the picture's slot once the picture is drawn.
#[cfg(target_os = "macos")]
#[test]
fn the_native_platform_plays_a_registered_clip() {
    let resources = Resources::new(
        ResourcesConfig {
            worker_threads: 1,
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || {},
    );
    let clip = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/video/rgb.mp4"),
    )
    .expect("the fixture clip");
    resources
        .register(CLIP, clip, Some("video/mp4"))
        .expect("register");
    let (reports, inbox) = ImageInbox::new();
    let view = resources.for_view(reports);
    let picture = PictureId::new();
    let key = PictureKey::Id(picture);
    view.request_playback(PlaybackRequest::Open {
        picture,
        source: Arc::from(CLIP),
        settings: MediaSettings::default(),
    });
    let hint = ImageSizeHint::new(64, 48);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    loop {
        view.service_images();
        events.extend(inbox.drain());
        let _ = view.describe(&key, hint);
        if !events.is_empty() && view.describe(&key, hint).is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "nothing arrived: {events:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        matches!(
            events.as_slice(),
            [ImageEvent::Playback {
                report: PlaybackReport::Media(MediaEvent::FirstFrame {
                    width: 64,
                    height: 48,
                    ..
                }),
                ..
            }]
        ),
        "{events:?}"
    );
    let taken = view.take_frame(picture).expect("the first frame");
    let pixels = taken.frame.into_pixels();
    assert_eq!((pixels.width, pixels.height), (64, 48));
    let _ = Url::parse(CLIP);
}
