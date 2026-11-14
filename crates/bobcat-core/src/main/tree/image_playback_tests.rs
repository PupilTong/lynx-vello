//! An `image` showing an animated `src`: the picture of its own it presents,
//! the requests it makes of the host, the four UI methods and the three
//! events, against a host that plays each picture with `vant`'s playhead at
//! instants the test names.

use std::sync::Arc;
use std::time::Duration;

use rustc_hash::FxHashMap;
use terminus::link::{
    ImageEvent, Loops, MethodCall, MethodOutcome, NodeId, PictureId, PictureKey, PlayPolicy,
    PlaybackControl, PlaybackReport, PlaybackRequest,
};
use vant::image::animation::{ImageFrames, Playhead, Timeline};

use super::super::image_sources::ImageOutcome;
use super::super::test_support::{child, with_image_sources};
use super::super::{
    ComponentEvent, ComponentEvents, ImageSources, LynxDocument, PageConfig, apply_image_events,
};

const SRC: &str = "app:///spinner.gif";
const OTHER_SRC: &str = "app:///other.gif";
const STILL: &str = "app:///still.png";

const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// What the host reports of a loaded source: animated or still.
fn loaded(source: &str, animated: bool) -> ImageEvent {
    ImageEvent::Loaded {
        source: Arc::from(source),
        width: 40,
        height: 20,
        animated,
    }
}

/// A host that serves the playback requests as the store does, every frame
/// ready: three frames of 100, 200 and 300 ms per picture, a playhead per
/// picture drawn from the moment it plays, advanced at the instants the test
/// names, and each completed loop reported.
#[derive(Default)]
struct Host {
    pictures: FxHashMap<PictureId, Playhead>,
    requests: Vec<PlaybackRequest>,
    now: Duration,
}

impl Host {
    fn timeline() -> Timeline {
        Timeline::new(&ImageFrames::new(
            vec![ms(100), ms(200), ms(300)],
            Loops::Infinite,
        ))
    }

    /// Serves what the entry's epilogue hands over.
    fn serve(&mut self, document: &LynxDocument, sources: &ImageSources) {
        let timeline = Self::timeline();
        for request in sources.take_playback(document) {
            match &request {
                PlaybackRequest::Play {
                    picture, policy, ..
                } => {
                    let mut playhead = Playhead::new(*policy);
                    playhead.set_drawn(true, &timeline, self.now);
                    self.pictures.insert(*picture, playhead);
                }
                PlaybackRequest::Control { picture, control } => {
                    if let Some(playhead) = self.pictures.get_mut(picture) {
                        playhead.control(*control, &timeline, self.now);
                    }
                }
                PlaybackRequest::End { picture } => {
                    self.pictures.remove(picture);
                }
                PlaybackRequest::Open { .. } | PlaybackRequest::Media { .. } => {
                    panic!("an `image` opens no video: {request:?}")
                }
            }
            self.requests.push(request);
        }
    }

    /// Advances every playhead to `until`, frame by frame, and answers the
    /// reports that owes, in order.
    fn run(&mut self, until: Duration) -> Vec<ImageEvent> {
        let timeline = Self::timeline();
        let mut events = Vec::new();
        for (picture, playhead) in &mut self.pictures {
            while let Some(due) = playhead.due(&timeline)
                && due <= until
            {
                let done = playhead.advance(&timeline, due, |_| true).loops_done();
                if done.completed > 0 {
                    events.push(ImageEvent::Playback {
                        picture: *picture,
                        report: PlaybackReport::LoopComplete,
                    });
                }
                if done.finished {
                    events.push(ImageEvent::Playback {
                        picture: *picture,
                        report: PlaybackReport::FinalLoopComplete,
                    });
                }
            }
        }
        self.now = until;
        events
    }

    fn take_requests(&mut self) -> Vec<PlaybackRequest> {
        std::mem::take(&mut self.requests)
    }
}

/// A document with its component events and sources, and a host.
struct Page {
    document: LynxDocument,
    events: ComponentEvents,
    sources: ImageSources,
    host: Host,
}

impl Page {
    fn new() -> Self {
        let (document, events, sources) = with_image_sources(PageConfig::default());
        Self {
            document,
            events,
            sources,
            host: Host::default(),
        }
    }

    fn image(&mut self) -> NodeId {
        child(&mut self.document, "image", "width: 40px; height: 20px")
    }

    /// The host's reports applied, as a turn does.
    fn report(&mut self, events: &[ImageEvent]) {
        let outcomes = apply_image_events(&mut self.document, &self.sources, events);
        self.events.extend_images(outcomes);
    }

    /// The end of the entry: the requests handed to the host, then the
    /// events the entry queued, as the runtime names them at delivery.
    fn settle(&mut self) -> Vec<String> {
        self.host.serve(&self.document, &self.sources);
        self.events
            .take()
            .into_iter()
            .filter_map(|event| match event {
                ComponentEvent::Image(ImageOutcome::StartPlay { node, picture }) => self
                    .sources
                    .autoplays(picture)
                    .then(|| format!("{}:startplay", node_name(node))),
                ComponentEvent::Image(ImageOutcome::Loaded { node, .. }) => {
                    Some(format!("{}:load", node_name(node)))
                }
                ComponentEvent::Image(ImageOutcome::Failed { node }) => {
                    Some(format!("{}:error", node_name(node)))
                }
                ComponentEvent::Image(ImageOutcome::Played { node, name }) => {
                    Some(format!("{}:{name}", node_name(node)))
                }
                _ => None,
            })
            .collect()
    }

    /// Runs the host's playheads to `until` and applies their reports.
    fn run(&mut self, until: Duration) -> Vec<String> {
        let events = self.host.run(until);
        self.report(&events);
        self.settle()
    }

    fn invoke(&mut self, element: NodeId, name: &str) -> MethodOutcome {
        self.document
            .invoke_element_method(element, MethodCall { name, params: "{}" })
    }

    fn picture(&self, element: NodeId) -> Option<PictureId> {
        self.sources.playing(element)
    }
}

/// How the events below name an element.
fn node_name(node: NodeId) -> String {
    format!("{node:?}")
}

/// `name` at `node`, as [`Page::settle`] names it.
fn at(node: NodeId, name: &str) -> String {
    format!("{}:{name}", node_name(node))
}

/// An animated `src` with `loop-count` N: the element presents a picture
/// of its own at the source's natural size, asks the host to play it N
/// times regardless of the file, fires `startplay` before `load`, then
/// `currentloopcomplete` at the end of every loop and `finalloopcomplete`
/// after the last, and nothing more.
#[test]
fn an_animated_src_plays_its_loop_count_and_fires_its_events_in_order() {
    let mut page = Page::new();
    let image = page.image();
    page.document.set_attribute(image, "loop-count", "2");
    page.document.set_attribute(image, "src", SRC);
    assert!(page.settle().is_empty(), "nothing has loaded");
    assert!(page.host.take_requests().is_empty());

    page.report(&[loaded(SRC, true)]);
    let picture = page.picture(image).expect("a picture of its own");
    assert_eq!(page.document.picture(image), Some(&PictureKey::Id(picture)));
    assert_eq!(
        page.settle(),
        [at(image, "startplay"), at(image, "load")],
        "startplay before load"
    );
    assert_eq!(
        page.host.take_requests(),
        [PlaybackRequest::Play {
            picture,
            source: Arc::from(SRC),
            policy: PlayPolicy {
                loops: Loops::from_plays(2),
                paused: false,
            },
        }]
    );

    assert!(page.run(ms(599)).is_empty(), "the first loop is not over");
    assert_eq!(page.run(ms(600)), [at(image, "currentloopcomplete")]);
    assert_eq!(
        page.run(ms(1200)),
        [
            at(image, "currentloopcomplete"),
            at(image, "finalloopcomplete")
        ]
    );
    assert!(page.run(ms(60_000)).is_empty(), "the last frame stays");
}

/// `loop-count` absent, zero, negative or not a number plays forever, and
/// a fraction plays its whole part: the file's own count is never read.
#[test]
fn loop_count_reads_as_native() {
    for (value, loops) in [
        (None, Loops::Infinite),
        (Some("0"), Loops::Infinite),
        (Some("-3"), Loops::Infinite),
        (Some("often"), Loops::Infinite),
        (Some("1"), Loops::from_plays(1)),
        (Some(" 3 "), Loops::from_plays(3)),
        (Some("2.9"), Loops::from_plays(2)),
    ] {
        assert_eq!(
            super::super::image_sources::play_policy(value, None).loops,
            loops,
            "{value:?}"
        );
    }
    for (value, paused) in [(None, false), (Some("true"), false), (Some("false"), true)] {
        assert_eq!(
            super::super::image_sources::play_policy(None, value).paused,
            paused,
            "{value:?}"
        );
    }
}

/// `autoplay` off: the picture is played paused on its first frame, and no
/// `startplay` fires; a resume plays it, and never fires one either.
#[test]
fn autoplay_off_shows_the_first_frame_and_owes_no_startplay() {
    let mut page = Page::new();
    let image = page.image();
    page.document.set_attribute(image, "autoplay", "false");
    page.document.set_attribute(image, "src", SRC);
    page.report(&[loaded(SRC, true)]);
    assert_eq!(page.settle(), [at(image, "load")]);
    let requests = page.host.take_requests();
    let [PlaybackRequest::Play { policy, .. }] = requests.as_slice() else {
        panic!("one play");
    };
    assert!(policy.paused && policy.loops == Loops::Infinite);
    assert!(page.run(ms(60_000)).is_empty(), "it does not play");

    let _ = page.invoke(image, "resumeAnimation");
    assert!(page.settle().is_empty(), "a resume fires no startplay");
    assert_eq!(page.run(ms(60_600)), [at(image, "currentloopcomplete")]);
}

/// Both attributes are read when the `src` loads: a `src` this document has
/// already settled loads inside the write, and an `autoplay` written after
/// it in the same entry still counts; a change after the load does nothing
/// until the next one. A `src` change ends the picture, and the next load
/// plays a new one from the start.
#[test]
fn attributes_are_read_when_the_src_loads_and_a_src_change_starts_over() {
    let mut page = Page::new();
    page.report(&[loaded(SRC, true), loaded(OTHER_SRC, true)]);
    let image = page.image();
    page.document.set_attribute(image, "src", SRC);
    page.document.set_attribute(image, "autoplay", "false");
    assert_eq!(page.settle(), [at(image, "load")], "no startplay");
    let first = page.picture(image).expect("playing");
    assert!(matches!(
        page.host.take_requests().as_slice(),
        [PlaybackRequest::Play { picture, policy, .. }] if *picture == first && policy.paused
    ));

    page.document.set_attribute(image, "autoplay", "true");
    page.document.set_attribute(image, "loop-count", "1");
    assert!(page.settle().is_empty());
    assert!(
        page.host.take_requests().is_empty(),
        "nothing until the next load"
    );

    page.document.set_attribute(image, "src", OTHER_SRC);
    let second = page.picture(image).expect("playing again");
    assert_ne!(first, second, "a picture of its own per load");
    assert_eq!(page.settle(), [at(image, "startplay"), at(image, "load")]);
    assert_eq!(
        page.host.take_requests(),
        [
            PlaybackRequest::End { picture: first },
            PlaybackRequest::Play {
                picture: second,
                source: Arc::from(OTHER_SRC),
                policy: PlayPolicy {
                    loops: Loops::from_plays(1),
                    paused: false,
                },
            },
        ]
    );
    assert_eq!(
        page.run(ms(600)),
        [
            at(image, "currentloopcomplete"),
            at(image, "finalloopcomplete")
        ]
    );
}

/// The four UI methods answer code 0 with native's message whether or not
/// anything plays, and each sends its control to the playing picture; on
/// a still image they send nothing.
#[test]
fn the_four_methods_answer_and_control_the_playing_picture() {
    let mut page = Page::new();
    let image = page.image();
    let still = page.image();
    page.document.set_attribute(image, "src", SRC);
    page.document.set_attribute(still, "src", STILL);
    page.report(&[loaded(SRC, true), loaded(STILL, false)]);
    page.settle();
    let picture = page.picture(image).expect("playing");
    page.host.take_requests();
    let methods = [
        (
            "startAnimate",
            PlaybackControl::Restart,
            "Animation restarted.",
        ),
        (
            "pauseAnimation",
            PlaybackControl::Pause,
            "Animation paused.",
        ),
        (
            "resumeAnimation",
            PlaybackControl::Resume,
            "Animation resumed.",
        ),
        ("stopAnimation", PlaybackControl::Stop, "Animation stopped."),
    ];
    for (name, control, message) in methods {
        let mut record = String::new();
        crate::main::record::write_record_field(&mut record, message);
        for element in [image, still] {
            assert_eq!(
                page.invoke(element, name),
                MethodOutcome::Data(record.clone()),
                "{name}"
            );
        }
        assert!(page.settle().is_empty(), "{name} fires nothing in the call");
        assert_eq!(
            page.host.take_requests(),
            [PlaybackRequest::Control { picture, control }],
            "{name} reaches the playing picture alone"
        );
    }
    assert_eq!(page.invoke(image, "play"), MethodOutcome::NotFound);
}

/// The methods against the host: a stop owes no final loop, a resume
/// after it plays the whole count again, and `startAnimate` from a
/// pause restarts with the loops reset.
#[test]
fn stop_resume_and_start_animate_play_whole_counts_again() {
    let mut page = Page::new();
    let image = page.image();
    page.document.set_attribute(image, "loop-count", "1");
    page.document.set_attribute(image, "src", SRC);
    page.report(&[loaded(SRC, true)]);
    page.settle();

    page.run(ms(350));
    let _ = page.invoke(image, "stopAnimation");
    page.settle();
    assert!(page.run(ms(60_000)).is_empty(), "a stop owes no final loop");
    let _ = page.invoke(image, "resumeAnimation");
    page.settle();
    assert_eq!(
        page.run(ms(60_600)),
        [
            at(image, "currentloopcomplete"),
            at(image, "finalloopcomplete")
        ],
        "after a stop, the whole count from the first frame"
    );

    let _ = page.invoke(image, "resumeAnimation");
    page.settle();
    page.run(ms(60_700));
    let _ = page.invoke(image, "pauseAnimation");
    page.settle();
    assert!(page.run(ms(70_000)).is_empty());
    let _ = page.invoke(image, "startAnimate");
    page.settle();
    assert_eq!(
        page.run(ms(70_600)),
        [
            at(image, "currentloopcomplete"),
            at(image, "finalloopcomplete")
        ],
        "startAnimate from a pause plays the count from the start"
    );
}

/// An animated `placeholder` is a URL picture under the file's count on the
/// shared playhead: nothing is asked of the host and nothing fires; the
/// `src` taking over is what plays a picture of its own.
#[test]
fn an_animated_placeholder_plays_the_shared_url_and_fires_nothing() {
    let mut page = Page::new();
    let image = page.image();
    page.document.set_attribute(image, "placeholder", OTHER_SRC);
    page.document.set_attribute(image, "src", SRC);
    page.report(&[loaded(OTHER_SRC, true)]);
    assert_eq!(
        page.document.picture(image),
        Some(&PictureKey::Url(Arc::from(OTHER_SRC)))
    );
    assert!(page.settle().is_empty());
    assert!(page.host.take_requests().is_empty());
    assert_eq!(page.sources.playing(image), None);
}

/// A still `src` is untouched: a URL picture, a `load`, no request.
#[test]
fn a_still_src_is_unaffected() {
    let mut page = Page::new();
    let image = page.image();
    page.document.set_attribute(image, "src", STILL);
    page.report(&[loaded(STILL, false)]);
    assert_eq!(
        page.document.picture(image),
        Some(&PictureKey::Url(Arc::from(STILL)))
    );
    assert_eq!(page.settle(), [at(image, "load")]);
    assert!(page.host.take_requests().is_empty());
}

/// A freed `image` ends its picture, and a report the host made before it
/// heard is nobody's.
#[test]
fn a_freed_image_ends_its_picture_and_a_late_report_is_dropped() {
    let mut page = Page::new();
    let image = page.image();
    page.document.set_attribute(image, "src", SRC);
    page.report(&[loaded(SRC, true)]);
    page.settle();
    let picture = page.picture(image).expect("playing");
    page.host.take_requests();
    page.document.drop_element(image);
    page.report(&[ImageEvent::Playback {
        picture,
        report: PlaybackReport::LoopComplete,
    }]);
    assert!(page.settle().is_empty());
    assert_eq!(
        page.host.take_requests(),
        [PlaybackRequest::End { picture }]
    );
}
