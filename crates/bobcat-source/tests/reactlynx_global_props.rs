//! Default React global-props mode: initial environment, change hooks and pixels.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::input::{InputEvent, Point2D, PointerKind, PointerPhase};
use bobcat_core::{
    DrawTarget, EngineError, EngineEvent, LynxGroup, LynxView, NoWakeup, Painter, StyleThreads,
};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use serde_json::json;
use url::Url;

mod support;
use support::{DelayedBackground, PendingSource};

#[tokio::test]
async fn default_global_props_initialize_module_and_state_then_rerender() {
    verify(false, false).await;
}

#[tokio::test]
async fn global_props_wait_for_bts_readiness_before_updates() {
    verify(true, false).await;
}

#[tokio::test]
async fn development_global_props_follow_default_reactive_hooks() {
    verify(false, true).await;
    verify(true, true).await;
}

async fn verify(delayed: bool, development: bool) {
    let mut page = PropsView::new(delayed, development).await;
    assert!(matches!(
        page.view.update_global_props("{}".into()),
        Err(EngineError::NotReady)
    ));
    page.wait(
        [0, 128, 0, 255],
        (!delayed).then_some("props-render green 3 retained 3 3"),
    )
    .await;
    page.released.set(true);
    page.wait([0, 128, 0, 255], Some("props-render green 3 retained 3 3"))
        .await;
    page.update(&json!({"seed":4,"color":"blue"}));
    page.wait([0, 0, 255, 255], Some("props-render blue 4 retained 3 3"))
        .await;
    assert_eq!(page.changes(), ["props-change 4"]);
    page.update(&json!({"color":"orange"}));
    page.wait(
        [255, 165, 0, 255],
        Some("props-render orange 4 retained 3 3"),
    )
    .await;
    assert_eq!(page.changes(), ["props-change 4", "props-change 4"]);
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        page.painter.dispatch_input(InputEvent::pointer(
            Point2D::new(20.0, 20.0),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
    page.wait([128, 0, 128, 255], None).await;
    assert_eq!(page.changes().len(), 2);
    assert_eq!(page.missing_websocket_warnings, usize::from(development));
}

struct PropsView {
    view: LynxView<DelayedBackground>,
    painter: Painter,
    released: Rc<Cell<bool>>,
    messages: Vec<String>,
    development: bool,
    missing_websocket_warnings: usize,
}

impl PropsView {
    async fn new(delayed: bool, development: bool) -> Self {
        let input = Url::parse("app:///react-global-props.lynx.bundle").unwrap();
        let page = PageSource::from_native_bundle(
            &input,
            if development {
                fixtures::fixture("react-global-props-development").page
            } else {
                fixtures::fixture("react-global-props").page
            },
            "react-global-props__main-thread",
        )
        .unwrap();
        let resources = Resources::new(ResourcesConfig::default(), || {});
        page.register_with(&resources);
        resources.set_base_url(Some(input));
        let mut sources = page.view_sources();
        sources.global_props =
            Some(json!({"seed":3,"color":"green","keep":"retained"}).to_string());
        let background = sources.background_entry.clone().unwrap();
        let released = Rc::new(Cell::new(!delayed));
        let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
            .await
            .unwrap();
        let view = group
            .create_lynx_view(
                120.0,
                120.0,
                1.0,
                |reports| DelayedBackground {
                    resources: resources.builder()(reports),
                    background,
                    pending: PendingSource::default(),
                    released: Rc::clone(&released),
                },
                Vec::new(),
                sources,
            )
            .unwrap();
        let mut painter = Painter::new(DrawTarget::Offscreen, 120.0, 120.0, 1.0)
            .await
            .unwrap();
        painter.attach(&view).unwrap();
        Self {
            view,
            painter,
            released,
            messages: Vec::new(),
            development,
            missing_websocket_warnings: 0,
        }
    }

    fn update(&self, props: &serde_json::Value) {
        self.view.update_global_props(props.to_string()).unwrap();
    }

    fn changes(&self) -> Vec<&str> {
        self.messages
            .iter()
            .map(String::as_str)
            .filter(|s| s.starts_with("props-change "))
            .collect()
    }

    async fn wait(&mut self, expected: [u8; 4], marker: Option<&str>) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            for event in self.view.pump() {
                match event {
                    EngineEvent::ScriptFinished => {}
                    EngineEvent::ConsoleMessage { message, .. } => self.messages.push(message),
                    EngineEvent::ScriptReported { level, message }
                        if self.development && level == "warning"
                            && message.contains("WebSocket is not found. Please use Lynx >= 2.16 or consider using a polyfill.") =>
                    {
                        self.missing_websocket_warnings += 1;
                    }
                    other => panic!("global props failed: {other:?}"),
                }
            }
            self.painter.pump().unwrap();
            let pixel = match self.painter.capture() {
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
            if pixel == Some(expected)
                && (marker.is_none() || self.view.is_ready())
                && marker.is_none_or(|marker| self.messages.iter().any(|s| s == marker))
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected {expected:?}, got {pixel:?}; messages={:?}",
                self.messages
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
