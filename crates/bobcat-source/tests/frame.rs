//! The real compiled `<frame>` fixtures, over the real resource system: a
//! `ReactLynx` host card whose `<frame src>` names another compiled
//! `.web.bundle`, fetched as a container and installed by
//! `BundleInstaller`, whose `root` then renders inside the frame from a
//! `ShadowRealm` child of the host's main-thread realm.
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

/// Where the host card is served from. Its frame's `src` is
/// `./react-frame-inner.web.bundle`, which resolves beside it.
const PAGE: &str = "app:///cards/react-frame.web.bundle";

/// Where `src` resolves to, against [`PAGE`].
const INNER: &str = "app:///cards/react-frame-inner.web.bundle";

/// What the inner card's component body logs, from the frame's realm.
const RENDERED: &str = "react-frame-inner-rendered";

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// The host page and a resource system that installs containers; with
/// `inner`, the inner card is registered where the frame's `src` resolves.
fn registered(inner: bool) -> (bobcat_core::ViewSources, Resources) {
    let input = Url::parse(PAGE).expect("a page URL");
    let page = PageSource::from_bytes(&input, fixtures::fixture("react-frame").page)
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

/// What a run saw: the diagnostics it reported, and the probed pixels of
/// the capture it stopped on.
struct Run {
    reported: Vec<String>,
    probes: Vec<[u8; 4]>,
}

/// Boots the host card and pumps it, painting as it goes, until boot
/// finished, `ready` holds of the events so far, and every `(x, y, colour)`
/// of `expected` is on screen. Console output, `ScriptFinished` and
/// diagnostics are collected; any other runtime event fails the test.
async fn run(
    inner: bool,
    ready: impl Fn(&[String], &[String]) -> bool,
    expected: &[(u16, u16, [u8; 4])],
) -> Run {
    let (sources, resources) = registered(inner);
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
                    eprintln!("react-frame [{level}] {message}");
                    logged.push(message);
                }
                EngineEvent::ScriptReported { message, .. } => {
                    eprintln!("react-frame reported: {message}");
                    reported.push(message);
                }
                other => panic!("react-frame: unexpected runtime event: {other:?}"),
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
                            other => panic!("react-frame: unexpected runtime event: {other:?}"),
                        }
                    }
                    painter.pump().expect("the frame draws");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                return Run { reported, probes };
            }
            assert!(
                Instant::now() < deadline,
                "react-frame: saw {probes:?} for {expected:?}; logged {logged:?}, reported {reported:?}"
            );
        }
        assert!(
            Instant::now() < deadline,
            "react-frame: finished={finished}; logged {logged:?}, reported {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The frame sits at the red page's top-left, 100px square; the inner
/// card's blue 100px view fills it once its main thread rendered in the
/// frame's realm. Its `frame:ready` text sits in the view's top-left corner,
/// clear of the centre probe.
#[tokio::test]
async fn a_compiled_frame_renders_the_card_its_src_names() {
    let run = run(
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

/// A `src` nobody serves is one diagnostic naming it, and the host page
/// renders on: red where the frame would have shown its card too, since an
/// empty frame paints nothing.
#[tokio::test]
async fn a_frame_whose_src_does_not_fetch_is_one_diagnostic() {
    let run = run(
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
