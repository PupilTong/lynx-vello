//! The stylesheet half of [`ViewSources`], over both accepted forms.
//!
//! The resource provider answers a stylesheet request with either CSS text or
//! a [`PreparsedStyleSheet`] it decoded itself; both mount on the document
//! before the entry module runs, in the order the sources list them, and both
//! are visible to that script.

mod support;

use std::sync::Arc;

use bobcat_core::resource::{ResourceCapability, ResourceFetcher};
use bobcat_core::{
    LynxView, LynxViewError, NoWakeup, NoWindow, PageConfig, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, ViewSources,
};
use support::{FetcherDouble, wait_for_script};

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

fn basic_sheet() -> PreparsedStyleSheet {
    PreparsedStyleSheet {
        rules: vec![style_rule(
            ".basic",
            vec![
                declaration("width", "100px"),
                declaration("height", "100px"),
            ],
        )],
    }
}

fn sources(style_sheets: &[&str]) -> ViewSources {
    ViewSources {
        style_sheets: style_sheets.iter().map(|url| (*url).to_owned()).collect(),
        ..ViewSources::new(SCRIPT_URL)
    }
}

async fn view_with(
    fetcher: &dyn ResourceFetcher,
    sources: ViewSources,
) -> Result<LynxView<'static, NoWindow>, LynxViewError> {
    LynxView::<NoWindow>::new(
        PageConfig::default(),
        fetcher,
        Arc::new(NoWakeup),
        393.0,
        727.0,
        1.0,
        sources,
    )
    .await
}

#[tokio::test]
async fn a_preparsed_sheet_mounts_before_the_entry_module_runs() {
    let fetcher = FetcherDouble::new(CLASSED_VIEW_SCRIPT.as_bytes().to_vec())
        .with_preparsed_style_sheet(basic_sheet())
        .resolving_to(SCRIPT_URL);
    assert!(
        fetcher.supports_capability(ResourceCapability::PreparsedStyleSheet),
        "a decoding host advertises the pre-parsed arm"
    );

    let mut view = view_with(&fetcher, sources(&[SHEET_URL]))
        .await
        .expect("the pre-parsed arm mounts");
    wait_for_script(&mut view).expect("script execution");
}

#[tokio::test]
async fn a_css_text_sheet_mounts_through_the_same_entry_point() {
    // No pre-parsed sheet registered, so the request falls through to the
    // byte path — the arm a browser embedder that only moves bytes uses.
    let fetcher = FetcherDouble::new(b".basic { width: 100px; height: 100px; }".to_vec())
        .resolving_to(SCRIPT_URL);

    view_with(&fetcher, sources(&[SHEET_URL]))
        .await
        .expect("the text arm mounts");
}

/// A BOM survives the fetch boundary intact and reaches the decode step.
/// (That the rule it prefixes still matches is asserted where computed style
/// is observable, in `bobcat_core::style`.)
#[tokio::test]
async fn a_byte_order_mark_prefixed_sheet_mounts() {
    let mut css = "\u{feff}".as_bytes().to_vec();
    css.extend_from_slice(b".basic { width: 100px; }");
    let fetcher = FetcherDouble::new(css).resolving_to(SCRIPT_URL);

    view_with(&fetcher, sources(&[SHEET_URL]))
        .await
        .expect("a BOM-prefixed sheet mounts");
}

/// A stylesheet that will not decode fails the construction, so no document
/// and no main thread outlive it.
#[tokio::test]
async fn a_stylesheet_that_is_not_utf8_is_a_precise_error() {
    let fetcher = FetcherDouble::new(vec![0xff, 0xfe, 0x00]).resolving_to(SCRIPT_URL);

    let error = view_with(&fetcher, sources(&[SHEET_URL]))
        .await
        .expect_err("invalid UTF-8 CSS is rejected, not silently dropped");
    // The reported URL is the resolved one, as it is for a script.
    assert!(
        matches!(
            error,
            LynxViewError::InvalidStyleSheetEncoding { ref url, .. } if url == SCRIPT_URL
        ),
        "{error}"
    );
}

/// Each listed sheet is a separate stylesheet request, so a repeated URL
/// accumulates sheets rather than collapsing to one. (That the later sheet
/// wins a cascade tie is asserted where computed style is observable, in
/// `bobcat_core::style`.)
#[tokio::test]
async fn every_listed_sheet_issues_its_own_stylesheet_request() {
    let fetcher = FetcherDouble::new(CLASSED_VIEW_SCRIPT.as_bytes().to_vec())
        .with_preparsed_style_sheet(basic_sheet())
        .resolving_to(SCRIPT_URL);

    let mut view = view_with(&fetcher, sources(&[SHEET_URL, SHEET_URL]))
        .await
        .expect("both sheets mount");
    assert_eq!(fetcher.style_sheet_fetch_count(), 2);
    assert_eq!(
        fetcher.fetch_count(),
        1,
        "a stylesheet must not be fetched through the byte path when the host answers it \
         pre-parsed; the one byte fetch is the entry module"
    );
    wait_for_script(&mut view).expect("script execution");
}
