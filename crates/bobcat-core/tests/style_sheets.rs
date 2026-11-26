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
use std::time::{Duration, Instant};

use bobcat_core::{
    DrawTarget, LynxView, LynxViewError, NoWakeup, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, ViewSources,
};
use support::{FetcherDouble, solo_view, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const SCRIPT_URL: &str = "app:///main-thread.js";
const SHEET_URL: &str = "app:///author.css";

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
        property: property.to_owned().into(),
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
                declaration("background-color", "rgb(0, 0, 255)"),
            ],
        )],
    }
}

fn sources(style_sheets: &[&str]) -> ViewSources {
    ViewSources {
        style_sheets: style_sheets.iter().map(|url| (*url).to_owned()).collect(),
        ..ViewSources::new("app:///", SCRIPT_URL, SCREEN, "en-US")
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
    assert!(message.contains(SCRIPT_URL), "{message}");
    assert!(message.contains("UTF-8"), "{message}");
}

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

const ADOPTING_FRAME_SCRIPT: &str = r"
    Object.assign(globalThis, {
      processData: function (data) { return data; },
      renderPage: function () {
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __SetClasses(view, 'basic');
        __AppendElement(page, view);
        lynx.requestAnimationFrame(function () {
          __AdoptStyleSheet({ url: 'app:///late.css' });
          __FlushElementTree();
        });
      },
      updatePage: function () {},
    });
";

#[tokio::test]
async fn a_frame_whose_callback_adopts_a_sheet_is_serviced_once_the_host_pumps() {
    let fetcher = Rc::new(
        FetcherDouble::card(ADOPTING_FRAME_SCRIPT)
            .with_preparsed_style_sheet(basic_sheet())
            .resolving_to(SCRIPT_URL),
    );
    let (mut view, mut painter) = view_with(|_sink| fetcher.clone(), sources(&[]))
        .await
        .expect("loading view");
    wait_for_script(&mut view).expect("script execution");

    let started = Instant::now();
    let posted = painter.post_frame().expect("an offscreen painter posts");
    let withheld = Instant::now() + Duration::from_millis(200);
    while Instant::now() < withheld {
        assert!(
            !painter.frame_serviced(&posted),
            "the frame was serviced before the host answered the adoption it waits behind"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        fetcher.style_sheet_fetch_count(),
        0,
        "the request reaches the fetcher only in the view's own turn"
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    while !painter.frame_serviced(&posted) {
        for _event in view.pump() {}
        assert!(Instant::now() < deadline, "the frame was never serviced");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(fetcher.style_sheet_fetch_count(), 1);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the frame took {:?}",
        started.elapsed()
    );

    painter
        .draw_posted(posted, true)
        .expect("the serviced frame draws");
    let shot = painter.capture().expect("capture the adopted page");
    let offset = (50 * shot.size.width as usize + 50) * 4;
    assert_eq!(
        &shot.pixels[offset..offset + 4],
        &[0, 0, 255, 255],
        "the serviced frame carries the sheet its own callback adopted"
    );
}
