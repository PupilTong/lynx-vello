//! The real compiled `<frame>` fixtures, over the real resource system: a
//! `ReactLynx` host card whose `<frame src>` names another compiled
//! `.web.bundle`, fetched as a container and installed by
//! `BundleInstaller`, whose `root` then renders inside the frame from a
//! `ShadowRealm` child of the host's main-thread realm, and whose background
//! thread runs in a Worker of that child.
//!
//! Every host card embeds the same card, `react-frame-inner`: `react-frame`
//! a plain 100px frame, `react-frame-data` one given `data` and
//! `global-props`, `react-frame-auto-height` a 300px-wide `auto-height`
//! frame with no height, and `react-frame-data-update` and
//! `react-frame-global-props-update` a 100px frame whose `data` or
//! `global-props` the host's state writes, above a 100px yellow square whose
//! tap changes that state.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
use bobcat_core::{DrawTarget, EngineEvent, LynxGroup, LynxView, NoWakeup, Painter, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::{BundleInstaller, PageSource};
use url::Url;

const WIDTH: u16 = 393;
const HEIGHT: u16 = 852;

/// The screen these tests' views report.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(393.0, 852.0, 1.0);

/// Where every host card's frame `src`, `./react-frame-inner.web.bundle`,
/// resolves to, against [`host_url`].
const INNER: &str = "app:///cards/react-frame-inner.web.bundle";

/// What the inner card's component body logs first, on each of its threads.
const RENDERED: &str = "react-frame-inner-rendered";

/// What the inner card's component body logs next: its init data's `label`
/// and its `lynx.__globalProps.message`, as a template literal writes them.
const DATA: &str = "react-frame-inner-data:";
const GLOBAL: &str = "react-frame-inner-global:";

/// What the inner card's mount effect logs, once its background thread
/// rendered: a main thread runs no effects.
const INNER_HYDRATED: &str = "react-frame-inner-hydrated";

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

/// Where the host card `host` is served from: beside [`INNER`], so its
/// frame's relative `src` names it.
fn host_url(host: &str) -> Url {
    Url::parse(&format!("app:///cards/{host}.web.bundle")).expect("a page URL")
}

/// The host card `host` and a resource system that installs containers;
/// with `inner`, the inner card is registered where the frame's `src`
/// resolves.
fn registered(host: &str, inner: bool) -> (bobcat_core::ViewSources, Resources) {
    let input = host_url(host);
    let page = PageSource::from_bytes(&input, fixtures::fixture(host).page)
        .expect("the host card decodes");
    let resources = Resources::new(
        ResourcesConfig {
            base_url: Some(input.clone()),
            // Installing the fetched container is what registers the inner
            // card's `root`, page CSS and background-thread boot where the
            // frame's boot loads them.
            container_installer: Some(Arc::new(BundleInstaller)),
            ..ResourcesConfig::default()
        },
        || {},
    );
    page.register_with(&resources);
    if inner {
        resources
            .register(INNER, fixtures::fixture("react-frame-inner").page, None)
            .expect("the inner card's URL is a URL");
    }
    (page.view_sources(SCREEN), resources)
}

/// One pixel of a capture, as RGBA.
fn pixel(pixels: &[u8], x: u16, y: u16) -> [u8; 4] {
    let at = (usize::from(y) * usize::from(WIDTH) + usize::from(x)) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

/// How many console messages of `logged` are exactly `message`.
fn count(logged: &[String], message: &str) -> usize {
    logged.iter().filter(|seen| *seen == message).count()
}

/// The inner card's `DATA` and `GLOBAL` lines of `logged`, sorted: the
/// card's component body runs once on each of its threads — the main
/// thread's first screen and the background thread's hydration — and the two
/// threads' console output has no order between them.
fn page_data_lines(logged: &[String]) -> Vec<&str> {
    let mut lines: Vec<&str> = logged
        .iter()
        .map(String::as_str)
        .filter(|message| message.starts_with(DATA) || message.starts_with(GLOBAL))
        .collect();
    lines.sort_unstable();
    lines
}

/// One booted host card, kept alive across [`Host::settle`]s so a test can
/// act on it in between: the view, the offscreen painter drawing it, and
/// what the view reported so far — its console output and its diagnostics
/// — in the order the pumps saw them.
///
/// Fields drop in order: the painter lets go of the view first, as the
/// locals of a boot function would.
struct Host {
    name: &'static str,
    painter: Painter,
    view: LynxView<ViewResources>,
    _group: LynxGroup,
    _resources: Resources,
    finished: bool,
    logged: Vec<String>,
    reported: Vec<String>,
}

impl Host {
    /// Boots the host card `name` in a group of its own, with the inner card
    /// registered when `inner`, and attaches an offscreen painter.
    async fn boot(name: &'static str, inner: bool) -> Self {
        let (sources, resources) = registered(name, inner);
        let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
            .await
            .expect("the group starts");
        let (width, height) = (f32::from(WIDTH), f32::from(HEIGHT));
        let view = group
            .create_lynx_view(width, height, 1.0, resources.builder(), Vec::new(), sources)
            .expect("the view is built");
        let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
            .await
            .expect("an offscreen target");
        painter.attach(&view).expect("the painter attaches");
        Self {
            name,
            painter,
            view,
            _group: group,
            _resources: resources,
            finished: false,
            logged: Vec::new(),
            reported: Vec::new(),
        }
    }

    /// One turn: every event the view has, then a draw. Console output and
    /// diagnostics are collected, and the one `ScriptFinished` noted; any
    /// other runtime event, a second `ScriptFinished` included, fails the
    /// test.
    fn turn(&mut self) {
        let name = self.name;
        for event in self.view.pump() {
            match event {
                EngineEvent::ScriptFinished if !self.finished => self.finished = true,
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("{name} [{level}] {message}");
                    self.logged.push(message);
                }
                EngineEvent::ScriptReported { message, .. } => {
                    eprintln!("{name} reported: {message}");
                    self.reported.push(message);
                }
                other => panic!("{name}: unexpected runtime event: {other:?}"),
            }
        }
        self.painter.pump().expect("the frame draws");
    }

    /// Pumps the view, painting as it goes, until boot finished, `ready`
    /// holds of the console output and diagnostics so far, and every
    /// `(x, y, colour)` of `expected` is on screen; answers the probed
    /// pixels of that capture. A few more turns follow, for an event that
    /// should not come, which a test's assertions then see.
    async fn settle(
        &mut self,
        ready: impl Fn(&[String], &[String]) -> bool,
        expected: &[(u16, u16, [u8; 4])],
    ) -> Vec<[u8; 4]> {
        let name = self.name;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.turn();
            if self.finished && ready(&self.logged, &self.reported) {
                let shot = self.painter.capture().expect("a capture");
                let probes: Vec<_> = expected
                    .iter()
                    .map(|&(x, y, _)| pixel(&shot.pixels, x, y))
                    .collect();
                if expected
                    .iter()
                    .zip(&probes)
                    .all(|(&(_, _, colour), seen)| colour == *seen)
                {
                    for _ in 0..20 {
                        self.turn();
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    return probes;
                }
                assert!(
                    Instant::now() < deadline,
                    "{name}: saw {probes:?} for {expected:?}; logged {:?}, reported {:?}",
                    self.logged,
                    self.reported
                );
            }
            assert!(
                Instant::now() < deadline,
                "{name}: finished={}; logged {:?}, reported {:?}",
                self.finished,
                self.logged,
                self.reported
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// A one-finger tap at `(x, y)`: a touch going down and coming up there.
    fn tap(&mut self, x: f32, y: f32) {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            self.painter.dispatch_input(InputEvent::pointer(
                Point2D::new(x, y),
                1,
                PointerKind::Touch,
                phase,
            ));
        }
    }
}

/// What a run saw: the console output and the diagnostics it reported, and
/// the probed pixels of the capture it stopped on.
struct Run {
    logged: Vec<String>,
    reported: Vec<String>,
    probes: Vec<[u8; 4]>,
}

/// Boots the host card `host` and settles it once on `ready` and `expected`
/// ([`Host::settle`]).
async fn run(
    host: &'static str,
    inner: bool,
    ready: impl Fn(&[String], &[String]) -> bool,
    expected: &[(u16, u16, [u8; 4])],
) -> Run {
    let mut host = Host::boot(host, inner).await;
    let probes = host.settle(ready, expected).await;
    Run {
        logged: std::mem::take(&mut host.logged),
        reported: std::mem::take(&mut host.reported),
        probes,
    }
}

/// The frame sits at the red page's top-left, 100px square; the inner
/// card's blue 100px view fills it once its main thread rendered in the
/// frame's realm. Its `frame:ready` text sits in the view's top-left corner,
/// clear of the centre probe; the card's two text rows below the view fall
/// outside the frame, which clips them.
#[tokio::test]
async fn a_compiled_frame_renders_the_card_its_src_names() {
    let run = run(
        "react-frame",
        true,
        |logged, _| logged.iter().any(|message| message.contains(RENDERED)),
        &[
            (50, 50, BLUE),
            (150, 150, RED),
            (99, 99, BLUE),
            (100, 50, RED),
        ],
    )
    .await;
    assert_eq!(
        run.probes,
        [BLUE, RED, BLUE, RED],
        "inside the frame, then outside it"
    );
    assert_eq!(run.reported, Vec::<String>::new());
}

/// The frame's `data` is its card's init data and its `global-props` the
/// card's `lynx.__globalProps`, from the first screen: the inner card logs
/// what `useInitData().label` and `lynx.__globalProps.message` read as it
/// renders, once on its main thread and once on its background thread,
/// which renders again to hydrate (web-core-e2e's `api-frame-data` and
/// `api-frame-global-props`, which read the same two values off the
/// rendered texts). Each run waits for all four lines before it compares
/// them, since the two threads log in no order between them.
///
/// The same card in `react-frame`'s frame, given neither, boots with `{}`
/// for both, which `bobcat:frame` hands a frame's card when nothing was
/// written: each key reads `undefined`, and the template literal logs
/// `undefined`.
#[tokio::test]
async fn a_compiled_frame_hands_its_card_data_and_global_props() {
    let four = |logged: &[String], _: &[String]| page_data_lines(logged).len() >= 4;
    let both = run(
        "react-frame-data",
        true,
        four,
        &[(50, 50, BLUE), (150, 150, RED)],
    )
    .await;
    assert_eq!(
        page_data_lines(&both.logged),
        [
            "react-frame-inner-data:from-data",
            "react-frame-inner-data:from-data",
            "react-frame-inner-global:from-global-props",
            "react-frame-inner-global:from-global-props"
        ],
        "one render on each thread, both with both values: {:?}",
        both.logged
    );
    assert_eq!(both.reported, Vec::<String>::new());

    let none = run("react-frame", true, four, &[(50, 50, BLUE)]).await;
    assert_eq!(
        page_data_lines(&none.logged),
        [
            "react-frame-inner-data:undefined",
            "react-frame-inner-data:undefined",
            "react-frame-inner-global:undefined",
            "react-frame-inner-global:undefined"
        ],
        "{:?}",
        none.logged
    );
    assert_eq!(none.reported, Vec::<String>::new());
}

/// An `auto-height` frame with no height takes its card's content's height
/// (web-core-e2e's `api-frame-auto-height`). The frame is 300px wide at the
/// red page's top-left, with a green background; the inner card is its blue
/// 100px view and two 20px text rows, 140px in all. So the frame shows blue
/// in the view, green beside the view and down to its last row, 139, and
/// red from row 140 down and right of 300px. The text rows' glyphs are at
/// the left, clear of the probes at x = 200.
///
/// Without `auto-height`, `contain: strict` would make the frame 0 tall in
/// its column, and every probe would be red.
#[tokio::test]
async fn a_compiled_auto_height_frame_takes_its_cards_height() {
    let expected = [
        (50, 50, BLUE),
        (200, 50, GREEN),
        (200, 139, GREEN),
        (200, 140, RED),
        (350, 50, RED),
    ];
    let run = run(
        "react-frame-auto-height",
        true,
        |logged, _| logged.iter().any(|message| message.contains(RENDERED)),
        &expected,
    )
    .await;
    assert_eq!(
        run.probes,
        expected.map(|(_, _, colour)| colour),
        "the card's view, the frame beside and under it, then below and beside the frame"
    );
    assert_eq!(run.reported, Vec::<String>::new());
}

/// The frame, (0, 0) to (100, 100), shows the inner card's 100px view,
/// blue until the value under test reads `after` and green from then on.
const FRAME_PROBE: (u16, u16) = (50, 50);

/// The host's yellow square right below the frame, (0, 100) to
/// (100, 200), whose tap sets the frame's `data` or `global-props` to
/// `after`; its `update` text sits in its top-left corner, clear of this
/// centre.
const BUTTON_PROBE: (u16, u16) = (50, 150);

/// Boots `host`, a frame of the inner card above a yellow square whose tap
/// writes `after` where the frame's `before` was, and checks the frame's
/// card re-renders after first screen with what the tap wrote, `line` being
/// the inner card's console line for the value under test without its
/// value. Before the tap: the inner card's background thread hydrated (its
/// mount effect logged), the host's did too (`host_hydrated`, so the tap has
/// a handler to reach), and the card shows blue with `before`. After it: the
/// card logged `after`, and shows green.
///
/// The card booted once: its mount effect logged once, its `before` line
/// came twice — the main thread's first screen and the background thread's
/// hydration — and its `after` line once, from the background thread's
/// re-render alone, since a `ReactLynx` main thread re-renders nothing once
/// its first screen is out. A frame that booted its card again for the new
/// value would have logged the mount effect twice and `after` from a second
/// first screen as well.
async fn re_renders_after_the_tap(host: &'static str, host_hydrated: &str, line: &str) {
    let before = format!("{line}before");
    let after = format!("{line}after");
    let mut host = Host::boot(host, true).await;
    host.settle(
        |logged, _| {
            count(logged, INNER_HYDRATED) > 0
                && count(logged, host_hydrated) > 0
                && count(logged, &before) > 0
        },
        &[
            (FRAME_PROBE.0, FRAME_PROBE.1, BLUE),
            (BUTTON_PROBE.0, BUTTON_PROBE.1, YELLOW),
            (150, 50, RED),
        ],
    )
    .await;
    assert_eq!(
        count(&host.logged, &after),
        0,
        "nothing reads `after` before the tap: {:?}",
        host.logged
    );

    host.tap(f32::from(BUTTON_PROBE.0), f32::from(BUTTON_PROBE.1));
    host.settle(
        |logged, _| count(logged, &after) > 0,
        &[
            (FRAME_PROBE.0, FRAME_PROBE.1, GREEN),
            (BUTTON_PROBE.0, BUTTON_PROBE.1, YELLOW),
        ],
    )
    .await;

    assert_eq!(host.reported, Vec::<String>::new());
    let logged = &host.logged;
    assert_eq!(count(logged, INNER_HYDRATED), 1, "one boot: {logged:?}");
    assert_eq!(count(logged, &before), 2, "one per thread: {logged:?}");
    assert_eq!(count(logged, &after), 1, "the BTS re-render: {logged:?}");
}

/// web-core-e2e's `api-frame-data-update`: the host's state writes the
/// frame's `data={{ label }}`, and a tap on `#update-frame-data` sets
/// `label` from `before` to `after`. The host's background thread
/// re-renders, its main thread writes the frame's new `data`, the frame
/// hands it to its card as an update, and the card's own background thread
/// takes it as `updateCardData` and re-renders `data:after`.
#[tokio::test]
async fn a_compiled_frames_card_re_renders_when_its_data_changes() {
    re_renders_after_the_tap(
        "react-frame-data-update",
        "react-frame-data-update-hydrated",
        DATA,
    )
    .await;
}

/// `react-frame-data-update` for `global-props`: a tap on
/// `#update-frame-global-props` sets the frame's
/// `global-props={{ message }}` from `before` to `after`, and the card's
/// background thread takes the merged global props as `updateGlobalProps`,
/// which re-renders it in `ReactLynx`'s default `reactive` global props mode.
#[tokio::test]
async fn a_compiled_frames_card_re_renders_when_its_global_props_change() {
    re_renders_after_the_tap(
        "react-frame-global-props-update",
        "react-frame-global-props-update-hydrated",
        GLOBAL,
    )
    .await;
}

/// A `src` nobody serves is one diagnostic naming it, and the host page
/// renders on: red where the frame would have shown its card too, since an
/// empty frame paints nothing.
#[tokio::test]
async fn a_frame_whose_src_does_not_fetch_is_one_diagnostic() {
    let run = run(
        "react-frame",
        false,
        |_, reported| !reported.is_empty(),
        &[(50, 50, RED), (150, 150, RED)],
    )
    .await;
    assert_eq!(run.reported.len(), 1, "{:?}", run.reported);
    // `bobcat:frame` names `src` as the card wrote it; the fetcher's own
    // error follows (no transport serves `app:` URLs).
    assert!(
        run.reported[0].contains("frame ./react-frame-inner.web.bundle: "),
        "the diagnostic names src: {:?}",
        run.reported
    );
}
