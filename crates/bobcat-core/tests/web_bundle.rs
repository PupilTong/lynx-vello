mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::script::ScriptError;
use bobcat_core::{DrawTarget, LynxView, NoWakeup, PageConfig, ViewSources};
use support::{FetcherDouble, wait_for_script};

const FIXTURES: &[(&str, &[u8])] = &[
    (
        "basic-bindtap",
        include_bytes!("../../bobcat-source/tests/fixtures/basic-bindtap.web.bundle"),
    ),
    (
        "basic-class-selector",
        include_bytes!("../../bobcat-source/tests/fixtures/basic-class-selector.web.bundle"),
    ),
];

fn page_config(template: &bobcat_source::web::WebTemplate) -> PageConfig {
    PageConfig {
        default_display_linear: template.config_flag("defaultDisplayLinear"),
        default_overflow_visible: template.config_flag("defaultOverflowVisible"),
        enable_css_selector: template.config_flag("enableCSSSelector"),
    }
}

async fn run(config: PageConfig, source: &str, resolved_url: &str) -> Result<(), ScriptError> {
    let fetcher =
        Rc::new(FetcherDouble::new(source.as_bytes().to_vec()).resolving_to(resolved_url));
    let mut view = LynxView::new(
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        |_reports| fetcher,
        ViewSources {
            config,
            ..ViewSources::new("main.js")
        },
    )
    .await
    .expect("fetch and start");

    wait_for_script(&mut view)
}

#[test]
fn decoded_bundle_page_config_is_supplied_at_view_construction() {
    for (name, bytes) in FIXTURES {
        let template = bobcat_source::web::decode(bytes).expect("decode");
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
/// makes the boundary's report fail startup, and what keeps this test from
/// passing on a card that failed quietly.
fn with_fatal_reporter(root: &str) -> String {
    format!("globalThis._ReportError = function (error) {{ throw error; }};\n{root}")
}

#[tokio::test]
async fn decoded_scripts_boot_through_the_element_papi_alone() {
    for (name, bytes) in FIXTURES {
        let template = bobcat_source::web::decode(bytes).expect("decode");
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
    let template = bobcat_source::web::decode(FIXTURES[0].1).expect("decode");
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
