//! The real compiled `<frame>` fixtures, over the real resource system: a
//! `ReactLynx` host card whose `<frame src>` names another compiled
//! `.web.bundle`, fetched as a container and installed by
//! `BundleInstaller`, whose `root` then renders inside the frame from a
//! `ShadowRealm` child of the host's main-thread realm.
//!
//! Every host card embeds the same card, `react-frame-inner`: `react-frame`
//! a plain 100px frame, `react-frame-data` one given `data` and
//! `global-props`, and `react-frame-auto-height` a 300px-wide
//! `auto-height` frame with no height.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
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

/// What the inner card's component body logs first, from the frame's realm.
const RENDERED: &str = "react-frame-inner-rendered";

/// What the inner card's component body logs next: its init data's `label`
/// and its `lynx.__globalProps.message`, as a template literal writes them.
const DATA: &str = "react-frame-inner-data:";
const GLOBAL: &str = "react-frame-inner-global:";

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

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
            // card's `root` and page CSS where the frame's boot loads them.
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

/// What a run saw: the console output and the diagnostics it reported, and
/// the probed pixels of the capture it stopped on.
struct Run {
    logged: Vec<String>,
    reported: Vec<String>,
    probes: Vec<[u8; 4]>,
}

/// Whether some console message of `logged` is exactly `message`.
fn logged_exactly(logged: &[String], message: &str) -> bool {
    logged.iter().any(|seen| seen == message)
}

/// The inner card's `DATA` and `GLOBAL` lines of `logged`, in order.
fn page_data_lines(logged: &[String]) -> Vec<&str> {
    logged
        .iter()
        .map(String::as_str)
        .filter(|message| message.starts_with(DATA) || message.starts_with(GLOBAL))
        .collect()
}

/// Boots the host card `host` and pumps it, painting as it goes, until boot
/// finished, `ready` holds of the events so far, and every `(x, y, colour)`
/// of `expected` is on screen. Console output, `ScriptFinished` and
/// diagnostics are collected; any other runtime event fails the test.
async fn run(
    host: &str,
    inner: bool,
    ready: impl Fn(&[String], &[String]) -> bool,
    expected: &[(u16, u16, [u8; 4])],
) -> Run {
    let (sources, resources) = registered(host, inner);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .expect("the group starts");
    let (width, height) = (f32::from(WIDTH), f32::from(HEIGHT));
    let mut view = group
        .create_lynx_view(width, height, 1.0, resources.builder(), Vec::new(), sources)
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
        .await
        .expect("an offscreen target");
    painter.attach(&view).expect("the painter attaches");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut finished = false;
    let mut logged = Vec::new();
    let mut reported = Vec::new();
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => finished = true,
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("{host} [{level}] {message}");
                    logged.push(message);
                }
                EngineEvent::ScriptReported { message, .. } => {
                    eprintln!("{host} reported: {message}");
                    reported.push(message);
                }
                other => panic!("{host}: unexpected runtime event: {other:?}"),
            }
        }
        painter.pump().expect("the frame draws");
        if finished && ready(&logged, &reported) {
            let shot = painter.capture().expect("a capture");
            let probes: Vec<_> = expected
                .iter()
                .map(|&(x, y, _)| pixel(&shot.pixels, x, y))
                .collect();
            if expected
                .iter()
                .zip(&probes)
                .all(|(&(_, _, colour), seen)| colour == *seen)
            {
                // A few more turns, for an event that should not come.
                for _ in 0..20 {
                    for event in view.pump() {
                        match event {
                            EngineEvent::ConsoleMessage { message, .. } => logged.push(message),
                            EngineEvent::ScriptReported { message, .. } => reported.push(message),
                            other => panic!("{host}: unexpected runtime event: {other:?}"),
                        }
                    }
                    painter.pump().expect("the frame draws");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                return Run {
                    logged,
                    reported,
                    probes,
                };
            }
            assert!(
                Instant::now() < deadline,
                "{host}: saw {probes:?} for {expected:?}; logged {logged:?}, reported {reported:?}"
            );
        }
        assert!(
            Instant::now() < deadline,
            "{host}: finished={finished}; logged {logged:?}, reported {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
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
/// card's `lynx.__globalProps`, from the first screen: the inner card's main
/// thread logs what `useInitData().label` and `lynx.__globalProps.message`
/// read as it renders (web-core-e2e's `api-frame-data` and
/// `api-frame-global-props`, which read the same two values off the
/// rendered texts).
///
/// The same card in `react-frame`'s frame, given neither, boots with `{}`
/// for both, which `bobcat:frame` hands a frame's card when nothing was
/// written: each key reads `undefined`, and the template literal logs
/// `undefined`.
#[tokio::test]
async fn a_compiled_frame_hands_its_card_data_and_global_props() {
    let given = |logged: &[String], _: &[String]| {
        logged_exactly(logged, "react-frame-inner-data:from-data")
            && logged_exactly(logged, "react-frame-inner-global:from-global-props")
    };
    let both = run(
        "react-frame-data",
        true,
        given,
        &[(50, 50, BLUE), (150, 150, RED)],
    )
    .await;
    assert_eq!(
        page_data_lines(&both.logged),
        [
            "react-frame-inner-data:from-data",
            "react-frame-inner-global:from-global-props"
        ],
        "one first screen, with both values: {:?}",
        both.logged
    );
    assert_eq!(both.reported, Vec::<String>::new());

    let neither = |logged: &[String], _: &[String]| {
        logged_exactly(logged, "react-frame-inner-data:undefined")
            && logged_exactly(logged, "react-frame-inner-global:undefined")
    };
    let none = run("react-frame", true, neither, &[(50, 50, BLUE)]).await;
    assert_eq!(
        page_data_lines(&none.logged),
        [
            "react-frame-inner-data:undefined",
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
