#![cfg(feature = "quickjs")]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::{
    EngineEvent, LynxView, NoWindow, PageConfig, ScriptRunError, quickjs_engine_factory,
};
use support::FetcherDouble;

const FIXTURES: &[(&str, &[u8])] = &[
    (
        "basic-bindtap",
        include_bytes!("../../lynx-template-decoder/tests/fixtures/basic-bindtap.web.bundle"),
    ),
    (
        "basic-class-selector",
        include_bytes!(
            "../../lynx-template-decoder/tests/fixtures/basic-class-selector.web.bundle"
        ),
    ),
];

fn page_config(template: &lynx_template_decoder::WebTemplate) -> PageConfig {
    PageConfig {
        default_display_linear: template.config_flag("defaultDisplayLinear"),
        default_overflow_visible: template.config_flag("defaultOverflowVisible"),
        enable_css_selector: template.config_flag("enableCSSSelector"),
    }
}

async fn run(config: PageConfig, source: &str, resolved_url: &str) -> Result<(), ScriptRunError> {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(source.as_bytes().to_vec()).resolving_to(resolved_url));
    let mut view = LynxView::<NoWindow>::new(
        config,
        resources,
        quickjs_engine_factory(),
        Arc::new(|| {}),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");
    view.execute_script("main.js")
        .await
        .expect("fetch and start");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = view.pump().into_iter().find_map(|event| match event {
            EngineEvent::ScriptFinished(result) => Some(result),
            _ => None,
        }) {
            return result;
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        std::thread::yield_now();
    }
}

#[test]
fn decoded_bundle_page_config_is_supplied_at_view_construction() {
    for (name, bytes) in FIXTURES {
        let template = lynx_template_decoder::decode(bytes).expect("decode");
        let config = page_config(&template);
        assert!(config.default_display_linear, "{name}");
        assert!(config.default_overflow_visible, "{name}");
        assert!(config.enable_css_selector, "{name}");
    }
}

#[tokio::test]
async fn decoded_script_is_passed_to_the_public_view_without_moving_decode_into_core() {
    for (name, bytes) in FIXTURES {
        let template = lynx_template_decoder::decode(bytes).expect("decode");
        let root = template
            .lepus_code
            .get("root")
            .unwrap_or_else(|| panic!("{name} has no lepusCode.root"));
        let url = format!("app:///{name}/main-thread.js");
        let error = run(page_config(&template), root, &url)
            .await
            .expect_err("the pending lynx global must remain a precise error");
        let message = error.to_string();
        assert!(
            message.contains("'lynx' is not defined"),
            "{name}: {message}"
        );
        assert!(message.contains(&url), "{name}: {message}");
    }
}

#[tokio::test]
async fn bundle_shaped_boot_sequence_runs_through_the_public_facade() {
    let template = lynx_template_decoder::decode(FIXTURES[0].1).expect("decode");
    run(
        page_config(&template),
        r"
        Object.assign(globalThis, {
          processData: function (data) { return data; },
          renderPage: function () {
            __AppendElement(__CreatePage('card', 0), __CreateView(0));
          },
          updatePage: function () {},
        });
        ",
        "app:///bundle-main.js",
    )
    .await
    .expect("boot");
}
