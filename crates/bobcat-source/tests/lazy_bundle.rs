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
use bobcat_source::{BundleInstaller, ContainerSources, PageSource};
use url::Url;

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(393.0, 727.0, 1.0);

const ORIGIN: &str = "https://cdn.test";

fn registered(name: &str) -> (bobcat_core::ViewSources, Resources) {
    let fixture = fixtures::fixture(name);
    let input = Url::parse(&format!("{ORIGIN}/{name}.lynx.bundle")).expect("a page URL");
    let page =
        PageSource::from_native_bundle(&input, fixture.page, &format!("{name}__main-thread"))
            .expect("the card decodes");
    let resources = Resources::new(
        ResourcesConfig {
            base_url: Some(input.clone()),
            container_installer: Some(Arc::new(BundleInstaller)),
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
    (page.view_sources(SCREEN, "en-US"), resources)
}

const LAZY_BOX: &[((usize, usize), [u8; 4])] = &[((40, 40), [0, 0, 255, 255])];

async fn boots(name: &str, expected: &str, probes: &[((usize, usize), [u8; 4])]) {
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
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    logged |= message.contains(expected);
                    eprintln!("{name} [{level}] {message}");
                }
                other => panic!("{name}: unexpected runtime event: {other:?}"),
            }
        }
        painter.pump().expect("the frame draws");
        if logged {
            let shot = painter.capture().expect("a capture");
            drawn = probes.iter().all(|&((x, y), colour)| {
                let offset = (y * 393 + x) * 4;
                shot.pixels[offset..offset + 4] == colour
            });
        }
        assert!(
            Instant::now() < deadline,
            "{name}: logged={logged} drawn={drawn}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_compiled_lazy_card_fetches_its_container_and_renders_it() {
    boots("react-lazy", "react-lazy-ready", LAZY_BOX).await;
}

#[tokio::test]
async fn a_sync_lazy_card_waits_out_its_container_on_the_background_thread() {
    boots("react-lazy-sync", "react-lazy-sync-listening", LAZY_BOX).await;
}

#[tokio::test]
async fn a_nested_lazy_card_adopts_the_outer_containers_css() {
    boots(
        "react-lazy-nested",
        "react-lazy-nested-ready",
        &[
            ((90, 90), [255, 165, 0, 255]),
            ((40, 40), [128, 0, 128, 255]),
        ],
    )
    .await;
}

#[test]
fn the_real_lazy_chunk_installs_the_sections_both_realms_ask_for() {
    let fixture = fixtures::fixture("react-lazy");
    let (name, bytes) = fixture.chunks[0];
    let url = Url::parse(&format!("{ORIGIN}/{name}")).expect("a chunk URL");

    let sources = ContainerSources::from_bytes(&url, bytes).expect("the container decodes");

    assert_eq!(
        sources
            .scripts()
            .iter()
            .map(|(url, _)| url.as_str())
            .collect::<Vec<_>>(),
        [
            format!("{ORIGIN}/{name}/background"),
            format!("{ORIGIN}/{name}/background.js"),
            format!("{ORIGIN}/{name}/main-thread"),
            format!("{ORIGIN}/{name}/main-thread.js"),
        ]
    );
    assert_eq!(
        sources
            .style_sheets()
            .iter()
            .map(|(url, _)| url.as_str())
            .collect::<Vec<_>>(),
        [format!("{ORIGIN}/{name}/custom_section/CSS.css")]
    );
    let template = bobcat_source::native::decode(bytes).expect("the container decodes");
    let has_style_info = template.style_info.as_ref().is_some_and(|info| {
        info.css_id_to_style_sheet
            .values()
            .any(|sheet| !sheet.rules.is_empty())
    });
    assert_eq!(
        sources.style_info().map(Url::as_str),
        has_style_info
            .then(|| format!("{ORIGIN}/{name}/style-info.css"))
            .as_deref()
    );
    assert!(sources.entry().is_none());
    assert!(sources.background_entry().is_none());

    let (_, main_thread) = sources
        .scripts()
        .iter()
        .find(|(url, _)| url.as_str().ends_with("/main-thread.js"))
        .expect("the MTS section");
    let body = template.custom_sections.as_ref().unwrap()["main-thread"]["content"]
        .as_str()
        .expect("the MTS section's source");
    assert_eq!(
        main_thread.as_ref(),
        format!("{}export default {body}", bobcat_core::MTS_CHUNK_PREAMBLE)
    );
    assert!(main_thread.contains("export default (function"));
}

#[test]
fn a_fetched_full_card_registers_its_root_as_the_entry_module() {
    let bytes = fixtures::fixture("basic-class-selector").page;
    let url = Url::parse(&format!("{ORIGIN}/card.web.bundle")).expect("a URL");
    let sources = ContainerSources::from_bytes(&url, bytes).expect("the card decodes");
    let template = bobcat_source::web::decode(bytes).expect("the card decodes");

    let root = format!("{ORIGIN}/card.web.bundle/root.js");
    assert_eq!(sources.entry().map(Url::as_str), Some(root.as_str()));
    let (_, registered) = sources
        .scripts()
        .iter()
        .find(|(at, _)| at.as_str() == root)
        .expect("the root is registered");
    assert_eq!(
        registered.as_ref(),
        bobcat_source::mts_entry_source(&template.lepus_code["root"])
    );
    assert!(!registered.contains("export default"));
    assert_eq!(
        sources.style_info().map(Url::as_str),
        Some(format!("{ORIGIN}/card.web.bundle/style-info.css").as_str()),
        "registered, though only a page lists it"
    );
}

#[test]
fn a_fetched_full_card_without_page_css_registers_no_page_sheet() {
    let bytes = fixtures::fixture("basic-bindtap").page;
    let url = Url::parse(&format!("{ORIGIN}/frames/bindtap.web.bundle?rev=1")).expect("a URL");
    let sources = ContainerSources::from_bytes(&url, bytes).expect("the card decodes");

    assert!(sources.style_info().is_none());
    let root = format!("{ORIGIN}/frames/bindtap.web.bundle/root.js?rev=1");
    assert_eq!(sources.entry().map(Url::as_str), Some(root.as_str()));
    let (_, registered) = sources
        .scripts()
        .iter()
        .find(|(at, _)| at.as_str() == root)
        .expect("the root is registered");
    assert!(registered.starts_with(bobcat_core::MTS_CHUNK_PREAMBLE));
}
