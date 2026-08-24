mod support;

use std::sync::Arc;

use bobcat_core::resource::ResourceFetcher;
use bobcat_core::script::ScriptError;
use bobcat_core::{LynxView, NoWindow, PageConfig};
use support::{FetcherDouble, wait_for_script};

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

async fn run(config: PageConfig, source: &str, resolved_url: &str) -> Result<(), ScriptError> {
    let resources: Arc<dyn ResourceFetcher> =
        Arc::new(FetcherDouble::new(source.as_bytes().to_vec()).resolving_to(resolved_url));
    let mut view = LynxView::<NoWindow>::new(config, resources, Arc::new(|| {}), 393.0, 727.0, 1.0)
        .expect("view");
    view.execute_script("main.js")
        .await
        .expect("fetch and start");

    wait_for_script(&mut view)
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

/// The card's own reporter, made fatal.
///
/// `ReactLynx` installs an error boundary around the render it drives, so a
/// missing PAPI surfaces as a call to `_ReportError` rather than as a thrown
/// exception. The realm's shim swallows that call by design; rethrowing is what
/// makes the boundary's report reach `ScriptRunError`, and what keeps this test
/// from passing on a card that failed quietly.
fn with_fatal_reporter(root: &str) -> String {
    format!("globalThis._ReportError = function (error) {{ throw error; }};\n{root}")
}

#[tokio::test]
async fn decoded_scripts_boot_through_the_element_papi_alone() {
    for (name, bytes) in FIXTURES {
        let template = lynx_template_decoder::decode(bytes).expect("decode");
        let root = template
            .lepus_code
            .get("root")
            .unwrap_or_else(|| panic!("{name} has no lepusCode.root"));
        run(
            page_config(&template),
            &with_fatal_reporter(root),
            &format!("app:///{name}/main-thread.js"),
        )
        .await
        .unwrap_or_else(|error| panic!("{name} needs a PAPI test double: {error}"));
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
