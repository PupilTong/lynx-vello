//! web-platform-tests over `terminus`: `css/css-flexbox`, `css/css-grid`
//! (with `grid-lanes/`) and `css/css-animations`.
//!
//! The suite files are vendored under `tests/wpt/suite/` at the commit
//! `tests/wpt/WPT_COMMIT` names (`tests/wpt/sync.py` refreshes them). Each
//! test file is classified, and the kinds the engine can execute without a
//! script engine are run:
//!
//! | kind | how it runs |
//! |---|---|
//! | check-layout (`check-layout-th.js`) | `wpt/check_layout.rs` over the layout |
//! | parsing / computed value helpers | `wpt/parsing.rs` over stylo and the cascade |
//! | reftest without script | `wpt/reftest.rs`, both sides rendered by the engine |
//! | crashtest without script | loaded, styled, laid out and rendered; passes if nothing panics |
//!
//! Everything else is reported with the reason it does not run: a script
//! the literal-call parser does not accept (`NEEDS_JS`), a manual test, a
//! visual test without a reference, or a paged-media (`print`) test.
//!
//! The run's results must equal `tests/wpt/expectations.tsv`, so a
//! regression and an unrecorded improvement both fail the test.
//! `WPT_UPDATE=1` rewrites that file and `docs/tracking/wpt-coverage.md`
//! (which also counts the hand-written ports in `tests/wpt/ported.tsv`).
//! `WPT_FILTER=<substring>` runs a subset (and never rewrites), `WPT_ROOT`
//! points at another checkout, and `WPT_JOBS` sets the worker count.

#![allow(clippy::float_cmp)]

#[path = "wpt/check_layout.rs"]
mod check_layout;
mod common;
#[path = "wpt/dom_script.rs"]
mod dom_script;
#[path = "wpt/loader.rs"]
mod loader;
#[path = "wpt/parsing.rs"]
mod parsing;
#[path = "wpt/reftest.rs"]
mod reftest;
#[path = "wpt/report.rs"]
mod report;
#[path = "wpt/script.rs"]
mod script;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use loader::{Paths, load};
use report::{Outcome, Status};
use terminus::link::render::gpu::Headless;

/// The suites, by the feature they cover.
pub(crate) const SUITES: &[(&str, &str)] = &[
    ("css-animations", "css/css-animations"),
    ("flexbox", "css/css-flexbox"),
    ("grid", "css/css-grid"),
    ("grid-lanes", "css/css-grid/grid-lanes"),
];

/// The feature a test path belongs to: the longest suite prefix.
pub(crate) fn feature_of(path: &str) -> &'static str {
    SUITES
        .iter()
        .filter(|(_, dir)| path.starts_with(&format!("{dir}/")))
        .max_by_key(|(_, dir)| dir.len())
        .map_or("other", |(feature, _)| feature)
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn is_test_file(relative: &str) -> bool {
    let segments: Vec<&str> = relative.split('/').collect();
    if segments
        .iter()
        .any(|s| matches!(*s, "support" | "reference" | "resources" | "test-plan"))
    {
        return false;
    }
    let name = segments.last().copied().unwrap_or_default();
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if !matches!(ext, "html" | "htm" | "xht" | "xhtml") {
        return false;
    }
    !(stem.ends_with("-ref")
        || stem.contains("-ref-")
        || stem.ends_with("-notref")
        || stem.starts_with("ref-")
        || stem.ends_with("-ref.tentative")
        || name.ends_with("-ref.html"))
}

fn discover(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = SUITES
        .iter()
        .map(|(_, dir)| root.join(dir))
        .filter(|dir| dir.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if is_test_file(&relative) {
                    out.push(relative);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

thread_local! {
    static PANIC_MESSAGE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn subtests_outcome(results: &[check_layout::Subtest]) -> Outcome {
    let passed = results.iter().filter(|r| r.is_ok()).count();
    let total = results.len();
    if total == 0 {
        return Outcome::new(Status::Fail, 0, 0, "no subtest ran");
    }
    let first_failure = results.iter().find_map(|r| r.as_ref().err().cloned());
    match first_failure {
        None => Outcome::new(Status::Pass, passed, total, ""),
        Some(message) => Outcome::new(Status::Fail, passed, total, &message),
    }
}

fn not_run(status: Status, message: &str) -> Outcome {
    Outcome::new(status, 0, 0, message)
}

#[allow(
    clippy::too_many_lines,
    reason = "one classification ladder, read top to bottom"
)]
fn run_one(paths: &Paths, relative: &str, gpu: &mut Option<Headless>) -> Outcome {
    let file = paths.root.join(relative);
    let name = relative.rsplit('/').next().unwrap_or_default();
    if name.contains("-manual") {
        return not_run(Status::Manual, "manual test");
    }
    if name.contains("-print") || relative.contains("/print/") {
        return not_run(Status::Print, "paged media");
    }
    let mut page = match load(paths, &file) {
        Ok(page) => page,
        Err(message) => return Outcome::new(Status::Fail, 0, 1, &message),
    };
    let waits = page
        .html_classes
        .iter()
        .any(|class| class == "reftest-wait" || class == "test-wait");
    let references: Vec<(String, String)> = page
        .links
        .iter()
        .filter(|(rel, _)| rel == "match" || rel == "mismatch")
        .cloned()
        .collect();
    let scripted = !page.scripts.iter().all(|s| s.trim().is_empty())
        || !page.script_srcs.is_empty()
        || page.body_onload.is_some()
        || page.has_event_handlers
        || waits;
    let harness = page
        .script_srcs
        .iter()
        .any(|src| src.ends_with("/resources/testharness.js"));

    // A reftest or crashtest whose script is within `dom_script`'s subset
    // runs that script first.
    let page_script = || -> Result<(), String> {
        if page.has_event_handlers {
            return Err("event handler attributes".into());
        }
        if let Some(src) = page.script_srcs.iter().find(|src| {
            !src.ends_with("/common/reftest-wait.js")
                && !src.ends_with("/common/rendering-utils.js")
        }) {
            return Err(format!("script helper {src}"));
        }
        dom_script::supported(&page.scripts, page.body_onload.as_deref())
    };
    let page_script = if scripted { page_script() } else { Ok(()) };
    let run_page_script = |page: &mut loader::Page| {
        if scripted {
            let scripts = page.scripts.clone();
            let onload = page.body_onload.clone();
            dom_script::run(page, &scripts, onload.as_deref()).map(|_| ())
        } else {
            Ok(())
        }
    };

    if name.contains("-crash") {
        if let Err(reason) = page_script {
            return not_run(
                Status::NeedsJs,
                &format!("crashtest drives the page from script: {reason}"),
            );
        }
        page.dom.layout();
        if let Err(message) = run_page_script(&mut page) {
            return Outcome::new(Status::Fail, 0, 1, &message);
        }
        let gpu = gpu.get_or_insert_with(|| prime_radiant::headless("wpt"));
        return match prime_radiant::capture_document(
            gpu,
            &mut page.dom,
            prime_radiant::peniko::Color::WHITE,
            &terminus::link::NoImages,
        ) {
            Ok(_) => Outcome::new(Status::Pass, 1, 1, ""),
            Err(error) => Outcome::new(Status::Fail, 0, 1, &error.to_string()),
        };
    }

    if !references.is_empty() {
        if let Err(reason) = page_script {
            return not_run(
                Status::NeedsJs,
                &format!("reftest drives the page from script: {reason}"),
            );
        }
        if scripted {
            page.dom.layout();
            if let Err(message) = run_page_script(&mut page) {
                return Outcome::new(Status::Fail, 0, 1, &message);
            }
        }
        return match reftest::run(gpu, paths, &file, &mut page, &references) {
            Ok(()) => Outcome::new(Status::Pass, 1, 1, ""),
            Err(message) => Outcome::new(Status::Fail, 0, 1, &message),
        };
    }

    if harness {
        if page
            .script_srcs
            .iter()
            .any(|src| src.ends_with("check-layout-th.js"))
        {
            let mut scripts = page.scripts.clone();
            if let Some(onload) = &page.body_onload {
                scripts.push(onload.clone());
            }
            if page.has_event_handlers {
                return not_run(Status::NeedsJs, "event handler attributes");
            }
            return match script::check_layout_selectors(&scripts) {
                Ok(selectors) if !selectors.is_empty() => {
                    subtests_outcome(&check_layout::run(&mut page, &selectors))
                }
                Ok(_) => not_run(Status::NeedsJs, "checkLayout is not called with a literal"),
                Err(reason) => {
                    // A page that changes the DOM before checking.
                    let only_harness = page.script_srcs.iter().all(|src| {
                        src.ends_with("/resources/testharness.js")
                            || src.ends_with("/resources/testharnessreport.js")
                            || src.ends_with("check-layout-th.js")
                    });
                    if !only_harness {
                        return not_run(Status::NeedsJs, &reason);
                    }
                    let scripts = page.scripts.clone();
                    let onload = page.body_onload.clone();
                    if dom_script::supported(&scripts, onload.as_deref()).is_err() {
                        return not_run(Status::NeedsJs, &reason);
                    }
                    page.dom.layout();
                    match dom_script::run(&mut page, &scripts, onload.as_deref()) {
                        Ok(results) => subtests_outcome(&results),
                        Err(message) => Outcome::new(Status::Fail, 0, 1, &message),
                    }
                }
            };
        }
        let known_srcs = page.script_srcs.iter().all(|src| {
            src.ends_with("/resources/testharness.js")
                || src.ends_with("/resources/testharnessreport.js")
                || src.ends_with("/css/support/parsing-testcommon.js")
                || src.ends_with("/css/support/computed-testcommon.js")
                || src.ends_with("/css/support/shorthand-testcommon.js")
        });
        if !known_srcs {
            let other = page
                .script_srcs
                .iter()
                .find(|src| !src.contains("testharness"))
                .cloned()
                .unwrap_or_default();
            return not_run(Status::NeedsJs, &format!("script helper {other}"));
        }
        if page.body_onload.is_some() || page.has_event_handlers {
            return not_run(Status::NeedsJs, "event handler attributes");
        }
        let mut calls = Vec::new();
        for source in &page.scripts {
            match script::parse_calls(source) {
                Ok(parsed) => calls.extend(parsed),
                Err(reason) => return not_run(Status::NeedsJs, &reason),
            }
        }
        if let Some(call) = calls
            .iter()
            .find(|call| !parsing::HELPERS.contains(&call.name.as_str()))
        {
            return not_run(Status::NeedsJs, &format!("calls `{}`", call.name));
        }
        let mut results = Vec::new();
        for call in &calls {
            if let Some(outcome) = parsing::run(&mut page, call) {
                results.extend(outcome);
            }
        }
        return subtests_outcome(&results);
    }

    if scripted {
        return not_run(Status::NeedsJs, "script without testharness");
    }
    not_run(Status::Visual, "visual test without a reference")
}

fn main() {
    // The libtest arguments cargo passes (`--quiet`, a filter) are ignored;
    // `WPT_FILTER` selects.
    let root = std::env::var_os("WPT_ROOT")
        .map_or_else(|| manifest_dir().join("tests/wpt/suite"), PathBuf::from);
    let filter = std::env::var("WPT_FILTER").ok();
    let update = std::env::var_os("WPT_UPDATE").is_some() && filter.is_none();
    let jobs = std::env::var("WPT_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(4, std::num::NonZero::get)
                .min(8)
        });

    let tests: Vec<String> = discover(&root)
        .into_iter()
        .filter(|path| filter.as_deref().is_none_or(|f| path.contains(f)))
        .collect();
    let paths = Paths { root: root.clone() };

    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into());
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()));
        PANIC_MESSAGE.with(|cell| {
            *cell.borrow_mut() = Some(format!("{message}{}", location.unwrap_or_default()));
        });
    }));

    let next = AtomicUsize::new(0);
    let results = Mutex::new(BTreeMap::new());
    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                let mut gpu: Option<Headless> = None;
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(relative) = tests.get(index) else {
                        break;
                    };
                    let outcome =
                        catch_unwind(AssertUnwindSafe(|| run_one(&paths, relative, &mut gpu)))
                            .unwrap_or_else(|_| {
                                // A panic may leave the GPU mid-frame.
                                gpu = None;
                                let message = PANIC_MESSAGE
                                    .with(|cell| cell.borrow_mut().take())
                                    .unwrap_or_default();
                                Outcome::new(Status::Panic, 0, 1, &message)
                            });
                    results
                        .lock()
                        .expect("results lock")
                        .insert(relative.clone(), outcome);
                }
            });
        }
    });
    let results = results.into_inner().expect("results lock");
    eprintln!(
        "wpt: {} tests in {:.1}s with {jobs} workers",
        results.len(),
        started.elapsed().as_secs_f64()
    );
    report::finish(&manifest_dir(), &results, update, filter.is_some());
}
