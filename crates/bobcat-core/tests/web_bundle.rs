#![cfg(feature = "quickjs")]

//! How far a real `.web.bundle`'s main-thread script gets today.
//!
//! The fixtures are the same `ReactLynx` build artifacts
//! `lynx-template-decoder` decodes, vendored from lynx-stack's `web-core-e2e`
//! suite. Running their `lepusCode.root` end to end needs far more than the
//! five Element PAPI members that exist; this test pins *exactly* where the
//! wall is, so the gap is a failing assertion to update rather than a
//! paragraph of prose that rots.

use std::cell::RefCell;
use std::rc::Rc;

use bobcat_core::quickjs::{MainThreadRuntime, local_commit_sink};
use lynx_element::{ElementTree, PageConfig, Viewport};

/// The single-threaded composition: the realm records PAPI writes and every
/// `__FlushElementTree` applies them to this shared tree on the spot.
fn shared_tree(config: PageConfig) -> (MainThreadRuntime, Rc<RefCell<ElementTree>>) {
    let elements = Rc::new(RefCell::new(ElementTree::new(VIEWPORT, config)));
    let runtime = MainThreadRuntime::new(local_commit_sink(&elements)).expect("QuickJS realm");
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

/// The decoded page config drives the UA cascade, exactly as web-core's
/// `onPageConfigReady` bakes it into the element-API closures before any
/// script runs.
#[test]
fn the_bundle_page_config_reaches_the_ua_cascade() {
    for (name, bytes) in FIXTURES {
        let template = lynx_template_decoder::decode(bytes).expect("decode");
        let config = page_config(&template);
        // Every fixture built by today's toolchain carries these.
        assert!(config.default_display_linear, "{name}");
        assert!(config.default_overflow_visible, "{name}");
        assert!(config.enable_css_selector, "{name}");

        let (_runtime, elements) = shared_tree(config);
        assert_eq!(elements.borrow().config(), config, "{name}");
    }
}

/// Where a real `ReactLynx` bundle stops today.
///
/// It is *not* the Element PAPI: a card root reaches for the `lynx` object
/// during its own module initialization — `ReactLynx`'s runtime calls
/// `lynx.registerDataProcessors()` at load so that `globalThis.processData`
/// always exists — so evaluation fails before `renderPage` is ever assigned,
/// let alone called.
///
/// That makes the next piece of runtime work concrete: the main-thread global
/// object (`lynx`, `SystemInfo`, `__globalProps`, `_ReportError`, …), which
/// web-core installs from `createMainThreadGlobalAPIs` alongside the Element
/// PAPI. Adding more PAPI members would not move this wall at all.
///
/// This assertion is meant to be *tightened* as the runtime grows: when the
/// `lynx` object lands, this test should fail and be updated to name whatever
/// the next missing global is.
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
        // The failure is a clean one: the realm reports where it happened, and
        // nothing was half-rendered.
        assert!(
            message.contains("main-thread.js:"),
            "{name}: the error should carry a source location: {message}"
        );
        assert!(elements.borrow().page().is_none(), "{name}");
    }
}

/// The wrapper and boot sequence themselves are not the blocker: the same
/// bundle's entry-point shape works when the script does not reach for a
/// global we have not built.
#[test]
fn the_boot_sequence_works_on_a_bundle_shaped_script() {
    let template = lynx_template_decoder::decode(FIXTURES[0].1).expect("decode");
    let (mut runtime, elements) = shared_tree(page_config(&template));

    // The shape a real card root has, minus the `lynx` dependency: an
    // `Object.assign(globalThis, …)` of the entry points web-core looks up.
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
    let elements = elements.borrow();
    assert!(elements.page().is_some());
    assert!(elements.element(2).is_some(), "the appended view is live");
}
