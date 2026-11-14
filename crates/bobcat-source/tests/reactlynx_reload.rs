//! Reload compiled React after readiness without evaluating its entry again.

// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;
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

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(120.0, 120.0, 1.0);

/// How the page is encoded: every script a string section, or as the
/// encoder writes it for production, main-thread bytecode with the
/// background as source or as bytecode too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Source,
    Bytecode,
    JsBytecode,
}

const BYTECODE_FORMS: [Form; 2] = [Form::Bytecode, Form::JsBytecode];

#[tokio::test]
async fn native_reload_recreates_state_and_effects_without_reexecuting_the_entry() {
    verify_reload(false, false, Form::Source).await;
}

#[tokio::test]
async fn native_reload_after_a_delayed_background_starts() {
    verify_reload(true, false, Form::Source).await;
}

#[tokio::test]
async fn development_build_boots_updates_and_reloads_with_default_framework_branches() {
    verify_reload(false, true, Form::Source).await;
    verify_reload(true, true, Form::Source).await;
}

#[tokio::test]
async fn bytecode_reload_recreates_state_and_effects_without_reexecuting_the_entry() {
    for form in BYTECODE_FORMS {
        verify_reload(false, false, form).await;
        verify_reload(true, false, form).await;
    }
}

/// Both bytecode forms of the development page are refused: `ReactLynx`'s
/// development main-thread runtime calls `eval` directly.
#[test]
fn development_bytecode_pages_are_refused() {
    let fixture = bytecode::fixture("react-reload-development");
    let input = Url::parse("app:///react-reload.lynx.bundle").unwrap();
    for bytes in [fixture.page, fixture.js_bytecode_page] {
        let error = PageSource::from_bytes(&input, bytes)
            .expect_err("a development page is refused")
            .to_string();
        assert!(error.contains("ROOT_LEPUS"), "{error}");
        assert!(error.contains("eval"), "{error}");
    }
}

#[tokio::test]
async fn bts_reload_keeps_its_callback_alive_across_component_cleanup_and_remount() {
    bts_reload_keeps_its_callback(Form::Source).await;
}

#[tokio::test]
async fn bytecode_bts_reload_keeps_its_callback_alive() {
    for form in BYTECODE_FORMS {
        bts_reload_keeps_its_callback(form).await;
    }
}

async fn bts_reload_keeps_its_callback(form: Form) {
    let ReloadView {
        mut view,
        mut painter,
        ..
    } = prepare_view(false, false, form).await;
    let mut observed = Observed::default();
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [0, 128, 0, 255],
        Some("reload-mount 0 retained 1"),
    )
    .await;
    tap(&mut painter);
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [0, 0, 255, 255],
        None,
    )
    .await;
    view.send_global_event("reload-from-js", json!([{ "seed":2 }]).to_string())
        .unwrap();
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [128, 0, 128, 255],
        Some("reload-callback 0 2 retained 1"),
    )
    .await;
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [128, 0, 128, 255],
        Some("reload-mount 2 retained 1"),
    )
    .await;
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message == "reload-cleanup 0")
    );
    assert_eq!(
        observed
            .messages
            .iter()
            .filter(|message| message.starts_with("reload-callback"))
            .count(),
        1
    );
    observed.messages.clear();
    tap(&mut painter);
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [255, 165, 0, 255],
        None,
    )
    .await;
    view.send_global_event("reload-from-js", "[{}]".into())
        .unwrap();
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [128, 0, 128, 255],
        Some("reload-callback 2 2 retained 1"),
    )
    .await;
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [128, 0, 128, 255],
        Some("reload-mount 2 retained 1"),
    )
    .await;
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message == "reload-cleanup 2")
    );
    assert_eq!(
        observed
            .messages
            .iter()
            .filter(|message| message.starts_with("reload-callback"))
            .count(),
        1
    );
}

#[derive(Default)]
struct Observed {
    booted: bool,
    messages: Vec<String>,
    expect_missing_websocket: bool,
    missing_websocket_warnings: usize,
}

struct ReloadView {
    view: LynxView<DelayedBackground>,
    painter: Painter,
    released: Rc<Cell<bool>>,
}

async fn prepare_view(before_background: bool, development: bool, form: Form) -> ReloadView {
    let input = Url::parse("app:///react-reload.lynx.bundle").unwrap();
    let bytecode = bytecode::fixture("react-reload");
    let page = match form {
        Form::Source => PageSource::from_native_bundle(
            &input,
            if development {
                fixtures::fixture("react-reload-development").page
            } else {
                fixtures::fixture("react-reload").page
            },
            "react-reload__main-thread",
        ),
        Form::Bytecode => PageSource::from_bytes(&input, bytecode.page),
        Form::JsBytecode => PageSource::from_bytes(&input, bytecode.js_bytecode_page),
    }
    .unwrap();
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);
    resources.set_base_url(Some(input));
    let mut sources = page.view_sources(SCREEN, "en-US");
    sources.init_data = Some(json!({"seed":0,"keep":"retained"}).to_string());
    let background = sources.background_entry.clone().unwrap();
    let released = Rc::new(Cell::new(!before_background));
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
    ReloadView {
        view,
        painter,
        released,
    }
}

async fn verify_reload(before_background: bool, development: bool, form: Form) {
    let ReloadView {
        mut view,
        mut painter,
        released,
    } = prepare_view(before_background, development, form).await;
    let mut observed = Observed {
        expect_missing_websocket: development,
        ..Observed::default()
    };
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [0, 128, 0, 255],
        (!before_background).then_some("reload-mount 0 retained 1"),
    )
    .await;
    if before_background {
        // MTS boot alone makes the view ready, so the reload is accepted and
        // waits in the BTS Worker's queue behind its still-loading entry.
        assert!(view.is_ready());
        view.reload("{}".into(), String::new()).unwrap();
        released.set(true);
        wait_pixel(
            &mut view,
            &mut painter,
            &mut observed,
            [0, 128, 0, 255],
            Some("reload-mount 0 retained 1"),
        )
        .await;
    }
    tap(&mut painter);
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [0, 0, 255, 255],
        None,
    )
    .await;
    view.update_data(r#"{"keep":"updated"}"#.into(), String::new())
        .unwrap();
    view.reload(r#"{"seed":2}"#.into(), String::new()).unwrap();
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [128, 0, 128, 255],
        Some("reload-mount 2 updated 1"),
    )
    .await;
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message == "reload-cleanup 0")
    );
    tap(&mut painter);
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [255, 165, 0, 255],
        None,
    )
    .await;
    observed.messages.clear();
    view.reload("{}".into(), String::new()).unwrap();
    wait_pixel(
        &mut view,
        &mut painter,
        &mut observed,
        [128, 0, 128, 255],
        Some("reload-mount 2 updated 1"),
    )
    .await;
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message == "reload-cleanup 2")
    );
    assert_eq!(
        observed.missing_websocket_warnings,
        usize::from(development)
    );
}

fn tap(painter: &mut Painter) {
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        painter.dispatch_input(InputEvent::pointer(
            Point2D::new(20.0, 20.0),
            1,
            PointerKind::Touch,
            phase,
        ));
    }
}

async fn wait_pixel(
    view: &mut LynxView<DelayedBackground>,
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
                    assert!(!observed.booted, "reload must not reboot the realm");
                    observed.booted = true;
                }
                EngineEvent::ConsoleMessage { message, .. } => observed.messages.push(message),
                EngineEvent::ScriptReported { level, message, .. }
                    if observed.expect_missing_websocket && level == "warn"
                        && message.contains("WebSocket is not found. Please use Lynx >= 2.16 or consider using a polyfill.") =>
                {
                    // The unmodified default HMR client detects the absent
                    // platform module, warns once and lets React start.
                    observed.missing_websocket_warnings += 1;
                }
                other => panic!("reload failed: {other:?}"),
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
        if pixel == Some(expected)
            && view.is_ready()
            && observed.booted
            && marker.is_none_or(|marker| observed.messages.iter().any(|message| message == marker))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "reload pixel expected {expected:?}, got {pixel:?}; boot={}, messages={:?}",
            observed.booted,
            observed.messages
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
