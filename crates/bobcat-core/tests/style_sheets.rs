//! The stylesheet half of [`ViewSources`], over both accepted forms.
//!
//! The resource provider answers a stylesheet request with either CSS text or
//! a [`PreparsedStyleSheet`] it decoded itself; both mount on the document the
//! entry module builds, and each listed sheet is its own request. The order
//! sheets mount in is asserted where it is observable, in bobcat-resources'
//! `source_startup`.

mod support;

use std::rc::Rc;
use std::sync::Arc;

use bobcat_core::{
    DrawTarget, LynxView, LynxViewError, NoWakeup, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, ViewSources,
};
use support::{FetcherDouble, solo_view, wait_for_script};

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const SCRIPT_URL: &str = "app:///main-thread.js";
const SHEET_URL: &str = "app:///author.css";

/// Creates one classed `view` under the page, then flushes.
const CLASSED_VIEW_SCRIPT: &str = r"
    Object.assign(globalThis, {
      processData: function (data) { return data; },
      renderPage: function () {
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __SetClasses(view, 'basic');
        __AppendElement(page, view);
      },
      updatePage: function () {},
    });
";

fn declaration(property: &str, value: &str) -> PreparsedDeclaration {
    PreparsedDeclaration {
        property: property.to_owned(),
        value: value.to_owned(),
        important: false,
    }
}

fn style_rule(selectors: &str, declarations: Vec<PreparsedDeclaration>) -> PreparsedRule {
    PreparsedRule::Style {
        selectors: selectors.to_owned(),
        declarations,
    }
}

/// A sheet whose one rule is observable in a capture: a 100×100 opaque box at
/// the page's origin.
fn basic_sheet() -> PreparsedStyleSheet {
    PreparsedStyleSheet {
        rules: vec![style_rule(
            ".basic",
            vec![
                declaration("width", "100px"),
                declaration("height", "100px"),
                declaration("background-color", "rgb(0, 0, 255)"),
            ],
        )],
    }
}

fn sources(style_sheets: &[&str]) -> ViewSources {
    ViewSources {
        style_sheets: style_sheets.iter().map(|url| (*url).to_owned()).collect(),
        ..ViewSources::new("app:///", SCRIPT_URL, SCREEN)
    }
}

async fn view_with(
    resources: impl FnOnce(bobcat_core::ImageReports) -> Rc<FetcherDouble>,
    sources: ViewSources,
) -> Result<(LynxView<Rc<FetcherDouble>>, bobcat_core::Painter), LynxViewError> {
    solo_view(
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        DrawTarget::Offscreen,
        resources,
        sources,
    )
    .await
}

/// The pre-parsed arm mounts, and its rules reach the page the entry builds.
///
/// When a sheet mounts is not what this asserts: boot's first
/// `__FlushElementTree` mounts every listed sheet before the document is
/// styled. That a text sheet and a pre-parsed one both mount through the real
/// resource system, in listed order, is bobcat-resources'
/// `text_and_preparsed_sheets_keep_cascade_order`; that boot publishes
/// nothing until a withheld sheet arrives, and that several sheets cascade in
/// listed order whatever order they arrived in, are `page_tests`'
/// `boot_publishes_nothing_until_a_withheld_sheet_arrives` and
/// `author_sheets_cascade_in_listed_order_and_boot_waits_for_all_of_them`.
#[tokio::test]
async fn a_preparsed_sheet_styles_the_page() {
    let fetcher = Rc::new(
        FetcherDouble::card(CLASSED_VIEW_SCRIPT)
            .with_preparsed_style_sheet(basic_sheet())
            .resolving_to(SCRIPT_URL),
    );

    let (mut view, mut painter) = view_with(|_sink| fetcher, sources(&[SHEET_URL]))
        .await
        .expect("the pre-parsed arm mounts");
    wait_for_script(&mut view).expect("script execution");

    let shot = painter.capture().expect("capture the styled page");
    let pixel = |x: usize, y: usize| {
        let offset = (y * shot.size.width as usize + x) * 4;
        &shot.pixels[offset..offset + 4]
    };
    assert_eq!(pixel(50, 50), &[0, 0, 255, 255], "inside the sheet's box");
    assert_ne!(pixel(200, 300), &[0, 0, 255, 255], "outside it");
}

#[tokio::test]
async fn a_css_text_sheet_mounts_through_the_same_entry_point() {
    // The raw-text arm is independent of the entry resource, as it is for an
    // embedder that keeps both URLs in one resource registry.
    let fetcher = Rc::new(
        FetcherDouble::card(CLASSED_VIEW_SCRIPT)
            .with_style_sheet_text(b".basic { width: 100px; height: 100px; }".to_vec())
            .resolving_to(SCRIPT_URL),
    );

    let (mut view, _painter) = view_with(|_sink| fetcher, sources(&[SHEET_URL]))
        .await
        .expect("loading view");
    wait_for_script(&mut view).expect("the text arm mounts");
}

/// A BOM survives the fetch boundary intact and reaches the decode step.
/// (That the rule it prefixes still matches is asserted where computed style
/// is observable, in `bobcat_core::style`.)
#[tokio::test]
async fn a_byte_order_mark_prefixed_sheet_mounts() {
    let mut css = "\u{feff}".as_bytes().to_vec();
    css.extend_from_slice(b".basic { width: 100px; }");
    let fetcher = Rc::new(
        FetcherDouble::card(CLASSED_VIEW_SCRIPT)
            .with_style_sheet_text(css)
            .resolving_to(SCRIPT_URL),
    );

    let (mut view, _painter) = view_with(|_sink| fetcher, sources(&[SHEET_URL]))
        .await
        .expect("loading view");
    wait_for_script(&mut view).expect("a BOM-prefixed sheet mounts");
}

/// A stylesheet that will not decode reports a precise startup failure.
///
/// The listed sheets are mounted by boot's first `__FlushElementTree`, so what
/// the embedder is told is the exception that flush threw — a `Script` error
/// rather than the fetcher's own `InvalidStyleSheetEncoding` — and the message
/// is what has to name the sheet and the reason.
#[tokio::test]
async fn a_stylesheet_that_is_not_utf8_is_a_precise_error() {
    let fetcher = Rc::new(
        FetcherDouble::card(CLASSED_VIEW_SCRIPT)
            .with_style_sheet_text(vec![0xff, 0xfe, 0x00])
            .resolving_to(SCRIPT_URL),
    );

    let (mut view, _painter) = view_with(|_sink| fetcher, sources(&[SHEET_URL]))
        .await
        .expect("loading view");
    let error = wait_for_script(&mut view)
        .expect_err("invalid UTF-8 CSS is rejected, not silently dropped");
    assert!(matches!(error, LynxViewError::Script(_)), "{error}");
    let message = error.to_string();
    // The reported URL is the resolved one, as it is for a script.
    assert!(message.contains(SCRIPT_URL), "{message}");
    assert!(message.contains("UTF-8"), "{message}");
}

/// Each listed sheet is a separate stylesheet request, so a repeated URL
/// accumulates sheets rather than collapsing to one. (That the later sheet
/// wins a cascade tie is asserted where computed style is observable, in
/// `bobcat_core::style`.)
///
/// Both requests are made inside `create_lynx_view`, before the first turn;
/// what the counts below read is the whole of startup, so the timing does not
/// change them.
#[tokio::test]
async fn every_listed_sheet_issues_its_own_stylesheet_request() {
    let fetcher = Rc::new(
        FetcherDouble::card(CLASSED_VIEW_SCRIPT)
            .with_preparsed_style_sheet(basic_sheet())
            .resolving_to(SCRIPT_URL),
    );

    let (mut view, _painter) = view_with(|_sink| fetcher.clone(), sources(&[SHEET_URL, SHEET_URL]))
        .await
        .expect("both sheets mount");
    wait_for_script(&mut view).expect("script execution");
    assert_eq!(fetcher.style_sheet_fetch_count(), 2);
    assert_eq!(
        fetcher.fetch_count(),
        1,
        "a stylesheet the host answers pre-parsed costs no payload; the one payload served \
         is the entry module"
    );
}
