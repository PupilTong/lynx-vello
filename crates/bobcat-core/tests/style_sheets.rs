//! `LynxView::load_style_sheet` over both accepted stylesheet forms.
//!
//! The resource provider answers a stylesheet request with either CSS text or
//! a [`PreparsedStyleSheet`] it decoded itself; both mount on the document, in
//! load order, and both are visible to the script that runs afterwards.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::{ResourceCapability, ResourceFetcher};
use bobcat_core::{
    EngineEvent, LynxView, LynxViewError, NoWindow, PageConfig, PreparsedDeclaration,
    PreparsedRule, PreparsedStyleSheet,
};
use support::FetcherDouble;

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

fn view_with(fetcher: FetcherDouble) -> LynxView<'static, NoWindow> {
    let resources: Arc<dyn ResourceFetcher> = Arc::new(fetcher.resolving_to(SCRIPT_URL));
    LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        Arc::new(|| {}),
        393.0,
        727.0,
        1.0,
    )
    .expect("view")
}

async fn run_script(view: &mut LynxView<'static, NoWindow>) {
    view.execute_script(SCRIPT_URL)
        .await
        .expect("fetch and start");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = view.pump().into_iter().find_map(|event| match event {
            EngineEvent::ScriptFinished(result) => Some(result),
            _ => None,
        }) {
            result.expect("script execution");
            return;
        }
        assert!(Instant::now() < deadline, "script thread did not finish");
        std::thread::yield_now();
    }
}

#[tokio::test]
async fn a_preparsed_sheet_loads_through_the_resource_provider() {
    let fetcher = FetcherDouble::new(CLASSED_VIEW_SCRIPT.as_bytes().to_vec())
        .with_preparsed_style_sheet(basic_sheet());
    assert!(
        fetcher.supports_capability(ResourceCapability::PreparsedStyleSheet),
        "a decoding host advertises the pre-parsed arm"
    );
    let mut view = view_with(fetcher);

    view.load_style_sheet(SHEET_URL)
        .await
        .expect("the pre-parsed arm mounts");
    run_script(&mut view).await;
}

#[tokio::test]
async fn a_css_text_sheet_loads_through_the_same_entry_point() {
    // No pre-parsed sheet registered, so the request falls through to the
    // byte path — the arm a browser embedder that only moves bytes uses.
    let mut view = view_with(FetcherDouble::new(
        b".basic { width: 100px; height: 100px; }".to_vec(),
    ));

    view.load_style_sheet(SHEET_URL)
        .await
        .expect("the text arm mounts");
}

/// A BOM survives the fetch boundary intact and reaches the decode step.
/// (That the rule it prefixes still matches is asserted where computed style
/// is observable, in `bobcat_core::style`.)
#[tokio::test]
async fn a_byte_order_mark_prefixed_sheet_loads() {
    let mut css = "\u{feff}".as_bytes().to_vec();
    css.extend_from_slice(b".basic { width: 100px; }");
    let mut view = view_with(FetcherDouble::new(css));

    view.load_style_sheet(SHEET_URL)
        .await
        .expect("a BOM-prefixed sheet mounts");
}

#[tokio::test]
async fn a_stylesheet_that_is_not_utf8_is_a_precise_error() {
    let mut view = view_with(FetcherDouble::new(vec![0xff, 0xfe, 0x00]));

    let error = view
        .load_style_sheet(SHEET_URL)
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

/// Each load is a separate stylesheet request, so repeated loads accumulate
/// sheets rather than replacing one. (That the later sheet wins a cascade tie
/// is asserted where computed style is observable, in `bobcat_core::style`.)
#[tokio::test]
async fn every_load_issues_its_own_stylesheet_request() {
    let fetcher = Arc::new(
        FetcherDouble::new(CLASSED_VIEW_SCRIPT.as_bytes().to_vec())
            .with_preparsed_style_sheet(basic_sheet())
            .resolving_to(SCRIPT_URL),
    );
    let resources: Arc<dyn ResourceFetcher> = fetcher.clone();
    let mut view = LynxView::<NoWindow>::new(
        PageConfig::default(),
        resources,
        Arc::new(|| {}),
        393.0,
        727.0,
        1.0,
    )
    .expect("view");

    view.load_style_sheet(SHEET_URL).await.expect("first sheet");
    view.load_style_sheet(SHEET_URL)
        .await
        .expect("second sheet");
    assert_eq!(fetcher.style_sheet_fetch_count(), 2);
    assert_eq!(
        fetcher.fetch_count(),
        0,
        "a stylesheet must not be fetched through the byte path when the host answers it pre-parsed"
    );
    run_script(&mut view).await;
}

#[tokio::test]
async fn a_sheet_loads_after_the_script_has_finished() {
    let mut view = view_with(
        FetcherDouble::new(CLASSED_VIEW_SCRIPT.as_bytes().to_vec())
            .with_preparsed_style_sheet(basic_sheet()),
    );

    run_script(&mut view).await;
    view.load_style_sheet(SHEET_URL)
        .await
        .expect("a sheet mounts against an existing tree");
}
