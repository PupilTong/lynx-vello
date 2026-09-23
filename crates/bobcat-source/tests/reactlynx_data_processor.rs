//! Compiler-generated processors feed both realms through the String-input facade.
#![cfg(not(target_arch = "wasm32"))]
#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{
    DrawTarget, EngineError, EngineEvent, LynxGroup, LynxView, NoWakeup, Painter, StyleThreads,
};
use bobcat_resources::{Resources, ResourcesConfig, ViewResources};
use bobcat_source::PageSource;
use url::Url;

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(120.0, 120.0, 1.0);

#[derive(Default)]
struct Observed {
    booted: bool,
    messages: Vec<String>,
}

#[tokio::test]
async fn compiled_processors_feed_initial_data_updates_reset_and_reload() {
    for (processor, increment, suffix) in [("", 1, "processed"), ("named", 10, "named")] {
        let input = Url::parse("app:///react-data-processor.lynx.bundle").unwrap();
        let page = PageSource::from_native_bundle(
            &input,
            fixtures::fixture("react-data-processor").page,
            "react-data-processor__main-thread",
        )
        .unwrap();
        let resources = Resources::new(ResourcesConfig::default(), || {});
        page.register_with(&resources);
        let mut sources = page.view_sources(SCREEN);
        sources.initial_processor = processor.to_owned();
        sources.init_data = Some(r#"{"rawSeed":0,"rawColor":"first","rawKeep":"retained"}"#.into());
        let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
            .await
            .unwrap();
        let mut view = group
            .create_lynx_view(120.0, 120.0, 1.0, resources.builder(), Vec::new(), sources)
            .unwrap();
        let mut painter = Painter::new(DrawTarget::Offscreen, 120.0, 120.0, 1.0)
            .await
            .unwrap();
        painter.attach(&view).unwrap();
        let mut seen = Observed::default();
        wait_pixel(
            &mut view,
            &mut painter,
            &mut seen,
            [0, 128, 0, 255],
            Some(&format!(
                "processed-data green {increment} retained-{suffix} {increment} 1"
            )),
        )
        .await;
        view.update_data(
            r#"{"rawColor":"second","rawSeed":2}"#.into(),
            processor.into(),
        )
        .unwrap();
        wait_pixel(
            &mut view,
            &mut painter,
            &mut seen,
            [0, 0, 255, 255],
            Some(&format!(
                "processed-data blue {} retained-{suffix} {increment} 1",
                increment + 2
            )),
        )
        .await;
        view.reset_data(r#"{"rawColor":"third"}"#.into(), processor.into())
            .unwrap();
        wait_pixel(
            &mut view,
            &mut painter,
            &mut seen,
            [128, 0, 128, 255],
            Some(&format!(
                "processed-data purple absent absent {increment} 1"
            )),
        )
        .await;
        view.reload(
            r#"{"rawSeed":6,"rawColor":"first"}"#.into(),
            processor.into(),
        )
        .unwrap();
        wait_pixel(
            &mut view,
            &mut painter,
            &mut seen,
            [0, 128, 0, 255],
            Some(&format!(
                "processed-data green {0} absent {0} 1",
                increment + 6
            )),
        )
        .await;
        let prefix = if processor.is_empty() {
            "processor-run"
        } else {
            "named-processor-run"
        };
        assert_eq!(
            seen.messages
                .iter()
                .filter(|message| message.starts_with(prefix))
                .count(),
            4
        );
    }
}

async fn wait_pixel(
    view: &mut LynxView<ViewResources>,
    painter: &mut Painter,
    observed: &mut Observed,
    expected: [u8; 4],
    marker: Option<&str>,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => {
                    assert!(
                        !observed.booted,
                        "data operations must not reexecute the entry"
                    );
                    observed.booted = true;
                }
                EngineEvent::ConsoleMessage { message, .. } => observed.messages.push(message),
                other => panic!("processor failed: {other:?}"),
            }
        }
        painter.pump().unwrap();
        let pixel = match painter.capture() {
            Ok(shot) => Some(
                <[u8; 4]>::try_from(&shot.pixels[4 * (20 * 120 + 20)..4 * (20 * 120 + 21)])
                    .unwrap(),
            ),
            Err(EngineError::Render(message))
                if message == "no frame has been rendered to capture" =>
            {
                None
            }
            Err(error) => panic!("capture failed: {error}"),
        };
        if view.is_ready()
            && pixel == Some(expected)
            && marker.is_none_or(|marker| observed.messages.iter().any(|message| message == marker))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, got {pixel:?}; boot={}, messages={:?}",
            observed.booted,
            observed.messages
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
