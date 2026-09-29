//! Compiled `ReactLynx` entries use the actual source registry and both `QuickJS`
//! threads. A framework error report fails these tests even if boot resolves.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use url::Url;

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(393.0, 727.0, 1.0);

async fn boot(bytes: &[u8], name: &str) {
    let input = Url::parse(&format!("app:///{name}")).unwrap();
    let page = if name == "react-native.lynx.bundle" {
        PageSource::from_native_bundle(&input, bytes, "react-native__main-thread")
    } else {
        PageSource::from_bytes(&input, bytes)
    }
    .expect("decode compiled card");
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    boot_registered(page.view_sources(SCREEN), resources, name).await;
}

async fn boot_registered(sources: bobcat_core::ViewSources, resources: Resources, name: &str) {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(393.0, 727.0, 1.0, resources.builder(), Vec::new(), sources)
        .unwrap();
    // Boot's first flush waits for a painter to bind the view, and this waits
    // for `ScriptFinished` past it.
    let mut painter =
        bobcat_core::Painter::new(bobcat_core::DrawTarget::Offscreen, 393.0, 727.0, 1.0)
            .await
            .unwrap();
    painter.attach(&view).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut finished = false;
    // Continue after boot: hydration and the patch acknowledgement are queued
    // by the framework, so entry evaluation alone is not an error-free run.
    let mut finished_at = None;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {
                    finished = true;
                    finished_at = Some(Instant::now());
                }
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("{name} [{level}] {message}");
                }
                other => panic!("{name}: unexpected runtime event: {other:?}"),
            }
        }
        if finished_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(100)) {
            break;
        }
        assert!(Instant::now() < deadline, "{name}: BTS boot did not finish");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(finished);
}

#[tokio::test]
async fn real_web_cards_boot_without_lynx_core() {
    boot(
        fixtures::fixture("basic-bindtap").page,
        "basic-bindtap.web.bundle",
    )
    .await;
    boot(
        fixtures::fixture("basic-class-selector").page,
        "basic-class-selector.web.bundle",
    )
    .await;
}

#[tokio::test]
async fn real_native_card_boots_without_lynx_core() {
    boot(
        fixtures::fixture("react-native").page,
        "react-native.lynx.bundle",
    )
    .await;
}

#[tokio::test]
async fn real_native_background_runs_from_its_preserved_custom_section() {
    let bytes = fixtures::fixture("react-native").page;
    let page = PageSource::from_native_bundle(
        &Url::parse("app:///custom-section-native.lynx.bundle").unwrap(),
        bytes,
        "react-native__main-thread",
    )
    .unwrap();
    let template = bobcat_source::native::decode(bytes).unwrap();
    let key = template
        .custom_sections
        .as_ref()
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .find(|key| key.contains("background."))
        .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let mut sources = page.view_sources(SCREEN);
    let entry = serde_json::to_string(sources.background_entry.as_ref().unwrap()).unwrap();
    let key = serde_json::to_string(key).unwrap();
    // Keep PageSource's actual section registration and the compiled factory.
    // Select its custom-section loader in place of the app-service launcher.
    let bootstrap = format!(
        r"
        import {{lynx}} from 'bobcat:bts-runtime';
        const original = lynx.requireModule;
        let loaded = false;
        lynx.requireModule = function(path, ...args) {{
            if (path !== '/app-service.js') return original(path, ...args);
            lynx.requireModule = original;
            loaded = true;
            return lynx.loadScript({key}, {{}});
        }};
        await import({entry});
        if (!loaded) throw Error('custom-section bootstrap did not run');
    "
    );
    resources
        .register(
            "app:///custom-bts.js",
            bootstrap.into_bytes(),
            Some("text/javascript"),
        )
        .unwrap();
    sources.background_entry = Some("app:///custom-bts.js".to_owned());
    boot_registered(sources, resources, "native custom-section BTS").await;
}

#[tokio::test]
async fn compiled_bindtap_set_state_changes_the_painted_pixels_twice() {
    paint_and_tap(
        fixtures::fixture("basic-bindtap").page,
        [50, 50],
        &[[255, 192, 203, 255], [0, 128, 0, 255], [255, 192, 203, 255]],
        None,
    )
    .await;
}

#[tokio::test]
async fn compiled_worklets_and_main_thread_refs_cross_both_directions() {
    // The effect runs on BTS, calls a compiled worklet on MTS, and uses its
    // hydrated main-thread ref to enlarge the pink 100px box to green 200px.
    paint_and_tap(
        fixtures::fixture("basic-mts-run-on-main-thread").page,
        [150, 150],
        &[[0, 128, 0, 255]],
        None,
    )
    .await;
    // The native input invokes a compiled MTS worklet; runOnBackground then
    // calls React's real state setter on BTS and the patch paints green.
    paint_and_tap(
        fixtures::fixture("basic-mts-run-on-background").page,
        [50, 50],
        &[[255, 192, 203, 255], [0, 128, 0, 255]],
        None,
    )
    .await;
}

#[tokio::test]
async fn compiled_bts_ref_queries_and_native_props_change_real_pixels() {
    paint_and_tap(
        fixtures::fixture("react-bts-query").page,
        [150, 150],
        &[[0, 128, 0, 255]],
        Some("react-bts-query verified"),
    )
    .await;
}

/// A compiled `<viewpager>` starts on the page its `select-index={1}` names,
/// green, and the tap's
/// selector-query `selectTab({index: 3, smooth: false})` crosses from the
/// background thread and turns it to the fourth, yellow, in the next frame.
#[tokio::test]
async fn compiled_viewpager_select_tab_turns_to_the_fourth_page() {
    paint_and_tap(
        fixtures::fixture("react-viewpager").page,
        [120, 120],
        &[[0, 128, 0, 255], [255, 255, 0, 255]],
        None,
    )
    .await;
}

/// A compiled `<scroll-coordinator>` (300 by 400: a translucent blue toolbar
/// 60 tall, a red header 200 tall, a slot of eight 100px items in a
/// `<scroll-view>`) boots with the header under the toolbar and the first
/// item at 200. A forward drag of 148px in the scroll-view — 140 of scroll
/// after the 8px slop, the fold's whole range — folds the header first: the
/// toolbar then covers the header's bottom band and the first item sits
/// directly under it, at 60..160.
#[tokio::test]
async fn compiled_scroll_coordinator_folds_its_header_before_the_content() {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};

    const HEIGHT: u16 = 400;
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 128, 0, 255];
    const YELLOW: [u8; 4] = [255, 255, 0, 255];

    let page = PageSource::from_bytes(
        &Url::parse("app:///react-scroll-coordinator.web.bundle").unwrap(),
        fixtures::fixture("react-scroll-coordinator").page,
    )
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let (width, height) = (f32::from(COORDINATOR_WIDTH), f32::from(HEIGHT));
    let mut view = group
        .create_lynx_view(
            width,
            height,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();

    let mut booted = false;
    let band = settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[(150, 150, RED), (150, 250, GREEN), (150, 350, YELLOW)],
        "boot",
    );
    eprintln!("toolbar over the header at boot: {band:?}");
    let [red, green, blue, _] = band;
    assert!(
        (126..=129).contains(&red) && green == 0 && (126..=129).contains(&blue),
        "the translucent toolbar shows the red header through it, got {band:?}"
    );

    // One move and a release: a single step measures no release velocity, so
    // nothing flings after it.
    for (phase, y) in [
        (PointerPhase::Down, 350.0),
        (PointerPhase::Move, 202.0),
        (PointerPhase::Up, 202.0),
    ] {
        painter.dispatch_input(InputEvent::pointer(
            Point2D::new(150.0, y),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    let band = settle(
        &mut view,
        &mut painter,
        &mut booted,
        &[(150, 65, GREEN), (150, 150, GREEN), (150, 200, YELLOW)],
        "folded",
    );
    eprintln!("toolbar over the header folded: {band:?}");
    assert!(
        (126..=129).contains(&band[0]) && band[1] == 0 && (126..=129).contains(&band[2]),
        "folded, the toolbar covers the header's bottom band, got {band:?}"
    );
}

/// The width of the coordinator test's view.
const COORDINATOR_WIDTH: u16 = 300;

/// Pumps `view` and `painter` until every `(x, y, colour)` of `expected` is
/// on screen, and answers the colour at (150, 30), the toolbar band.
fn settle<F: bobcat_core::resource::ResourceFetcher + 'static>(
    view: &mut bobcat_core::LynxView<F>,
    painter: &mut bobcat_core::Painter,
    booted: &mut bool,
    expected: &[(u16, u16, [u8; 4])],
    stage: &str,
) -> [u8; 4] {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => *booted = true,
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("[{level}] {message}");
                }
                other => panic!("{stage}: {other:?}"),
            }
        }
        painter.pump().unwrap();
        if *booted {
            let shot = painter.capture().unwrap();
            let pixel = |x: u16, y: u16| {
                let at = (usize::from(y) * usize::from(COORDINATOR_WIDTH) + usize::from(x)) * 4;
                [
                    shot.pixels[at],
                    shot.pixels[at + 1],
                    shot.pixels[at + 2],
                    shot.pixels[at + 3],
                ]
            };
            let seen: Vec<_> = expected.iter().map(|(x, y, _)| pixel(*x, *y)).collect();
            if expected
                .iter()
                .zip(&seen)
                .all(|((_, _, colour), seen)| colour == seen)
            {
                return pixel(150, 30);
            }
            assert!(
                Instant::now() < deadline,
                "{stage}: expected {expected:?}, got {seen:?}"
            );
        } else {
            assert!(Instant::now() < deadline, "BTS boot did not finish");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

async fn paint_and_tap(
    bytes: &[u8],
    point: [u16; 2],
    colors: &[[u8; 4]],
    verification: Option<&str>,
) {
    let page =
        PageSource::from_bytes(&Url::parse("app:///tap.web.bundle").unwrap(), bytes).unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    paint_registered(
        page.view_sources(SCREEN),
        resources,
        point,
        colors,
        verification,
    )
    .await;
}

async fn paint_registered(
    sources: bobcat_core::ViewSources,
    resources: Resources,
    point: [u16; 2],
    colors: &[[u8; 4]],
    verification: Option<&str>,
) {
    use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
    use bobcat_core::{DrawTarget, Painter};
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(240.0, 240.0, 1.0, resources.builder(), Vec::new(), sources)
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, 240.0, 240.0, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();
    let mut booted = false;
    let mut verified = verification.is_none();
    for (round, expected) in colors.iter().enumerate() {
        if round > 0 {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                painter.dispatch_input(InputEvent::pointer(
                    Point2D::new(f32::from(point[0]), f32::from(point[1])),
                    1,
                    PointerKind::Touch,
                    phase,
                ));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for event in view.pump() {
                match event {
                    EngineEvent::ScriptFinished => booted = true,
                    EngineEvent::ConsoleMessage { level, message, .. } => {
                        verified |= verification.is_some_and(|required| message.contains(required));
                        eprintln!("[{level}] {message}");
                    }
                    other => panic!("tap round {round}: {other:?}"),
                }
            }
            painter.pump().unwrap();
            if !booted {
                assert!(Instant::now() < deadline, "BTS boot did not finish");
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
            let shot = painter.capture().unwrap();
            let offset = (usize::from(point[1]) * 240 + usize::from(point[0])) * 4;
            if booted && verified && shot.pixels[offset..offset + 4] == *expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tap round {round}: expected {expected:?}, got {:?}",
                &shot.pixels[offset..offset + 4]
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
