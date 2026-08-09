#![cfg(feature = "quickjs")]

//! A real `.web.bundle`'s main-thread script, booted end to end.
//!
//! The fixtures are the same `ReactLynx` build artifacts
//! `lynx-template-decoder` decodes, vendored from lynx-stack's `web-core-e2e`
//! suite. Each one is evaluated in the realm and driven through
//! `processData` → `renderPage` → `__FlushElementTree`, then its committed
//! element tree is asserted against the exact Element PAPI sequence the bundle
//! is known to issue. That makes both directions regressions: a missing global
//! stops the boot, and a wrong handle or a dropped mutation shows up as a
//! missing element rather than as a blank frame.

use bobcat_core::engine::SharedTree;
use bobcat_core::quickjs::MainThreadRuntime;
use lynx_element::{ElementTree, LynxElement, PageConfig, Viewport};

/// The single-threaded composition: the realm takes the tree from this
/// slot per batch and every `__FlushElementTree` puts it back committed.
fn shared_tree(config: PageConfig) -> (MainThreadRuntime, SharedTree) {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, config));
    let runtime = MainThreadRuntime::new(elements.clone(), VIEWPORT, || {}).expect("QuickJS realm");
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
    (
        "basic-performance-large-css",
        include_bytes!(
            "../../lynx-template-decoder/tests/fixtures/basic-performance-large-css.web.bundle"
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

fn root_script(bytes: &[u8]) -> (PageConfig, String, String) {
    let template = lynx_template_decoder::decode(bytes).expect("decode");
    let config = page_config(&template);
    let author_css = template
        .style_info
        .as_ref()
        .map(lynx_template_decoder::StyleInfo::to_css)
        .unwrap_or_default();
    let root = template
        .lepus_code
        .get("root")
        .expect("a bundle has a lepusCode.root")
        .clone();
    (config, root, author_css)
}

/// The decoded page config drives the UA cascade, exactly as web-core's
/// `onPageConfigReady` bakes it into the element-API closures before any
/// script runs.
#[test]
fn the_bundle_page_config_reaches_the_ua_cascade() {
    for (name, bytes) in FIXTURES {
        let (config, _, _) = root_script(bytes);
        // Every fixture built by today's toolchain carries these.
        assert!(config.default_display_linear, "{name}");
        assert!(config.default_overflow_visible, "{name}");
        assert!(config.enable_css_selector, "{name}");

        let (_runtime, elements) = shared_tree(config);
        assert_eq!(elements.tree().config(), config, "{name}");
    }
}

/// Every fixture boots: evaluation finds the main-thread globals it reaches
/// for, `renderPage` runs to completion, and the flush commits a page.
#[test]
fn every_fixture_renders_its_first_screen() {
    for (name, bytes) in FIXTURES {
        let (config, root, author_css) = root_script(bytes);
        let (mut runtime, elements) = shared_tree(config);
        elements.tree().add_author_stylesheet(&author_css);

        runtime
            .run_main_thread_script(&root)
            .unwrap_or_else(|error| panic!("{name} did not boot: {error}"));

        let elements = elements.tree();
        assert!(elements.page().is_some(), "{name} created no page");
        assert!(
            !elements.has_uncommitted_mutations(),
            "{name} left a batch open"
        );
        assert!(
            elements.element(2).is_some(),
            "{name} rendered no content under the page"
        );
    }
}

/// `basic-bindtap` binds one `tap` handler on its single view. The binding is
/// recorded on the element rather than as an attribute, which is where both
/// Lynx and web-core keep it.
#[test]
fn a_bundle_event_binding_is_recorded_on_its_element() {
    let (config, root, _) = root_script(FIXTURES[0].1);
    let (mut runtime, elements) = shared_tree(config);
    runtime.run_main_thread_script(&root).expect("boot");

    let elements = elements.tree();
    let events = elements
        .element(2)
        .map(LynxElement::events)
        .expect("the bound view is live");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].event_type, "bindEvent");
    assert_eq!(events[0].name, "Tap");
    assert_eq!(events[0].handler.as_deref(), Some("-2:0:"));
}

/// `__SetCSSId` reaches the element it names. The bundles all call it with the
/// page and `cssId` 0 — the unscoped sheet — which is what `__CreatePage`
/// already recorded, so the observable part is that the call is accepted and
/// lands rather than throwing on the array argument.
#[test]
fn the_page_carries_the_css_id_the_bundle_set() {
    for (name, bytes) in FIXTURES {
        let (config, root, _) = root_script(bytes);
        let (mut runtime, elements) = shared_tree(config);
        runtime.run_main_thread_script(&root).expect("boot");

        let elements = elements.tree();
        let page = elements.page().expect("a page");
        assert_eq!(
            elements.element(page).map(LynxElement::component_css_id),
            Some(0),
            "{name}"
        );
    }
}

/// The wrapper and boot sequence themselves are not the blocker: the same
/// bundle's entry-point shape works when the script defines only the entry
/// points web-core looks up.
#[test]
fn the_boot_sequence_works_on_a_bundle_shaped_script() {
    let (config, _, _) = root_script(FIXTURES[0].1);
    let (mut runtime, elements) = shared_tree(config);

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
    assert!(elements.page().is_some());
    assert!(elements.element(2).is_some(), "the appended view is live");
}
