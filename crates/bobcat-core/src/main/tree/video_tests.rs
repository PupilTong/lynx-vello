//! A `video`: the picture it presents, the requests it makes of the host,
//! its two UI methods and its three events, against a test media host that
//! plays each opened picture as a clip of a fixed length at instants the
//! test names, as a platform player reports one.

#![allow(
    clippy::float_cmp,
    reason = "every size here is a reported pixel count"
)]

use std::sync::Arc;
use std::time::Duration;

use rustc_hash::FxHashMap;
use terminus::link::stylo::computed_values::object_fit::T as ObjectFit;
use terminus::link::{
    ImageEvent, MediaCommand, MediaError, MediaErrorKind, MediaEvent, MediaSettings, MethodCall,
    MethodError, MethodOutcome, NodeId, PictureId, PictureKey, PlaybackReport, PlaybackRequest,
};

use super::super::test_support::{child, style_of, with_videos};
use super::super::{ComponentDetail, ComponentEvent, ComponentEvents, Field, LynxDocument};
use super::{Videos, apply_reports};

const SRC: &str = "app:///clip.mp4";
const OTHER_SRC: &str = "app:///other.mp4";
/// The test clip: 2.5 seconds of 64 by 48 pixels.
const LENGTH: Duration = Duration::from_millis(2500);
const NATURAL: (u32, u32) = (64, 48);

/// One opened picture as the test host plays it.
#[derive(Debug)]
struct Player {
    settings: MediaSettings,
    playing: bool,
    /// Where in the clip it is.
    at: Duration,
    ended: bool,
}

/// A host that serves the requests as a platform player would: an open
/// reports its first frame at once, paused; `Play` plays from where it is,
/// from the start after the end; time passes only when the test says; the
/// end is reported unless the player loops, when it starts over.
#[derive(Default)]
struct Host {
    players: FxHashMap<PictureId, Player>,
    requests: Vec<PlaybackRequest>,
    /// What the open of each source answers: a failure when named here.
    failing: FxHashMap<&'static str, MediaError>,
    /// Reports not delivered yet.
    reports: Vec<ImageEvent>,
}

impl Host {
    fn report(&mut self, picture: PictureId, event: MediaEvent) {
        self.reports.push(ImageEvent::Playback {
            picture,
            report: PlaybackReport::Media(event),
        });
    }

    /// Serves what the entry's epilogue hands over.
    fn serve(&mut self, document: &LynxDocument, videos: &Videos) {
        for request in videos.take_requests(document) {
            match &request {
                PlaybackRequest::Open {
                    picture,
                    source,
                    settings,
                } => {
                    if let Some(error) = self.failing.get(&**source).cloned() {
                        self.report(*picture, MediaEvent::Failed(error));
                    } else {
                        self.players.insert(
                            *picture,
                            Player {
                                settings: *settings,
                                playing: false,
                                at: Duration::ZERO,
                                ended: false,
                            },
                        );
                        self.report(
                            *picture,
                            MediaEvent::FirstFrame {
                                width: NATURAL.0,
                                height: NATURAL.1,
                                duration: LENGTH,
                            },
                        );
                    }
                }
                PlaybackRequest::Media { picture, command } => {
                    if let Some(player) = self.players.get_mut(picture) {
                        match command {
                            MediaCommand::Play => {
                                if player.ended {
                                    player.at = Duration::ZERO;
                                    player.ended = false;
                                }
                                player.playing = true;
                            }
                            MediaCommand::Pause => player.playing = false,
                            MediaCommand::Loop(looping) => player.settings.looping = *looping,
                            MediaCommand::Muted(muted) => player.settings.muted = *muted,
                        }
                    }
                }
                PlaybackRequest::End { picture } => {
                    self.players.remove(picture);
                }
                PlaybackRequest::Play { .. } | PlaybackRequest::Control { .. } => {
                    panic!("a `video` plays no image: {request:?}")
                }
            }
            self.requests.push(request);
        }
    }

    /// Lets `elapsed` pass for every playing player.
    fn run(&mut self, elapsed: Duration) {
        let mut ended = Vec::new();
        for (picture, player) in &mut self.players {
            if !player.playing {
                continue;
            }
            player.at += elapsed;
            while player.at >= LENGTH {
                if player.settings.looping {
                    player.at -= LENGTH;
                } else {
                    player.at = LENGTH;
                    player.playing = false;
                    player.ended = true;
                    ended.push(*picture);
                    break;
                }
            }
        }
        for picture in ended {
            self.report(picture, MediaEvent::Ended);
        }
    }

    fn take_requests(&mut self) -> Vec<PlaybackRequest> {
        std::mem::take(&mut self.requests)
    }
}

/// A document with its component events and videos, and a host.
struct Page {
    document: LynxDocument,
    events: ComponentEvents,
    videos: Videos,
    host: Host,
}

impl Page {
    fn new() -> Self {
        let (document, events, videos) = with_videos();
        Self {
            document,
            events,
            videos,
            host: Host::default(),
        }
    }

    /// An unsized, absolutely positioned video: its box is its natural size
    /// once it has one.
    fn video(&mut self) -> NodeId {
        child(&mut self.document, "video", "position: absolute")
    }

    /// The end of an entry: the requests handed to the host, and what it
    /// reported back applied, as a turn does.
    fn settle(&mut self) {
        self.host.serve(&self.document, &self.videos);
        let reports = std::mem::take(&mut self.host.reports);
        let owed = apply_reports(&mut self.document, &self.videos, &reports);
        self.events.extend(owed);
    }

    fn invoke(&mut self, element: NodeId, name: &str) -> MethodOutcome {
        self.document
            .invoke_element_method(element, MethodCall { name, params: "{}" })
    }

    /// The events queued since the last call, as `name detail`.
    fn events(&self) -> Vec<String> {
        self.events
            .take()
            .into_iter()
            .map(|event| match event {
                ComponentEvent::Plain { name, .. } => format!("{name} {{}}"),
                ComponentEvent::Detailed {
                    name,
                    detail: ComponentDetail::Fields(fields),
                    ..
                } => {
                    let fields: Vec<String> = fields
                        .iter()
                        .map(|(key, field)| match field {
                            Field::Number(number) => format!("{key}={number}"),
                            Field::Bool(flag) => format!("{key}={flag}"),
                        })
                        .collect();
                    format!("{name} {}", fields.join(","))
                }
                ComponentEvent::Detailed {
                    name,
                    detail: ComponentDetail::Error { code, message },
                    ..
                } => format!("{name} {code} {message}"),
                other => panic!("not a video's event: {other:?}"),
            })
            .collect()
    }

    fn size(&mut self, element: NodeId) -> (f32, f32) {
        self.document.layout();
        let layout = self.document.rounded_layout(element).expect("laid out");
        (layout.size.width, layout.size.height)
    }
}

/// `src` presents a picture of the element's own from the write, with no
/// natural size, and opens it under the `loop` and `muted` the element has
/// when the entry ends, written before or after `src`. The first frame
/// sizes the box and fires `firstframe` with the duration in seconds.
#[test]
fn src_opens_a_picture_under_the_attributes_the_entry_leaves() {
    let mut page = Page::new();
    let video = page.video();
    page.document.set_attribute(video, "src", SRC);
    page.document.set_attribute(video, "loop", "true");
    let picture = page.videos.picture(video).expect("a picture");
    assert_eq!(page.document.picture(video), Some(&PictureKey::Id(picture)));
    assert_eq!(
        page.size(video),
        (0.0, 0.0),
        "no natural size before the first frame"
    );

    page.settle();
    assert_eq!(
        page.host.take_requests(),
        [PlaybackRequest::Open {
            picture,
            source: Arc::from(SRC),
            settings: MediaSettings {
                looping: true,
                muted: false,
            },
        }]
    );
    assert_eq!(page.events(), ["firstframe duration=2.5"]);
    assert_eq!(page.size(video), (64.0, 48.0), "the video's natural size");

    // A change after the open is a command to the player.
    page.document.set_attribute(video, "muted", "true");
    page.document.set_attribute(video, "loop", "false");
    page.settle();
    assert_eq!(
        page.host.take_requests(),
        [
            PlaybackRequest::Media {
                picture,
                command: MediaCommand::Muted(true),
            },
            PlaybackRequest::Media {
                picture,
                command: MediaCommand::Loop(false),
            },
        ]
    );
    assert!(page.host.players[&picture].settings.muted);
}

/// `loop` and `muted` are booleans as `__SetAttribute` writes one: on when
/// present, off when absent or the `"false"` a JavaScript `false` becomes.
#[test]
fn loop_and_muted_read_as_booleans() {
    for (value, on) in [
        (Some("true"), true),
        (Some(""), true),
        (Some("false"), false),
        (None, false),
    ] {
        let mut page = Page::new();
        let video = page.video();
        if let Some(value) = value {
            page.document.set_attribute(video, "muted", value);
        }
        page.document.set_attribute(video, "src", SRC);
        page.settle();
        let requests = page.host.take_requests();
        let [PlaybackRequest::Open { settings, .. }] = requests.as_slice() else {
            panic!("one open: {requests:?}");
        };
        assert_eq!(settings.muted, on, "muted={value:?}");
    }
}

/// Nothing plays until `play`: an open is paused on its first frame, as on
/// both platforms. `play` and `pause` each send their command and answer
/// code 0 with `{success: true}` (the record's one field), playing or not.
#[test]
fn play_and_pause_send_their_commands_and_answer_success() {
    let mut page = Page::new();
    let video = page.video();
    page.document.set_attribute(video, "src", SRC);
    page.settle();
    let _ = (page.host.take_requests(), page.events());
    let picture = page.videos.picture(video).expect("a picture");
    assert!(
        !page.host.players[&picture].playing,
        "an open does not play"
    );

    for (name, command) in [
        ("play", MediaCommand::Play),
        ("pause", MediaCommand::Pause),
        ("pause", MediaCommand::Pause),
    ] {
        assert_eq!(
            page.invoke(video, name),
            MethodOutcome::Data("1:1".to_owned()),
            "{name}"
        );
        page.settle();
        assert_eq!(
            page.host.take_requests(),
            [PlaybackRequest::Media { picture, command }]
        );
    }
    assert!(page.events().is_empty(), "no event is fired for either");
}

/// `play` with no `src` answers 8 and fires `error` with iOS's `-1` and
/// both platforms' message; `pause` with none answers success.
#[test]
fn play_with_no_source_fails_and_fires_an_error() {
    let mut page = Page::new();
    let video = page.video();
    assert_eq!(
        page.invoke(video, "play"),
        MethodOutcome::Failed(MethodError::Operation)
    );
    assert_eq!(page.events(), ["error -1 missing video source"]);
    assert_eq!(
        page.invoke(video, "pause"),
        MethodOutcome::Data("1:1".to_owned())
    );
    page.settle();
    assert!(page.host.take_requests().is_empty());
}

/// `ended` fires once when playback reaches the end of a video that does not
/// loop, and never for one that does; a `play` after the end plays again
/// from the start.
#[test]
fn ended_fires_at_the_end_unless_the_video_loops() {
    let mut page = Page::new();
    let once = page.video();
    let looping = page.video();
    page.document.set_attribute(once, "src", SRC);
    page.document.set_attribute(looping, "src", OTHER_SRC);
    page.document.set_attribute(looping, "loop", "true");
    page.settle();
    let _ = page.events();
    for video in [once, looping] {
        let _ = page.invoke(video, "play");
    }
    page.settle();

    page.host.run(Duration::from_secs(6));
    page.settle();
    assert_eq!(page.events(), ["ended {}"]);
    let ended = page.events.take();
    assert!(ended.is_empty());

    let _ = page.invoke(once, "play");
    page.settle();
    page.host.run(Duration::from_secs(3));
    page.settle();
    let queued = page.events.take();
    assert!(
        matches!(queued.as_slice(), [ComponentEvent::Plain { node, name: "ended" }] if *node == once),
        "the second play reached the end again: {queued:?}"
    );
}

/// A player's failure is the element's `error`, with iOS's codes: `-1` for a
/// source that could not be opened, `-2` for anything else, a platform with
/// no player included. Nothing else is fired for it.
#[test]
fn a_failure_fires_error_with_its_code_and_message() {
    let mut page = Page::new();
    page.host.failing.insert(
        SRC,
        MediaError::new(MediaErrorKind::Source, "the file is not there"),
    );
    page.host.failing.insert(
        OTHER_SRC,
        MediaError::new(MediaErrorKind::Unsupported, vant::media::NO_MEDIA_MESSAGE),
    );
    let missing = page.video();
    let unplayable = page.video();
    page.document.set_attribute(missing, "src", SRC);
    page.document.set_attribute(unplayable, "src", OTHER_SRC);
    page.settle();
    assert_eq!(
        page.events(),
        [
            "error -1 the file is not there".to_owned(),
            format!("error -2 {}", vant::media::NO_MEDIA_MESSAGE),
        ]
    );
    assert_eq!(page.size(missing), (0.0, 0.0), "nothing sized it");
}

/// A new `src` ends the picture before and opens a new one, whose first
/// frame sizes the box again; what the old one reports afterwards is
/// dropped. Removing `src` ends the picture and presents nothing.
#[test]
fn a_src_change_ends_the_picture_and_opens_another() {
    let mut page = Page::new();
    let video = page.video();
    page.document.set_attribute(video, "src", SRC);
    page.settle();
    let first = page.videos.picture(video).expect("a picture");
    let _ = (page.host.take_requests(), page.events());

    page.document.set_attribute(video, "src", SRC);
    page.settle();
    assert!(
        page.host.take_requests().is_empty(),
        "the same value opens nothing"
    );

    page.document.set_attribute(video, "src", OTHER_SRC);
    let second = page.videos.picture(video).expect("another picture");
    assert_ne!(first, second);
    assert_eq!(page.document.picture(video), Some(&PictureKey::Id(second)));
    page.host.report(first, MediaEvent::Ended);
    page.settle();
    let requests = page.host.take_requests();
    assert!(
        matches!(
            requests.as_slice(),
            [
                PlaybackRequest::End { picture },
                PlaybackRequest::Open { picture: opened, source, .. },
            ] if *picture == first && *opened == second && &**source == OTHER_SRC
        ),
        "{requests:?}"
    );
    assert_eq!(
        page.events(),
        ["firstframe duration=2.5"],
        "the old picture's end is dropped"
    );

    page.document.remove_attribute(video, "src");
    assert_eq!(page.document.picture(video), None);
    page.settle();
    assert_eq!(
        page.host.take_requests(),
        [PlaybackRequest::End { picture: second }]
    );
}

/// An element freed while it plays ends its picture, and what its player
/// reports afterwards owes nothing.
#[test]
fn a_freed_video_ends_its_picture() {
    let mut page = Page::new();
    let video = page.video();
    page.document.set_attribute(video, "src", SRC);
    page.settle();
    let picture = page.videos.picture(video).expect("a picture");
    let _ = page.invoke(video, "play");
    page.settle();
    let _ = (page.host.take_requests(), page.events());

    page.document.drop_element(video);
    page.host.report(picture, MediaEvent::Ended);
    page.settle();
    assert_eq!(
        page.host.take_requests(),
        [PlaybackRequest::End { picture }]
    );
    assert!(page.events().is_empty());
    assert!(page.host.players.is_empty());
}

/// What is not built: `stop` and `seek` answer as methods the element does
/// not have, and `volume`, `speed`, `mode` and `timeupdate-interval` reach
/// nothing.
#[test]
fn the_reduced_surface_answers_nothing_else() {
    let mut page = Page::new();
    let video = page.video();
    page.document.set_attribute(video, "src", SRC);
    page.settle();
    let _ = (page.host.take_requests(), page.events());
    for name in ["stop", "seek"] {
        assert_eq!(page.invoke(video, name), MethodOutcome::NotFound, "{name}");
    }
    for (name, value) in [
        ("volume", "0.5"),
        ("speed", "2"),
        ("mode", "direct"),
        ("timeupdate-interval", "1"),
    ] {
        page.document.set_attribute(video, name, value);
    }
    page.settle();
    assert!(page.host.take_requests().is_empty());
}

/// `object-fit` reaches the engine as a UA attribute rule: `contain`,
/// native's default, for the bare tag and anything unrecognised; `cover`
/// and `fill` as written.
#[test]
fn object_fit_selects_the_css_property() {
    let mut page = Page::new();
    let video = child(&mut page.document, "video", "width: 100px; height: 60px");
    for (value, fit) in [
        (None, ObjectFit::Contain),
        (Some("cover"), ObjectFit::Cover),
        (Some("fill"), ObjectFit::Fill),
        (Some("contain"), ObjectFit::Contain),
        (Some("none"), ObjectFit::Contain),
        (Some("Cover"), ObjectFit::Contain),
    ] {
        match value {
            Some(value) => page.document.set_attribute(video, "object-fit", value),
            None => page.document.remove_attribute(video, "object-fit"),
        }
        page.document.layout();
        assert_eq!(
            *style_of(&page.document, video).get_object_fit(),
            fit,
            "object-fit={value:?}"
        );
    }
}

/// A `video` is sized like any replaced element: a CSS size wins, and one
/// axis derives the other through the video's ratio. Its children make no
/// box.
#[test]
fn a_video_is_sized_like_a_replaced_element() {
    let mut page = Page::new();
    let sized = child(
        &mut page.document,
        "video",
        "position: absolute; width: 128px",
    );
    let inner = super::super::test_support::element_under(&mut page.document, sized, "view", "");
    page.document.set_attribute(sized, "src", SRC);
    page.settle();
    assert_eq!(page.size(sized), (128.0, 96.0));
    assert_eq!(
        super::super::test_support::display(&page.document, inner),
        terminus::link::stylo::values::computed::Display::None
    );
}
