//! Every card in `packages/web-core-e2e-fixtures`, rendered and pinned.
//!
//! The corpus is `lynx-stack`'s web-core end-to-end suite, vendored as
//! ReactLynx source and compiled for the engine version this repository
//! targets. Upstream checks those cards with Playwright — a screenshot for
//! some, DOM assertions for the rest — against web-core. This suite checks
//! them against *this* engine, and a golden here is only ever a rendering
//! someone read the case for and judged right.
//!
//! **Comparing with web-core's screenshots directly would not say that.** The
//! two stacks rasterize text differently, upstream captures at its own
//! viewport, and several cases are about behaviour an image only indirectly
//! shows. What the case tests has to be read, and our answer judged, before a
//! picture of it is worth keeping.
//!
//! So each card sits in exactly one list:
//!
//! * [`verified!`] — the case was read, this engine's rendering of it is
//!   right, and the golden beside this file is that rendering. It fails when
//!   the rendering changes, which is then either a regression or a new
//!   judgement to make.
//! * [`pending!`] — nobody has judged it yet, or the rendering is wrong and
//!   the reason says what about it. There is deliberately no golden: an image
//!   of a wrong rendering is worse than none, because the next reader would
//!   take it for a decision. `no_pending_case_has_a_golden` holds that line.
//!
//! `unlisted_cases_are_an_error` fails when the corpus gains a card that is in
//! neither list, so the suite cannot quietly stop covering the corpus.
//!
//! Accepting a rendering is `FLASHBULB_UPDATE_SNAPSHOTS=1` on that one test,
//! *after* reading the case and deciding — and moving it to [`verified!`] in
//! the same change, or `no_pending_case_has_a_golden` fails.
//!
//! Requires the corpus: `pnpm --filter web-core-e2e-fixtures build`. Cards
//! fetch their own bitmaps and fonts through `file:` URLs the build bakes in,
//! so nothing has to be served for them to resolve.
// These integration tests use native threads and GPU capture.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use flashbulb::{Image, Screenshots};
use url::Url;

/// The viewport upstream's Playwright project uses, and the one every case was
/// written against.
const VIEWPORT: (f32, f32) = (393.0, 727.0);
/// Time a card is given to boot its background thread.
const BOOT_DEADLINE: Duration = Duration::from_secs(30);
/// Time to keep pumping after boot, so a `useEffect` patch, a lazy child and
/// an image read all land before the capture.
const SETTLE: Duration = Duration::from_millis(500);

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> sits two levels under the repository")
        .to_path_buf()
}

fn corpus() -> PathBuf {
    repository().join("packages/web-core-e2e-fixtures/dist")
}

fn screenshots() -> Screenshots {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR"))
}

fn golden_path(case: &str) -> PathBuf {
    screenshots().path(&["web-core-e2e", case])
}

/// Boots one card and captures its first screen.
async fn first_screen(case: &str) -> Image {
    let path = corpus().join(format!("{case}.web.bundle"));
    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}\nrun `pnpm --filter web-core-e2e-fixtures build` first",
            path.display()
        )
    });
    let input = Url::parse(&format!("app:///{case}.web.bundle")).unwrap();
    let page = PageSource::from_bytes(&input, &bytes).expect("decode the compiled card");
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);

    let (width, height) = VIEWPORT;
    let screen = bobcat_core::ScreenMetrics::for_viewport(width, height, 1.0);
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Auto)
        .await
        .unwrap();
    let mut view = group
        .create_lynx_view(
            width,
            height,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(screen),
        )
        .unwrap();
    let mut painter = Painter::new(DrawTarget::Offscreen, width, height, 1.0)
        .await
        .unwrap();
    painter.attach(&view).unwrap();

    let deadline = Instant::now() + BOOT_DEADLINE;
    let mut settling_until = None;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => settling_until = Some(Instant::now() + SETTLE),
                EngineEvent::ConsoleMessage { level, message } => {
                    eprintln!("[{level}] {message}");
                }
                // A card that reports an error still has a first screen, and
                // whether that screen is right is what the reader judged, so
                // nothing here fails the test on its own.
                other => eprintln!("[event] {other:?}"),
            }
        }
        painter.pump().unwrap();
        match settling_until {
            Some(until) if Instant::now() >= until => break,
            Some(_) => {}
            None => assert!(Instant::now() < deadline, "{case}: the card never booted"),
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let shot = painter.capture().expect("capture the first screen");
    Image::from_rgba8(shot.size.width, shot.size.height, shot.pixels).expect("captured RGBA image")
}

/// Renders a card and holds it to the rendering that was judged right.
async fn renders_as_judged(case: &str) {
    let actual = first_screen(case).await;
    assert!(
        golden_path(case).exists() || std::env::var(flashbulb::UPDATE_ENV).as_deref() == Ok("1"),
        "{case}: no golden. A verified case keeps the rendering someone judged \
         right; accept one with {}=1 only after reading the case",
        flashbulb::UPDATE_ENV,
    );
    screenshots().assert_matches(&["web-core-e2e", case], &actual);
}

macro_rules! verified {
    ($($name:ident => $case:literal,)*) => {
        /// Cards whose rendering has been read against the case and judged right.
        const VERIFIED: &[&str] = &[$($case,)*];
        $(
            #[tokio::test]
            async fn $name() {
                renders_as_judged($case).await;
            }
        )*
    };
}

macro_rules! pending {
    ($($case:literal => $reason:literal,)*) => {
        /// Cards nobody has judged yet, each with what stands in the way.
        const PENDING: &[(&str, &str)] = &[$(($case, $reason),)*];
    };
}

include!("web_core_e2e/verified.rs");
include!("web_core_e2e/pending.rs");

/// A pending case must have no golden: a picture of a rendering nobody has
/// judged would be read as a decision by whoever comes next.
#[test]
fn no_pending_case_has_a_golden() {
    let accepted: Vec<&str> = PENDING
        .iter()
        .map(|(case, _)| *case)
        .filter(|case| golden_path(case).exists())
        .collect();
    assert!(
        accepted.is_empty(),
        "these cases are pending but have a golden — move them to `verified!` \
         if the rendering was judged right, or delete the golden: {accepted:?}",
    );
}

/// Every card in the corpus is in one of the two lists, and nothing names a
/// card that is gone.
#[test]
fn unlisted_cases_are_an_error() {
    let dist = corpus();
    let Ok(entries) = std::fs::read_dir(&dist) else {
        eprintln!(
            "{}: no corpus; run `pnpm --filter web-core-e2e-fixtures build`",
            dist.display()
        );
        return;
    };
    let built: BTreeSet<String> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".web.bundle"))
                .map(str::to_owned)
        })
        .collect();
    let listed: BTreeSet<String> = VERIFIED
        .iter()
        .copied()
        .chain(PENDING.iter().map(|(case, _)| *case))
        .map(str::to_owned)
        .collect();

    let unlisted: Vec<&String> = built.difference(&listed).collect();
    let gone: Vec<&String> = listed.difference(&built).collect();
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "the corpus and this suite disagree.\n  cards with no entry: {unlisted:?}\n  \
         entries with no card: {gone:?}",
    );
}

/// Writes every card's current rendering somewhere to look at, which is how a
/// batch gets read before anything is accepted. Never writes a golden.
#[tokio::test]
#[ignore = "review aid: WEB_CORE_E2E_DUMP=<dir> cargo test -- --ignored dump_every_rendering"]
async fn dump_every_rendering() {
    let Ok(directory) = std::env::var("WEB_CORE_E2E_DUMP") else {
        panic!("set WEB_CORE_E2E_DUMP to the directory to write into");
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).expect("create the dump directory");
    let mut cases: Vec<&str> = VERIFIED.to_vec();
    cases.extend(PENDING.iter().map(|(case, _)| *case));
    cases.sort_unstable();
    for case in cases {
        let image = first_screen(case).await;
        image
            .write_png(directory.join(format!("{case}.png")))
            .expect("write the rendering");
    }
}
