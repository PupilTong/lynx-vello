//! Several views at once — in one group, and in several.
//!
//! A view already spanned two threads: the embedder's, which paints, and the
//! `bobcat-main` its group owns. What multiplies is the pair. A host puts
//! views in one group when it wants them to share that thread — one `QuickJS`
//! runtime, one Stylo pool, and turns taken one view at a time — and in
//! separate groups when it wants two pages genuinely parallel, which costs a
//! thread and a set of workers each.
//!
//! Every view in the concurrent test is built behind a barrier, so the
//! constructions, boots and first commits genuinely overlap rather than
//! merely coexisting.

mod support;

use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, Barrier};
use std::thread::ThreadId;

use bobcat_core::{
    DrawTarget, LynxGroup, MAX_STYLE_THREADS, NoWakeup, PreparsedDeclaration, PreparsedRule,
    PreparsedStyleSheet, StyleThreads, ViewSources,
};
use support::{FetcherDouble, wait_for_script};

const STYLE_URL: &str = "app:///author.css";
const VIEWS: usize = 3;

/// Enough boxes that the style traversal is worth handing to a pool: Stylo's
/// driver only parallelizes a level wider than one work unit.
const BOXES: usize = 64;

/// One page of identically classed boxes, so every view runs the same shape
/// of work and only its colour differs.
///
/// The guard is what makes a shared realm visible. Views in one group share a
/// `QuickJS` runtime but must not share a realm, and the second view to boot
/// on a shared one would find the first's `renderPage` already defined.
fn page_script() -> String {
    format!(
        r"
if (typeof globalThis.renderPage !== 'undefined') {{
  throw new Error('this realm already carries another view of the group');
}}
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

fn fetcher(entry_url: &str, color: &str) -> Rc<FetcherDouble> {
    Rc::new(
        FetcherDouble::new(page_script().into_bytes())
            .resolving_to(entry_url)
            .with_preparsed_style_sheet(sheet(color)),
    )
}

fn sources(entry_url: &str) -> ViewSources {
    ViewSources {
        style_sheets: vec![STYLE_URL.to_owned()],
        ..ViewSources::new(entry_url)
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
        let entry_url = "app:///main.js";
        // A group of this thread's own: its `bobcat-main`, its runtime and its
        // Stylo workers are shared with nothing on any other thread.
        let group = LynxGroup::new(
            Arc::new(NoWakeup),
            StyleThreads::Fixed(NonZeroUsize::new(2).expect("a positive worker count")),
        )
        .await
        .expect("the group starts");
        let mut view = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                DrawTarget::Offscreen,
                |_reports| fetcher(entry_url, color),
                sources(entry_url),
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
fn views_in_separate_groups_boot_and_paint_concurrently() {
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

/// Two views on one group's thread, sharing its runtime and its pool.
///
/// That this builds at all is the claim. Rayon takes a thread over as index
/// zero of the first pool built on it and refuses a second one there forever,
/// so a pool belonging to each view would make this configuration
/// unrepresentable — the second view's would be refused outright. One pool
/// per group is what makes two views on one thread possible, and their
/// traversals cannot collide because the single thread driving them is
/// already inside whichever one is running.
///
/// The two realms are separate even so, which the guard in `page_script`
/// pins: the second view's entry would throw if it booted into a realm the
/// first had already defined `renderPage` in.
///
/// Their entry URLs differ because module sources are registered on the
/// runtime the two views share. Giving each view its own URL is the
/// embedder's job, and this is what doing it looks like.
#[tokio::test]
async fn two_views_in_one_group_share_its_thread_and_still_paint_their_own_page() {
    let group = LynxGroup::new(
        Arc::new(NoWakeup),
        StyleThreads::Fixed(NonZeroUsize::new(2).expect("a positive worker count")),
    )
    .await
    .expect("the group starts");

    let mut views = Vec::new();
    for (entry_url, color) in [("app:///red.js", "#ff0000"), ("app:///blue.js", "#0000ff")] {
        let mut view = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                DrawTarget::Offscreen,
                |_reports| fetcher(entry_url, color),
                sources(entry_url),
            )
            .await
            .expect("the view is built on the group's thread");
        wait_for_script(&mut view).expect("the entry module boots");
        views.push(view);
    }

    let painted: Vec<[u8; 4]> = views
        .iter_mut()
        .map(|view| {
            view.tick(true).expect("the first frame");
            let shot = view.capture().expect("the committed frame");
            <[u8; 4]>::try_from(&shot.pixels[..4]).expect("an RGBA frame")
        })
        .collect();

    assert_eq!(
        painted,
        [[255, 0, 0, 255], [0, 0, 255, 255]],
        "each view painted its own page over the document it owns, not its sibling's"
    );
}

/// A group whose views would have nowhere to restyle is a group that was
/// never built.
///
/// The ceiling is Stylo's `ScopedTLS` array length, and the refusal happens on
/// `bobcat-main` — the thread that builds the pool — so this is also the proof
/// that a failure there is reported back as a construction error rather than
/// as a group that hands out views with no workers behind them.
#[tokio::test]
async fn a_group_asking_for_more_workers_than_stylo_indexes_is_not_built() {
    let error = LynxGroup::new(
        Arc::new(NoWakeup),
        StyleThreads::Fixed(
            NonZeroUsize::new(MAX_STYLE_THREADS + 1).expect("a positive worker count"),
        ),
    )
    .await
    .expect_err("the ceiling is enforced before a group exists");
    let message = error.to_string();
    assert!(
        message.contains(&MAX_STYLE_THREADS.to_string()),
        "the ceiling is named: {message}"
    );
}
