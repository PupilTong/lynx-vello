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
    boot_registered(page.view_sources(), resources, name).await;
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
                EngineEvent::ConsoleMessage { level, message } => {
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
    let mut sources = page.view_sources();
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
    paint_registered(page.view_sources(), resources, point, colors, verification).await;
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
                    EngineEvent::ConsoleMessage { level, message } => {
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
