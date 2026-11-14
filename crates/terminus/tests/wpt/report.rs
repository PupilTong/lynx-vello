//! Results, the expectations file, and the coverage report.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::{SUITES, feature_of};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Status {
    Pass,
    Fail,
    Panic,
    NeedsJs,
    Manual,
    Visual,
    Print,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Panic => "PANIC",
            Status::NeedsJs => "NEEDS_JS",
            Status::Manual => "MANUAL",
            Status::Visual => "VISUAL",
            Status::Print => "PRINT",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "PASS" => Status::Pass,
            "FAIL" => Status::Fail,
            "PANIC" => Status::Panic,
            "NEEDS_JS" => Status::NeedsJs,
            "MANUAL" => Status::Manual,
            "VISUAL" => Status::Visual,
            "PRINT" => Status::Print,
            _ => return None,
        })
    }

    fn ran(self) -> bool {
        matches!(self, Status::Pass | Status::Fail | Status::Panic)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Outcome {
    pub(crate) status: Status,
    pub(crate) passed: usize,
    pub(crate) total: usize,
    pub(crate) message: String,
}

impl Outcome {
    pub(crate) fn new(status: Status, passed: usize, total: usize, message: &str) -> Self {
        let message: String = message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('\t', " ")
            .chars()
            .take(200)
            .collect();
        Self {
            status,
            passed,
            total,
            message,
        }
    }

    fn key(&self) -> (Status, usize, usize) {
        (self.status, self.passed, self.total)
    }
}

const HEADER: &str = "path\tstatus\tsubtests\tmessage";

fn serialize(results: &BTreeMap<String, Outcome>) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for (path, outcome) in results {
        let _ = writeln!(
            out,
            "{path}\t{}\t{}/{}\t{}",
            outcome.status.as_str(),
            outcome.passed,
            outcome.total,
            outcome.message
        );
    }
    out
}

fn parse(text: &str) -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 3 {
            continue;
        }
        let Some(status) = Status::parse(fields[1]) else {
            continue;
        };
        let (passed, total) = fields[2].split_once('/').unwrap_or(("0", "0"));
        out.insert(
            fields[0].to_owned(),
            Outcome::new(
                status,
                passed.parse().unwrap_or(0),
                total.parse().unwrap_or(0),
                fields.get(3).copied().unwrap_or_default(),
            ),
        );
    }
    out
}

/// One row of `ported.tsv`: a hand-written port of a testharness file.
struct Port {
    status: String,
    passed: usize,
    total: usize,
}

fn ported(manifest: &Path) -> BTreeMap<String, Port> {
    let Ok(text) = std::fs::read_to_string(manifest.join("tests/wpt/ported.tsv")) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 3 {
            continue;
        }
        let (passed, total) = fields[2].split_once('/').unwrap_or(("0", "0"));
        out.insert(
            fields[0].to_owned(),
            Port {
                status: fields[1].to_owned(),
                passed: passed.trim().parse().unwrap_or(0),
                total: total.trim().parse().unwrap_or(0),
            },
        );
    }
    out
}

/// A likely cause, named by what a failing test's source uses: where to
/// look, not a diagnosis. `unsupported` marks what `terminus` does not
/// implement at all (outside the `lynx` grammar, deferred, or absent from
/// `seldon`), as opposed to a feature it has and may get wrong.
struct Cause {
    name: &'static str,
    unsupported: bool,
    needles: &'static [&'static str],
}

const CAUSES: &[Cause] = &[
    Cause {
        name: "inline-level display (inline-flex/-grid/-block)",
        unsupported: true,
        needles: &[
            "inline-flex",
            "inline-grid",
            "inline-block",
            "inline-grid-lanes",
        ],
    },
    Cause {
        name: "writing-mode / vertical text",
        unsupported: true,
        needles: &["writing-mode", "text-orientation"],
    },
    Cause {
        name: "subgrid",
        unsupported: true,
        needles: &["subgrid"],
    },
    Cause {
        name: "float",
        unsupported: true,
        needles: &["float:", "float :"],
    },
    Cause {
        name: "table layout",
        unsupported: true,
        needles: &["display: table", "display:table", "<table"],
    },
    Cause {
        name: "multicol",
        unsupported: true,
        needles: &[
            "column-count",
            "column-width",
            " columns:",
            ";columns:",
            "{columns:",
        ],
    },
    Cause {
        name: "images / form controls / svg / iframe",
        unsupported: true,
        needles: &[
            "<img",
            "<input",
            "<button",
            "<select",
            "<textarea",
            "<canvas",
            "<video",
            "<iframe",
            "<svg",
            "<object",
            "url(",
        ],
    },
    Cause {
        name: "::before / ::after / ::first-line / ::marker",
        unsupported: true,
        needles: &[
            "::before",
            "::after",
            ":before",
            ":after",
            "::first-line",
            "::marker",
        ],
    },
    Cause {
        name: "`grid-lanes-direction` (not implemented)",
        unsupported: true,
        needles: &["grid-lanes-direction"],
    },
    Cause {
        name: "`flex-wrap: balance`",
        unsupported: true,
        needles: &["flex-wrap: balance", "flex-wrap:balance"],
    },
    Cause {
        name: "position: absolute (static position under the block emulation)",
        unsupported: false,
        needles: &["position: absolute", "position:absolute"],
    },
    Cause {
        name: "baseline alignment",
        unsupported: false,
        needles: &["baseline"],
    },
    Cause {
        name: "intrinsic keywords",
        unsupported: false,
        needles: &["min-content", "max-content", "fit-content"],
    },
    Cause {
        name: "aspect-ratio",
        unsupported: false,
        needles: &["aspect-ratio"],
    },
    Cause {
        name: "margin between block siblings (no collapsing)",
        unsupported: false,
        needles: &["<p>", "<p "],
    },
];

/// The test's source with its references' and linked sheets' text: a
/// reference drawn with a float fails a test that uses none.
fn source_with_links(root: &Path, path: &str) -> String {
    let file = root.join(path);
    let mut source = std::fs::read_to_string(&file)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut linked = String::new();
    for tag in source.split("<link").skip(1) {
        let tag = tag.split('>').next().unwrap_or_default();
        if !(tag.contains("match") || tag.contains("stylesheet")) {
            continue;
        }
        let Some(href) = tag.split("href=").nth(1) else {
            continue;
        };
        let href = href.trim_start_matches(['"', '\'']);
        let href: String = href
            .chars()
            .take_while(|c| !matches!(c, '"' | '\'' | ' ' | '>'))
            .collect();
        let target = match href.strip_prefix('/') {
            Some(rooted) => root.join(rooted),
            None => file.parent().map(|dir| dir.join(&href)).unwrap_or_default(),
        };
        if let Ok(text) = std::fs::read_to_string(target) {
            linked.push_str(&text.to_ascii_lowercase());
        }
    }
    source.push_str(&linked);
    source
}

fn causes(root: &Path, path: &str) -> Vec<&'static Cause> {
    let source = source_with_links(root, path);
    CAUSES
        .iter()
        .filter(|cause| cause.needles.iter().any(|needle| source.contains(needle)))
        .collect()
}

/// Groups the runner's reason for not running a script-driven file.
fn js_category(message: &str) -> String {
    if message.starts_with("reftest drives") {
        "reftest changes the page from script before the screenshot".into()
    } else if message.starts_with("crashtest drives") {
        "crashtest changes the page from script".into()
    } else if message.starts_with("script does more than checkLayout") {
        "check-layout page changes the DOM or styles before checking".into()
    } else if let Some(helper) = message.strip_prefix("script helper ") {
        format!(
            "uses the helper library `{}`",
            helper.rsplit('/').next().unwrap_or(helper)
        )
    } else if let Some(name) = message.strip_prefix("calls `") {
        format!("calls `{}`", name.trim_end_matches('`'))
    } else if message.starts_with("script without testharness") {
        "script-driven page without testharness".into()
    } else if message.starts_with("event handler") {
        "event handler attributes".into()
    } else {
        "test logic written in JS (functions, loops, variables, DOM calls)".into()
    }
}

#[derive(Default)]
struct Counts {
    files: usize,
    ran: usize,
    pass: usize,
    fail: usize,
    panic: usize,
    needs_js: usize,
    manual: usize,
    visual: usize,
    print: usize,
    subtests_pass: usize,
    subtests_total: usize,
    ported_files: usize,
    ported_pass: usize,
    /// Failing files that use a feature `terminus` does not implement.
    fail_unsupported: usize,
}

fn percent(numerator: usize, denominator: usize) -> String {
    if denominator == 0 {
        "—".into()
    } else {
        #[allow(clippy::cast_precision_loss)]
        let ratio = numerator as f64 * 100.0 / denominator as f64;
        format!("{ratio:.1}%")
    }
}

fn kind_of(root: &Path, path: &str) -> &'static str {
    let source = std::fs::read_to_string(root.join(path)).unwrap_or_default();
    if path.contains("-crash") {
        "crashtest"
    } else if source.contains("rel=\"match\"")
        || source.contains("rel='match'")
        || source.contains("rel=match")
        || source.contains("rel=\"mismatch\"")
    {
        "reftest"
    } else if source.contains("check-layout-th.js") {
        "check-layout"
    } else if source.contains("testharness.js") {
        "testharness"
    } else {
        "other"
    }
}

#[allow(clippy::too_many_lines, reason = "the report, section by section")]
fn markdown(root: &Path, manifest: &Path, results: &BTreeMap<String, Outcome>) -> String {
    let ports = ported(manifest);
    let commit = std::fs::read_to_string(manifest.join("tests/wpt/WPT_COMMIT"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    let mut by_feature: BTreeMap<&str, Counts> = BTreeMap::new();
    let mut by_kind: BTreeMap<(&str, &str), (usize, usize, usize)> = BTreeMap::new();
    let mut cause_counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut js_reasons: BTreeMap<(&str, String), usize> = BTreeMap::new();
    for (path, outcome) in results {
        let feature = feature_of(path);
        let counts = by_feature.entry(feature).or_default();
        counts.files += 1;
        let mut status = outcome.status;
        let (mut passed, mut total) = (outcome.passed, outcome.total);
        if status == Status::NeedsJs
            && let Some(port) = ports.get(path)
            && port.status != "NA"
        {
            counts.ported_files += 1;
            passed = port.passed;
            total = port.total;
            status = if port.status == "PASS" {
                counts.ported_pass += 1;
                Status::Pass
            } else if port.status == "PARTIAL" {
                // Every ported subtest passes, but not every subtest was
                // portable: the file is run, not passed.
                Status::Fail
            } else {
                Status::Fail
            };
        }
        if status.ran() {
            counts.ran += 1;
        }
        let kind = kind_of(root, path);
        let entry = by_kind.entry((feature, kind)).or_default();
        entry.0 += 1;
        if status.ran() {
            entry.1 += 1;
        }
        if status == Status::Pass {
            entry.2 += 1;
        }
        match status {
            Status::Pass => counts.pass += 1,
            Status::Fail => counts.fail += 1,
            Status::Panic => counts.panic += 1,
            Status::NeedsJs => {
                counts.needs_js += 1;
                let reason = js_category(&outcome.message);
                *js_reasons.entry((feature, reason)).or_default() += 1;
            }
            Status::Manual => counts.manual += 1,
            Status::Visual => counts.visual += 1,
            Status::Print => counts.print += 1,
        }
        counts.subtests_pass += passed;
        counts.subtests_total += total;
        if matches!(status, Status::Fail | Status::Panic) && !ports.contains_key(path) {
            let found = causes(root, path);
            if found.iter().any(|cause| cause.unsupported) {
                counts.fail_unsupported += 1;
            }
            for cause in found {
                *cause_counts.entry((feature, cause.name)).or_default() += 1;
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# WPT coverage of `terminus`\n\n\
         Generated by `WPT_UPDATE=1 cargo test -p terminus --test wpt` — do not edit by hand.\n\
         Suite: web-platform-tests `{commit}`, vendored under `crates/terminus/tests/wpt/suite/`.\n\
         Per-file results are in `crates/terminus/tests/wpt/expectations.tsv`; the runner and\n\
         its adaptations (notably the harness sheet that lays HTML block elements out as\n\
         `display: linear`, since the engine has no block flow) are described in\n\
         `crates/terminus/tests/wpt.rs` and `crates/terminus/tests/wpt/loader.rs`.\n\n\
         **Coverage** is files passing over all test files in the suite. A file passes when\n\
         every subtest in it passes. *Run* counts the files the runner (or a hand-written\n\
         port in `crates/terminus/tests/wpt_css_animations.rs`) executed; the rest need a\n\
         script engine, a person, or paged media, which `terminus` alone does not have.\n"
    );
    let _ = writeln!(
        out,
        "## Summary\n\n\
         | feature | files | run | pass | fail | panic | needs JS | manual / visual / print | coverage (pass / files) | pass / run | subtests pass / total |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
    );
    let mut all = Counts::default();
    for &(feature, _) in SUITES {
        let Some(c) = by_feature.get(feature) else {
            continue;
        };
        let _ = writeln!(
            out,
            "| {feature} | {} | {} | {} | {} | {} | {} | {} / {} / {} | **{}** | {} | {} / {} ({}) |",
            c.files,
            c.ran,
            c.pass,
            c.fail,
            c.panic,
            c.needs_js,
            c.manual,
            c.visual,
            c.print,
            percent(c.pass, c.files),
            percent(c.pass, c.ran),
            c.subtests_pass,
            c.subtests_total,
            percent(c.subtests_pass, c.subtests_total),
        );
        all.files += c.files;
        all.ran += c.ran;
        all.pass += c.pass;
        all.fail += c.fail;
        all.panic += c.panic;
        all.needs_js += c.needs_js;
        all.manual += c.manual;
        all.visual += c.visual;
        all.print += c.print;
        all.subtests_pass += c.subtests_pass;
        all.subtests_total += c.subtests_total;
        all.fail_unsupported += c.fail_unsupported;
    }
    let mut scoped = String::from(
        "### Within what `terminus` implements\n\n\
         The same run with the failing files set aside whose source uses a feature `terminus` \
         does not implement at all (inline-level `display`, `writing-mode`, `float`, tables, \
         multicol, `subgrid`, replaced elements and form controls, generated-content \
         pseudo-elements, and the grid-lanes and flexbox properties named in the table of causes below; \
         the test, its references and its linked sheets are searched). The detection is a text match, so \
         this is an estimate of how well the implemented features hold up, not a second \
         coverage number.\n\n\
         | feature | run | of which fail on an unimplemented feature | remaining run | pass | pass / remaining |\n\
         |---|---:|---:|---:|---:|---:|\n",
    );
    for &(feature, _) in SUITES {
        if let Some(c) = by_feature.get(feature) {
            let remaining = c.ran - c.fail_unsupported;
            let _ = writeln!(
                scoped,
                "| {feature} | {} | {} | {remaining} | {} | {} |",
                c.ran,
                c.fail_unsupported,
                c.pass,
                percent(c.pass, remaining)
            );
        }
    }
    let remaining = all.ran - all.fail_unsupported;
    let _ = writeln!(
        scoped,
        "| **all** | {} | {} | {remaining} | {} | {} |\n",
        all.ran,
        all.fail_unsupported,
        all.pass,
        percent(all.pass, remaining)
    );
    let _ = writeln!(
        out,
        "| **all** | {} | {} | {} | {} | {} | {} | {} / {} / {} | **{}** | {} | {} / {} ({}) |\n",
        all.files,
        all.ran,
        all.pass,
        all.fail,
        all.panic,
        all.needs_js,
        all.manual,
        all.visual,
        all.print,
        percent(all.pass, all.files),
        percent(all.pass, all.ran),
        all.subtests_pass,
        all.subtests_total,
        percent(all.subtests_pass, all.subtests_total),
    );
    if let Some(c) = by_feature.get("css-animations") {
        let _ = writeln!(
            out,
            "css-animations counts {} testharness files ported by hand ({} of them passing in full); \
             the rest of its run files are reftests, crashtests and parsing tests.\n",
            c.ported_files, c.ported_pass
        );
    }
    out.push_str(&scoped);

    let _ = writeln!(
        out,
        "## By test kind\n\n| feature | kind | files | run | pass |\n|---|---|---:|---:|---:|"
    );
    for ((feature, kind), (files, ran, pass)) in &by_kind {
        let _ = writeln!(out, "| {feature} | {kind} | {files} | {ran} | {pass} |");
    }

    let _ = writeln!(
        out,
        "\n## What the failing files use\n\n\
         Counted from each failing file's source (a file can count under several rows). This is\n\
         where to look, not a diagnosis.\n\n| feature | uses | failing files |\n|---|---|---:|"
    );
    let mut causes: Vec<_> = cause_counts.into_iter().collect();
    causes.sort_by(|a, b| a.0.0.cmp(b.0.0).then(b.1.cmp(&a.1)));
    for ((feature, cause), count) in causes {
        let _ = writeln!(out, "| {feature} | {cause} | {count} |");
    }

    let _ = writeln!(
        out,
        "\n## Why files need a script engine\n\n| feature | reason | files |\n|---|---|---:|"
    );
    let mut reasons: Vec<_> = js_reasons.into_iter().collect();
    reasons.sort_by(|a, b| a.0.0.cmp(b.0.0).then(b.1.cmp(&a.1)));
    for ((feature, reason), count) in reasons {
        let _ = writeln!(out, "| {feature} | {reason} | {count} |");
    }
    out.push_str(LIMITATIONS);
    out
}

const LIMITATIONS: &str = "
## What the runner cannot tell apart

A failing file is not always a flex, grid or animation bug. The runner loads pages the
way a browser would, with these differences that come from the engine, not from the test:

- **No block flow.** HTML block elements are laid out as `display: linear` (column), so
  block siblings stack and stretch, but margins between them do not collapse, and an
  absolutely positioned box with auto insets takes its static position at the start of
  its container instead of below the block before it. Many reftests place a red
  `position: absolute` box behind the subject after a `<p>`; they fail for this reason.
- **No images.** Pages are rendered with no image source, so `<img>` and `url()`
  backgrounds draw nothing.
- **Scripts.** Only literal calls to the WPT helpers listed in `tests/wpt.rs` and the
  straight-line DOM changes `tests/wpt/dom_script.rs` lists run.
- **Fonts.** The vendored Ahem and Roboto faces are registered and Roboto is the default
  family, in place of the browser's serif face.
- **Raster noise.** Reftests ignore a per-channel difference of up to 2 levels on top of
  each file's own `fuzzy` allowance.
";

fn summary(results: &BTreeMap<String, Outcome>) -> String {
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (path, outcome) in results {
        *counts
            .entry((feature_of(path), outcome.status.as_str()))
            .or_default() += 1;
    }
    let mut out = String::new();
    for ((feature, status), count) in counts {
        let _ = writeln!(out, "  {feature:<15} {status:<9} {count}");
    }
    out
}

pub(crate) fn finish(
    manifest: &Path,
    results: &BTreeMap<String, Outcome>,
    update: bool,
    filtered: bool,
) {
    let expectations_path = manifest.join("tests/wpt/expectations.tsv");
    eprint!("{}", summary(results));
    if update {
        std::fs::write(&expectations_path, serialize(results)).expect("write expectations");
        let root = std::env::var_os("WPT_ROOT").map_or_else(
            || manifest.join("tests/wpt/suite"),
            std::path::PathBuf::from,
        );
        let report = markdown(&root, manifest, results);
        let docs = manifest.join("../../docs/tracking/wpt-coverage.md");
        std::fs::write(&docs, report).expect("write coverage report");
        eprintln!(
            "wpt: wrote {} and {}",
            expectations_path.display(),
            docs.display()
        );
        return;
    }
    if filtered {
        for (path, outcome) in results {
            eprintln!(
                "{:<9} {}/{} {path}  {}",
                outcome.status.as_str(),
                outcome.passed,
                outcome.total,
                outcome.message
            );
        }
    }
    let expected = std::fs::read_to_string(&expectations_path)
        .map(|text| parse(&text))
        .unwrap_or_default();
    let mut differences = Vec::new();
    for (path, outcome) in results {
        match expected.get(path) {
            Some(want) if want.key() == outcome.key() => {}
            Some(want) => differences.push(format!(
                "{path}: expected {} {}/{}, got {} {}/{} {}",
                want.status.as_str(),
                want.passed,
                want.total,
                outcome.status.as_str(),
                outcome.passed,
                outcome.total,
                outcome.message
            )),
            None => differences.push(format!(
                "{path}: not in expectations.tsv, got {}",
                outcome.status.as_str()
            )),
        }
    }
    if !filtered {
        for path in expected.keys().filter(|path| !results.contains_key(*path)) {
            differences.push(format!("{path}: in expectations.tsv but not in the suite"));
        }
    }
    if !differences.is_empty() {
        for line in differences.iter().take(60) {
            eprintln!("wpt: {line}");
        }
        eprintln!(
            "wpt: {} results differ from tests/wpt/expectations.tsv; \
             rerun with WPT_UPDATE=1 if the change is intended",
            differences.len()
        );
        std::process::exit(1);
    }
    eprintln!("wpt: all {} results match expectations", results.len());
}
