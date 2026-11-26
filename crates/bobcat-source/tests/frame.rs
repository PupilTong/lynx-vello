//! The real compiled `<frame>` fixtures, over the real resource system: a
//! `ReactLynx` host card whose `<frame src>` names another compiled
//! `.web.bundle`, fetched as a container and installed by
//! `BundleInstaller`, whose `root` then renders inside the frame from a
//! `ShadowRealm` child of the host's main-thread realm, and whose background
//! thread runs in a Worker of that child.
//!
//! Most host cards embed the same card, `react-frame-inner`: `react-frame`
//! a plain 100px frame, `react-frame-data` one given `data` and
//! `global-props`, `react-frame-auto-height` a 300px-wide `auto-height`
//! frame with no height, and `react-frame-data-update` and
//! `react-frame-global-props-update` a 100px frame whose `data` or
//! `global-props` the host's state writes, above a 100px yellow square whose
//! tap changes that state. `react-frame-tap` embeds `react-frame-tap-inner`,
//! whose views take a tap in the card's own threads, inside a wrapper whose
//! own tap handler logs.
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

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(393.0, 852.0, 1.0);

const INNER_CARD: &str = "react-frame-inner";

const RENDERED: &str = "react-frame-inner-rendered";

const DATA: &str = "react-frame-inner-data:";
const GLOBAL: &str = "react-frame-inner-global:";

const INNER_HYDRATED: &str = "react-frame-inner-hydrated";

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

fn card_url(card: &str) -> Url {
    Url::parse(&format!("app:///cards/{card}.web.bundle")).expect("a page URL")
}

fn registered(host: &str, inner: Option<&str>) -> (bobcat_core::ViewSources, Resources) {
    let input = card_url(host);
    let page = PageSource::from_bytes(&input, fixtures::fixture(host).page)
        .expect("the host card decodes");
    let resources = Resources::new(
        ResourcesConfig {
            base_url: Some(input.clone()),
            container_installer: Some(Arc::new(BundleInstaller)),
            ..ResourcesConfig::default()
        },
        || {},
    );
    page.register_with(&resources);
    if let Some(inner) = inner {
        resources
            .register(
                card_url(inner).as_str(),
                fixtures::fixture(inner).page,
                None,
            )
            .expect("the inner card's URL is a URL");
    }
    (page.view_sources(SCREEN, "en-US"), resources)
}

fn pixel(pixels: &[u8], x: u16, y: u16) -> [u8; 4] {
    let at = (usize::from(y) * usize::from(WIDTH) + usize::from(x)) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

fn count(logged: &[String], message: &str) -> usize {
    logged.iter().filter(|seen| *seen == message).count()
}

fn page_data_lines(logged: &[String]) -> Vec<&str> {
    let mut lines: Vec<&str> = logged
        .iter()
        .map(String::as_str)
        .filter(|message| message.starts_with(DATA) || message.starts_with(GLOBAL))
        .collect();
    lines.sort_unstable();
    lines
}

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
    async fn boot(name: &'static str, inner: Option<&'static str>) -> Self {
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

struct Run {
    logged: Vec<String>,
    reported: Vec<String>,
    probes: Vec<[u8; 4]>,
}

async fn run(
    host: &'static str,
    inner: Option<&'static str>,
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

#[tokio::test]
async fn a_compiled_frame_renders_the_card_its_src_names() {
    let run = run(
        "react-frame",
        Some(INNER_CARD),
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

#[tokio::test]
async fn a_compiled_frame_hands_its_card_data_and_global_props() {
    let four = |logged: &[String], _: &[String]| page_data_lines(logged).len() >= 4;
    let both = run(
        "react-frame-data",
        Some(INNER_CARD),
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

    let none = run("react-frame", Some(INNER_CARD), four, &[(50, 50, BLUE)]).await;
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
        Some(INNER_CARD),
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

const FRAME_PROBE: (u16, u16) = (50, 50);

const BUTTON_PROBE: (u16, u16) = (50, 150);

async fn re_renders_after_the_tap(host: &'static str, host_hydrated: &str, line: &str) {
    let before = format!("{line}before");
    let after = format!("{line}after");
    let mut host = Host::boot(host, Some(INNER_CARD)).await;
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

#[tokio::test]
async fn a_compiled_frames_card_re_renders_when_its_data_changes() {
    re_renders_after_the_tap(
        "react-frame-data-update",
        "react-frame-data-update-hydrated",
        DATA,
    )
    .await;
}

#[tokio::test]
async fn a_compiled_frames_card_re_renders_when_its_global_props_change() {
    re_renders_after_the_tap(
        "react-frame-global-props-update",
        "react-frame-global-props-update-hydrated",
        GLOBAL,
    )
    .await;
}

#[tokio::test]
async fn a_frame_whose_src_does_not_fetch_is_one_diagnostic() {
    let run = run(
        "react-frame",
        None,
        |_, reported| !reported.is_empty(),
        &[(50, 50, RED), (150, 150, RED)],
    )
    .await;
    assert_eq!(run.reported.len(), 1, "{:?}", run.reported);
    assert!(
        run.reported[0].contains("frame ./react-frame-inner.web.bundle: "),
        "the diagnostic names src: {:?}",
        run.reported
    );
}

const TAP_CARD: &str = "react-frame-tap-inner";

const TAP_HOST_HYDRATED: &str = "react-frame-tap-hydrated";

const TAP_CARD_HYDRATED: &str = "react-frame-tap-inner-hydrated";

const TAP_COUNT: &str = "react-frame-tap-inner-count:";

const TAP_HOST_LINE: &str = "react-frame-tap-host:";

const TAP_CARD_LINE: &str = "react-frame-tap-inner-at:";

const TAP_CARD_MAIN_THREAD_LINE: &str = "react-frame-tap-inner-main-thread-at:";

const SPACER_PROBE: (u16, u16) = (50, 50);

const TAP_ME_PROBE: (u16, u16) = (50, 150);

const TAP_ME_ON_MAIN_THREAD_PROBE: (u16, u16) = (50, 250);

const BESIDE_THE_FRAME_PROBE: (u16, u16) = (150, 150);

fn count_starting(logged: &[String], prefix: &str) -> usize {
    logged
        .iter()
        .filter(|seen| seen.starts_with(prefix))
        .count()
}

async fn boot_tap_host() -> Host {
    let mut host = Host::boot("react-frame-tap", Some(TAP_CARD)).await;
    host.settle(
        |logged, _| count(logged, TAP_HOST_HYDRATED) > 0 && count(logged, TAP_CARD_HYDRATED) > 0,
        &[
            (SPACER_PROBE.0, SPACER_PROBE.1, YELLOW),
            (TAP_ME_PROBE.0, TAP_ME_PROBE.1, BLUE),
            (
                TAP_ME_ON_MAIN_THREAD_PROBE.0,
                TAP_ME_ON_MAIN_THREAD_PROBE.1,
                MAGENTA,
            ),
            (BESIDE_THE_FRAME_PROBE.0, BESIDE_THE_FRAME_PROBE.1, RED),
        ],
    )
    .await;
    let logged = &host.logged;
    assert_eq!(
        count_starting(logged, TAP_HOST_LINE) + count_starting(logged, TAP_CARD_LINE),
        0,
        "no tap handler ran before the tap: {logged:?}"
    );
    host
}

#[tokio::test]
async fn a_tap_inside_a_frame_runs_the_cards_handler_and_the_pages() {
    let host_line = format!("{TAP_HOST_LINE}target:150");
    let card_line = format!("{TAP_CARD_LINE}50,50");
    let counted = format!("{TAP_COUNT}1");
    let mut host = boot_tap_host().await;

    host.tap(f32::from(TAP_ME_PROBE.0), f32::from(TAP_ME_PROBE.1));
    host.settle(
        |logged, _| count(logged, &counted) > 0 && count(logged, &host_line) > 0,
        &[
            (TAP_ME_PROBE.0, TAP_ME_PROBE.1, GREEN),
            (
                TAP_ME_ON_MAIN_THREAD_PROBE.0,
                TAP_ME_ON_MAIN_THREAD_PROBE.1,
                MAGENTA,
            ),
            (SPACER_PROBE.0, SPACER_PROBE.1, YELLOW),
        ],
    )
    .await;

    assert_eq!(host.reported, Vec::<String>::new());
    let logged = &host.logged;
    assert_eq!(
        count(logged, &host_line),
        1,
        "the page's handler: {logged:?}"
    );
    assert_eq!(
        count_starting(logged, TAP_HOST_LINE),
        1,
        "nothing else reached the page's handler: {logged:?}"
    );
    assert_eq!(
        count(logged, &card_line),
        1,
        "the card's handler: {logged:?}"
    );
    assert_eq!(
        count_starting(logged, TAP_CARD_LINE),
        1,
        "nothing else reached the card's handler: {logged:?}"
    );
    assert_eq!(
        count_starting(logged, TAP_CARD_MAIN_THREAD_LINE),
        0,
        "the other view's worklet did not run: {logged:?}"
    );
    assert_eq!(count(logged, TAP_HOST_HYDRATED), 1, "{logged:?}");
    assert_eq!(count(logged, TAP_CARD_HYDRATED), 1, "one boot: {logged:?}");
    assert_eq!(
        count(logged, &format!("{TAP_COUNT}0")),
        2,
        "one per thread: {logged:?}"
    );
    assert_eq!(count(logged, &counted), 1, "the BTS re-render: {logged:?}");
}

#[tokio::test]
async fn a_tap_inside_a_frame_runs_the_cards_main_thread_handler_and_the_pages() {
    let host_line = format!("{TAP_HOST_LINE}target:250");
    let card_line = format!("{TAP_CARD_MAIN_THREAD_LINE}50,150");
    let mut host = boot_tap_host().await;

    host.tap(
        f32::from(TAP_ME_ON_MAIN_THREAD_PROBE.0),
        f32::from(TAP_ME_ON_MAIN_THREAD_PROBE.1),
    );
    host.settle(
        |logged, _| count(logged, &card_line) > 0 && count(logged, &host_line) > 0,
        &[
            (
                TAP_ME_ON_MAIN_THREAD_PROBE.0,
                TAP_ME_ON_MAIN_THREAD_PROBE.1,
                GREEN,
            ),
            (TAP_ME_PROBE.0, TAP_ME_PROBE.1, BLUE),
        ],
    )
    .await;

    assert_eq!(host.reported, Vec::<String>::new());
    let logged = &host.logged;
    assert_eq!(
        count(logged, &host_line),
        1,
        "the page's handler: {logged:?}"
    );
    assert_eq!(
        count_starting(logged, TAP_HOST_LINE),
        1,
        "nothing else reached the page's handler: {logged:?}"
    );
    assert_eq!(
        count(logged, &card_line),
        1,
        "the card's worklet: {logged:?}"
    );
    assert_eq!(
        count_starting(logged, TAP_CARD_MAIN_THREAD_LINE),
        1,
        "nothing else reached the card's worklet: {logged:?}"
    );
    assert_eq!(
        count_starting(logged, TAP_CARD_LINE),
        0,
        "the other view's handler did not run: {logged:?}"
    );
    assert_eq!(count(logged, TAP_CARD_HYDRATED), 1, "one boot: {logged:?}");
    assert_eq!(
        count(logged, &format!("{TAP_COUNT}1")),
        0,
        "the card's state did not change: {logged:?}"
    );
}
