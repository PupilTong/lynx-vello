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
use std::time::{Duration, Instant};

use bobcat_core::{
    DrawTarget, LynxGroup, MAX_STYLE_THREADS, NoWakeup, Painter, PreparsedDeclaration,
    PreparsedRule, PreparsedStyleSheet, StyleThreads, ViewSources,
};
use support::{FetcherDouble, tick, wait_for_script};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const STYLE_URL: &str = "app:///author.css";
const VIEWS: usize = 3;

const BOXES: usize = 64;

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
        property: property.to_owned().into(),
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
        FetcherDouble::card(page_script())
            .resolving_to(entry_url)
            .with_preparsed_style_sheet(sheet(color)),
    )
}

fn sources(entry_url: &str) -> ViewSources {
    ViewSources {
        style_sheets: vec![STYLE_URL.to_owned()],
        ..ViewSources::new("app:///", entry_url, SCREEN, "en-US")
    }
}

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
    ready.wait();
    runtime.block_on(async move {
        let entry_url = "app:///main.js";
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
                |_reports| fetcher(entry_url, color),
                Vec::new(),
                sources(entry_url),
            )
            .expect("the view is built");
        let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
            .await
            .expect("the view's painter is built");
        painter.attach(&view).expect("a fresh view takes a painter");
        wait_for_script(&mut view).expect("the entry module boots");
        tick(&mut painter, true).expect("the first frame");
        let shot = painter.capture().expect("the committed frame");
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
                |_reports| fetcher(entry_url, color),
                Vec::new(),
                sources(entry_url),
            )
            .expect("the view is built on the group's thread");
        let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
            .await
            .expect("each view's painter is built");
        painter.attach(&view).expect("a fresh view takes a painter");
        wait_for_script(&mut view).expect("the entry module boots");
        views.push((view, painter));
    }

    let painted: Vec<[u8; 4]> = views
        .iter_mut()
        .map(|(_view, painter)| {
            tick(painter, true).expect("the first frame");
            let shot = painter.capture().expect("the committed frame");
            <[u8; 4]>::try_from(&shot.pixels[..4]).expect("an RGBA frame")
        })
        .collect();

    assert_eq!(
        painted,
        [[255, 0, 0, 255], [0, 0, 255, 255]],
        "each view painted its own page over the document it owns, not its sibling's"
    );
}

#[tokio::test]
async fn a_job_parked_on_one_views_host_holds_up_a_siblings_frame_without_blocking_it() {
    const ADOPTING: &str = r"
        globalThis.renderPage = function renderPage() {
          __CreatePage('card', 0);
          lynx.requestAnimationFrame(function () {
            __AdoptStyleSheet({ url: 'app:///late.css' });
          });
        };
    ";
    const PLAIN: &str = r"
        globalThis.renderPage = function renderPage() {
          __CreatePage('card', 0);
        };
    ";
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .expect("the group starts");

    let adopting_fetcher = Rc::new(
        FetcherDouble::card(ADOPTING)
            .resolving_to("app:///adopting.js")
            .with_preparsed_style_sheet(sheet("#ff0000")),
    );
    let mut views = Vec::new();
    for (entry_url, fetcher) in [
        ("app:///adopting.js", Rc::clone(&adopting_fetcher)),
        (
            "app:///plain.js",
            Rc::new(FetcherDouble::card(PLAIN).resolving_to("app:///plain.js")),
        ),
    ] {
        let mut view = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                |_reports| fetcher,
                Vec::new(),
                ViewSources::new("app:///", entry_url, SCREEN, "en-US"),
            )
            .expect("the view is built on the group's thread");
        let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
            .await
            .expect("each view's painter is built");
        painter.attach(&view).expect("a fresh view takes a painter");
        wait_for_script(&mut view).expect("the entry module boots");
        views.push((view, painter));
    }
    let [
        (adopting_view, adopting_painter),
        (_plain_view, plain_painter),
    ] = &mut views[..]
    else {
        unreachable!("two views were built");
    };

    let adopting_frame = adopting_painter.post_frame().expect("an offscreen post");
    let plain_frame = plain_painter.post_frame().expect("an offscreen post");
    let withheld = Instant::now() + Duration::from_millis(200);
    while Instant::now() < withheld {
        assert!(
            !adopting_painter.frame_serviced(&adopting_frame),
            "the adopting view's frame was serviced with its adoption unanswered"
        );
        assert!(
            !plain_painter.frame_serviced(&plain_frame),
            "the sibling's frame overtook a job parked ahead of it in the group's queue"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(adopting_fetcher.style_sheet_fetch_count(), 0);

    let deadline = Instant::now() + Duration::from_secs(30);
    while !(adopting_painter.frame_serviced(&adopting_frame)
        && plain_painter.frame_serviced(&plain_frame))
    {
        for _event in adopting_view.pump() {}
        assert!(Instant::now() < deadline, "the frames were never serviced");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(adopting_fetcher.style_sheet_fetch_count(), 1);
    adopting_painter
        .draw_posted(adopting_frame, false)
        .expect("the adopting view's frame draws");
    plain_painter
        .draw_posted(plain_frame, false)
        .expect("the sibling's frame draws");
}

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
