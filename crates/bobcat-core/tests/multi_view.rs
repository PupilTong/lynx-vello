//! Several views, several threads, at the same time.
//!
//! A view already spanned two threads — the embedder's, which paints, and its
//! own `bobcat-main`. What multiplies is the pair: a host builds one view per
//! thread it wants to host a page on, and each of those views brings its own
//! Stylo workers, so two pages restyle at once instead of queueing behind one
//! process-wide traversal lock.
//!
//! Every view here is built behind a barrier, so the constructions, boots and
//! first commits genuinely overlap rather than merely coexisting.
//!
//! The workers are `bobcat-main`'s to start: it builds them on the thread
//! that owns the document they serve, before that document exists, and their
//! failure to start is a construction failure like any other boot failure.

mod support;

use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, Barrier};
use std::thread::ThreadId;

use bobcat_core::{
    DrawTarget, LynxView, MAX_STYLE_THREADS, NoWakeup, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, StyleThreads, ViewSources,
};
use support::{FetcherDouble, wait_for_script};

const SCRIPT_URL: &str = "app:///main.js";
const STYLE_URL: &str = "app:///author.css";
const VIEWS: usize = 3;

/// Enough boxes that the style traversal is worth handing to a pool: Stylo's
/// driver only parallelizes a level wider than one work unit.
const BOXES: usize = 64;

/// One page of identically classed boxes, so every view runs the same shape
/// of work and only its colour differs.
fn page_script() -> String {
    format!(
        r"
globalThis.renderPage = function renderPage() {{
  const page = __CreatePage('card', 0);
  for (let index = 0; index < {BOXES}; index += 1) {{
    const box = __CreateView(0);
    __SetClasses(box, 'box');
    __AppendElement(page, box);
  }}
}};
"
    )
}

fn declaration(property: &str, value: &str) -> PreparsedDeclaration {
    PreparsedDeclaration {
        property: property.to_owned(),
        value: value.to_owned(),
        important: false,
    }
}

fn sheet(color: &str) -> PreparsedStyleSheet {
    PreparsedStyleSheet {
        rules: vec![PreparsedRule::Style {
            selectors: ".box".to_owned(),
            declarations: vec![
                declaration("width", "8px"),
                declaration("height", "8px"),
                declaration("background-color", color),
            ],
        }],
    }
}

/// What one view reports back to the test. The view itself never leaves the
/// thread that built it — its painter is `!Send`, and so is it.
#[derive(Debug)]
struct ViewReport {
    painter: ThreadId,
    top_left: [u8; 4],
}

fn run_view(color: &'static str, ready: &Barrier) -> ViewReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a per-thread runtime");
    // Every thread arrives here with its resources already prepared, so the
    // constructions themselves are what overlap.
    ready.wait();
    runtime.block_on(async move {
        let fetcher = Rc::new(
            FetcherDouble::new(page_script().into_bytes())
                .resolving_to(SCRIPT_URL)
                .with_preparsed_style_sheet(sheet(color)),
        );
        let mut view = LynxView::new(
            Arc::new(NoWakeup),
            32.0,
            24.0,
            1.0,
            DrawTarget::Offscreen,
            |_reports| fetcher,
            ViewSources {
                style_sheets: vec![STYLE_URL.to_owned()],
                // Its own workers, not a share of anyone else's.
                style_threads: StyleThreads::Fixed(
                    NonZeroUsize::new(2).expect("a positive worker count"),
                ),
                ..ViewSources::new(SCRIPT_URL)
            },
        )
        .await
        .expect("the view is built");
        wait_for_script(&mut view).expect("the entry module boots");
        view.tick(true).expect("the first frame");
        let shot = view.capture().expect("the committed frame");
        let top_left = <[u8; 4]>::try_from(&shot.pixels[..4]).expect("an RGBA frame");
        ViewReport {
            painter: std::thread::current().id(),
            top_left,
        }
    })
}

#[test]
fn views_on_separate_threads_boot_and_paint_concurrently() {
    let colors = ["#ff0000", "#00ff00", "#0000ff"];
    let expected = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
    let ready = Arc::new(Barrier::new(VIEWS));

    let views: Vec<_> = colors
        .into_iter()
        .map(|color| {
            let ready = Arc::clone(&ready);
            std::thread::spawn(move || run_view(color, &ready))
        })
        .collect();

    let reports: Vec<_> = views
        .into_iter()
        .map(|view| view.join().expect("the hosting thread finished"))
        .collect();

    assert_eq!(reports.len(), VIEWS);
    for (report, color) in reports.iter().zip(expected) {
        assert_eq!(
            report.top_left, color,
            "each view painted its own page, not another's: {reports:?}"
        );
    }

    let painters: std::collections::HashSet<_> =
        reports.iter().map(|report| report.painter).collect();
    assert_eq!(
        painters.len(),
        VIEWS,
        "each view paints on the thread that built it, and no two shared one"
    );
    assert!(
        !painters.contains(&std::thread::current().id()),
        "the test thread hosts no view of its own"
    );
}

/// Workers that cannot start are a view that was never built.
///
/// The ceiling is Stylo's `ScopedTLS` array length, and the refusal happens on
/// `bobcat-main` — the thread that builds the pool — so this is also the proof
/// that a failure there is reported back as a construction error rather than
/// as a view that boots without anywhere to restyle.
#[tokio::test]
async fn a_view_asking_for_more_workers_than_stylo_indexes_is_not_built() {
    let fetcher = Rc::new(
        FetcherDouble::new(page_script().into_bytes())
            .resolving_to(SCRIPT_URL)
            .with_preparsed_style_sheet(sheet("#ff0000")),
    );
    let error = LynxView::new(
        Arc::new(NoWakeup),
        32.0,
        24.0,
        1.0,
        DrawTarget::Offscreen,
        |_reports| fetcher,
        ViewSources {
            style_sheets: vec![STYLE_URL.to_owned()],
            style_threads: StyleThreads::Fixed(
                NonZeroUsize::new(MAX_STYLE_THREADS + 1).expect("a positive worker count"),
            ),
            ..ViewSources::new(SCRIPT_URL)
        },
    )
    .await
    .expect_err("the ceiling is enforced before a view exists");
    let message = error.to_string();
    assert!(
        message.contains(&MAX_STYLE_THREADS.to_string()),
        "the ceiling is named: {message}"
    );
}
