#![cfg(feature = "quickjs")]

//! How far a real `.web.bundle`'s main-thread script gets today.
//!
//! The fixtures are the same `ReactLynx` build artifacts
//! `lynx-template-decoder` decodes, vendored from lynx-stack's `web-core-e2e`
//! suite. Running their `lepusCode.root` end to end needs far more than the
//! Element PAPI constructors that exist; this test pins *exactly* where the
//! wall is, so the gap is a failing assertion to update rather than a
//! paragraph of prose that rots.

use bobcat_core::engine::SharedTree;
use bobcat_core::quickjs::MainThreadRuntime;
use bobcat_core::tree::{ElementTree, PageConfig, Viewport};

fn shared_tree(config: PageConfig) -> (MainThreadRuntime, SharedTree) {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, config));
    let runtime = MainThreadRuntime::new(elements.clone(), || {}).expect("QuickJS realm");
    (runtime, elements)
}

const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

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

#[test]
fn the_bundle_page_config_reaches_the_ua_cascade() {
    for (name, bytes) in FIXTURES {
        let template = lynx_template_decoder::decode(bytes).expect("decode");
        let config = page_config(&template);
        assert!(config.default_display_linear, "{name}");
        assert!(config.default_overflow_visible, "{name}");
        assert!(config.enable_css_selector, "{name}");

        let (_runtime, elements) = shared_tree(config);
        assert_eq!(elements.tree().config(), config, "{name}");
    }
}

#[test]
fn a_real_bundle_stops_at_the_missing_lynx_global() {
    for (name, bytes) in FIXTURES {
        let template = lynx_template_decoder::decode(bytes).expect("decode");
        let root = template
            .lepus_code
            .get("root")
            .unwrap_or_else(|| panic!("{name} has no lepusCode.root"));

        let (mut runtime, elements) = shared_tree(page_config(&template));
        let error = runtime
            .evaluate_main_thread_script(root)
            .expect_err("a real ReactLynx bundle needs the main-thread global object");
        let message = error.to_string();
        assert!(
            message.contains("'lynx' is not defined"),
            "{name}: expected the missing `lynx` global, got: {message}"
        );
        assert!(
            message.contains("main-thread.js:"),
            "{name}: the error should carry a source location: {message}"
        );
        assert!(!elements.tree().page_created(), "{name}");
    }
}

#[test]
fn the_boot_sequence_works_on_a_bundle_shaped_script() {
    let template = lynx_template_decoder::decode(FIXTURES[0].1).expect("decode");
    let (mut runtime, elements) = shared_tree(page_config(&template));

    runtime
        .run_main_thread_script(
            r"
            Object.assign(globalThis, {
              processData: function (data) { return data; },
              renderPage: function () {
                __AppendElement(__CreatePage('card', 0), __CreateView(0));
              },
              updatePage: function () {},
            });
            ",
        )
        .expect("boot");
    let elements = elements.tree();
    assert!(elements.page_created());
    assert!(
        elements.document().get(2).is_some(),
        "the appended view is live"
    );
}
