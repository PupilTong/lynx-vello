//! The real compiled lazy fixtures, over the real resource system: a
//! `ReactLynx` `lazy(() => import(...))` card, its container fetched and
//! installed at runtime by `lynx.fetchBundle`, and its sections evaluated on
//! both threads afterwards.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::{LazyBundleInstaller, PageSource};
use url::Url;

/// Where the fixtures are served from. The compiled card asks for its lazy
/// container by a **rooted** path (webpack's `publicPath` is `/`), so each
/// chunk is registered at this origin's root rather than beside the page.
const ORIGIN: &str = "https://cdn.test";

/// The page, its lazy chunks, and a resource system that installs containers.
fn registered(name: &str) -> (bobcat_core::ViewSources, Resources) {
    let fixture = fixtures::fixture(name);
    let input = Url::parse(&format!("{ORIGIN}/{name}.lynx.bundle")).expect("a page URL");
    // A source-based `.lynx.bundle` names its main-thread module after the
    // entry it was compiled from, so it is selected explicitly.
    let page =
        PageSource::from_native_bundle(&input, fixture.page, &format!("{name}__main-thread"))
            .expect("the card decodes");
    let resources = Resources::new(
        ResourcesConfig {
            base_url: Some(input.clone()),
            // Without this every `lynx.fetchBundle` fails: installing a
            // container is what makes its sections loadable.
            container_installer: Some(Arc::new(LazyBundleInstaller)),
            ..ResourcesConfig::default()
        },
        || {},
    );
    page.register_with(&resources);
    for (chunk, bytes) in fixture.chunks {
        resources
            .register(&format!("{ORIGIN}/{chunk}"), *bytes, None)
            .expect("a chunk URL is a URL");
    }
    (page.view_sources(), resources)
}

/// Boots one fixture and runs it until `expected` has been logged, painting
/// as it goes so that the lazy component's own CSS reaches a frame.
///
/// The `.lazy-box` assertion is the point of the paint: a blue box is the
/// lazy container's `CSS` section having been adopted through
/// `__LoadStyleSheet`, which only happens if the container installed.
async fn boots(name: &str, expected: &str) {
    let (sources, resources) = registered(name);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .expect("the group starts");
    let mut view = group
        .create_lynx_view(393.0, 727.0, 1.0, resources.builder(), Vec::new(), sources)
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 393.0, 727.0, 1.0)
        .await
        .expect("an offscreen target");
    painter.attach(&view).expect("the painter attaches");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut logged = false;
    let mut drawn = false;
    while !logged || !drawn {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {}
                EngineEvent::ConsoleMessage { level, message } => {
                    logged |= message.contains(expected);
                    eprintln!("{name} [{level}] {message}");
                }
                other => panic!("{name}: unexpected runtime event: {other:?}"),
            }
        }
        painter.pump().expect("the frame draws");
        if logged {
            // 40×40 into the page, which is inside the 80px `.lazy-box`.
            let shot = painter.capture().expect("a capture");
            let offset = (40 * 393 + 40) * 4;
            drawn = shot.pixels[offset..offset + 4] == [0, 0, 255, 255];
        }
        assert!(
            Instant::now() < deadline,
            "{name}: logged={logged} drawn={drawn}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The asynchronous shape: BTS fetches the container, its `.then` runs as a
/// reaction of the fetch's Future, and MTS prepares its half through
/// `rLynxPrepareLazyBundleMTS` — whose `lynx.loadScript('main-thread')` and
/// `__LoadStyleSheet('CSS')` both run **inline**, because that second
/// `fetchBundle` is a URL the view's fetcher already holds and its
/// `fetch_probe` says so in the realm's own job.
///
/// That inline answer is the whole reason the probe exists. Without it the
/// prepare returns having loaded nothing, the `callLepusMethod` reply
/// resolves BTS's lazy import, BTS renders and sends `rLynxChange`, and MTS
/// applies a patch naming a snapshot its `main-thread` section has not
/// registered yet — `Snapshot not found`. See `docs/tracking/deviations.md`.
#[tokio::test]
async fn a_compiled_lazy_card_fetches_its_container_and_renders_it() {
    boots("react-lazy", "react-lazy-ready").await;
}

/// The synchronous shape: `lynx.fetchBundle(url, {}).wait(5)` parks the BTS
/// job until the container is installed, and the section loads before the
/// call returns.
#[tokio::test]
async fn a_sync_lazy_card_waits_out_its_container_on_the_background_thread() {
    boots("react-lazy-sync", "react-lazy-sync-listening").await;
}

/// What the real `react-lazy` chunk decodes to, URL by URL: the one rule both
/// realms write, over a container the compiler actually produced.
#[test]
fn the_real_lazy_chunk_installs_the_sections_both_realms_ask_for() {
    let fixture = fixtures::fixture("react-lazy");
    let (name, bytes) = fixture.chunks[0];
    let url = Url::parse(&format!("{ORIGIN}/{name}")).expect("a chunk URL");

    let sources = bobcat_source::lazy_bundle_sources(&url, bytes).expect("the container decodes");

    let mut scripts: Vec<&str> = sources
        .scripts
        .iter()
        .map(|(url, _)| url.as_str())
        .collect();
    scripts.sort_unstable();
    assert_eq!(
        scripts,
        [
            // `lynx.loadScript('background', {bundleName})` on BTS.
            format!("{ORIGIN}/{name}/background.js"),
            // `lynx.loadScript('main-thread', {bundleName})` on MTS.
            format!("{ORIGIN}/{name}/main-thread.js"),
        ]
    );
    assert_eq!(
        sources
            .style_sheets
            .iter()
            .map(|(url, _)| url.as_str())
            .collect::<Vec<_>>(),
        // `__LoadStyleSheet('CSS', bundleName)` on MTS.
        [format!("{ORIGIN}/{name}/index.css")]
    );

    let (_, main_thread) = sources
        .scripts
        .iter()
        .find(|(url, _)| url.as_str().ends_with("/main-thread.js"))
        .expect("the MTS section");
    // The MTS body is an *expression* the card calls, so it is the module's
    // default export, and the whole module is one physical line: a body keeps
    // the line numbering it had in the container.
    assert!(main_thread.starts_with(bobcat_core::MTS_CHUNK_PREAMBLE));
    assert!(main_thread.contains("export default (function"));
}
