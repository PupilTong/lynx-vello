//! Browser-referenced, pure-`<div>` CSS paint screenshot tests.
//!
//! The matrix retains all 1,000 independent 128×128 probes.  The 666 probes
//! that pixelmatch Chromium own browser references.  Another 145 W3C-correct
//! cases exercise rasterization/sampling differences or CSS-permitted UA
//! choices against native Pulsar/Parley snapshots.  The other 189 audited
//! differences remain ignored fixtures.  Up to twenty-five active documents
//! share one isolated 640×640 Vello atlas readback; a full audit compares all
//! 1,000 against temporary Chromium references.

#[path = "support/html.rs"]
mod html;
#[path = "paint_common/mod.rs"]
mod paint_common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use dom::render::gpu::Headless;
use dom::vello::Scene;
use dom::vello::kurbo::{Affine, Rect};
use dom::vello::peniko::{BlendMode, Color, Compose, Fill, Mix};
use flashbulb::{CompareOptions, Image, compare, screenshots_in};

const CASE_COUNT: usize = 1_000;
const CELL_SIZE: u32 = 128;
const CELL_SIZE_F32: f32 = 128.0;
const CELL_SIZE_F64: f64 = 128.0;
const GRID: usize = 5;
const CASES_PER_SHARD: usize = GRID * GRID;
const SHARD_COUNT: usize = CASE_COUNT / CASES_PER_SHARD;
const ATLAS_SIZE: u32 = CELL_SIZE * 5;
const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");
const AUDIT_ENV: &str = "CSS_PAINT_AUDIT";
const REFERENCE_DIR_ENV: &str = "CSS_PAINT_REFERENCE_DIR";
const UPDATE_NATIVE_ENV: &str = "CSS_PAINT_UPDATE_NATIVE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DifferenceKind {
    RasterOrSampling,
    UaChoice,
    W3cGap,
    NonW3cCompatibility,
}

impl DifferenceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RasterOrSampling => "w3c-correct-raster-or-sampling",
            Self::UaChoice => "w3c-correct-ua-choice",
            Self::W3cGap => "w3c-gap",
            Self::NonW3cCompatibility => "non-w3c-compatibility",
        }
    }

    const fn uses_native_snapshot(self) -> bool {
        matches!(self, Self::RasterOrSampling | Self::UaChoice)
    }
}

#[derive(Clone, Copy, Debug)]
enum Expectation {
    BrowserMatch,
    NativeSnapshot {
        kind: DifferenceKind,
        issue: &'static str,
    },
    Skip {
        kind: DifferenceKind,
        issue: &'static str,
    },
}

impl Expectation {
    const fn difference(self) -> Option<(DifferenceKind, &'static str)> {
        match self {
            Self::BrowserMatch => None,
            Self::NativeSnapshot { kind, issue } | Self::Skip { kind, issue } => {
                Some((kind, issue))
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CssPaintCase {
    name: &'static str,
    category: &'static str,
    source: &'static str,
    fragment: &'static str,
    expectation: Expectation,
}

macro_rules! css_paint_case_tests {
    (
        browser_matches {
            $( $browser_index:literal => $browser_test:ident; )*
        }
        native_snapshots {
            $( $native_index:literal => $native_test:ident; )*
        }
        skips {
            $( $skip_index:literal => $skip_test:ident, $reason:literal; )*
        }
    ) => {
        $(
            #[test]
            fn $browser_test() {
                crate::run_browser_match_case($browser_index);
            }
        )*
        $(
            #[test]
            fn $native_test() {
                crate::run_native_snapshot_case($native_index);
            }
        )*
        $(
            #[test]
            #[ignore = $reason]
            fn $skip_test() {
                crate::run_skipped_case($skip_index);
            }
        )*
    };
}

mod generated {
    use super::{CssPaintCase, DifferenceKind, Expectation};

    include!("generated/css_paint_cases.rs");
}

#[derive(Debug)]
enum ShardOutcome {
    Ready(Image),
    Failed(Arc<str>),
}

static GPU: OnceLock<Mutex<Headless>> = OnceLock::new();
static SHARDS: [OnceLock<ShardOutcome>; SHARD_COUNT] = [const { OnceLock::new() }; SHARD_COUNT];
static AUDIT_WRITE: Mutex<()> = Mutex::new(());
static AUDIT_TARGET: OnceLock<PathBuf> = OnceLock::new();

fn run_browser_match_case(index: usize) {
    assert_flashbulb_update_disabled();
    let case = &generated::CASES[index];
    assert!(
        matches!(case.expectation, Expectation::BrowserMatch),
        "{}: generated browser test is not classified as a browser match",
        case.name
    );
    assert!(
        !native_update_enabled(),
        "{UPDATE_NATIVE_ENV}=1 only accepts tests filtered by `css_native_`; \
         browser references are never writable from Rust"
    );
    compare_case(index);
}

fn run_native_snapshot_case(index: usize) {
    assert_flashbulb_update_disabled();
    let case = &generated::CASES[index];
    assert!(
        matches!(case.expectation, Expectation::NativeSnapshot { .. }),
        "{}: generated native test is not classified as a native snapshot",
        case.name
    );
    compare_case(index);
}

fn run_skipped_case(index: usize) {
    assert_flashbulb_update_disabled();
    let case = &generated::CASES[index];
    assert!(
        matches!(case.expectation, Expectation::Skip { .. }),
        "{}: generated ignored test is not classified as a skip",
        case.name
    );

    if std::env::var_os(AUDIT_ENV).is_some() {
        assert!(
            std::env::var_os(REFERENCE_DIR_ENV).is_some(),
            "{REFERENCE_DIR_ENV} must point at temporary all-case references \
             when auditing ignored CSS-paint fixtures"
        );
        compare_case(index);
    } else {
        assert!(
            !case.fragment.is_empty(),
            "{}: empty skipped fragment",
            case.name
        );
    }
}

fn compare_case(index: usize) {
    let case = &generated::CASES[index];
    let audit = std::env::var_os(AUDIT_ENV);
    let update_native = native_update_enabled();
    assert!(
        audit.is_none() || !update_native,
        "{AUDIT_ENV} and {UPDATE_NATIVE_ENV}=1 are mutually exclusive"
    );
    let shard = index / CASES_PER_SHARD;
    let slot = index % CASES_PER_SHARD;
    let actual_atlas = match SHARDS[shard].get_or_init(|| render_shard(shard)) {
        ShardOutcome::Ready(image) => image,
        ShardOutcome::Failed(error) => panic!("{}: shard {shard:02} failed: {error}", case.name),
    };
    let actual = crop_cell(actual_atlas, slot);

    if audit.is_none()
        && update_native
        && matches!(case.expectation, Expectation::NativeSnapshot { .. })
    {
        update_native_reference(case.name, &actual);
        return;
    }

    let (golden, reference_owner) = reference_path(case);
    assert!(
        golden.exists(),
        "{}: missing {reference_owner} golden {}",
        case.name,
        golden.display()
    );
    let expected = Image::read_png(&golden)
        .unwrap_or_else(|error| panic!("{}: cannot read {}: {error}", case.name, golden.display()));
    assert_eq!(
        (expected.width(), expected.height()),
        (CELL_SIZE, CELL_SIZE),
        "{}: {reference_owner} golden must be {CELL_SIZE}×{CELL_SIZE}",
        case.name
    );
    let comparison = compare(&expected, &actual, CompareOptions::default());

    if let Some(report) = std::env::var_os(AUDIT_ENV) {
        append_audit(Path::new(&report), index, case, &comparison);
        return;
    }

    if comparison.is_match() {
        return;
    }
    let artifacts = write_artifacts(case.name, &expected, &actual, &comparison.diff);
    panic!(
        "{} [{}] differs from {reference_owner}: {} of {} pixels ({:.4}%), \
         {} anti-aliased pixels ignored; source {}\n{}",
        case.name,
        case.category,
        comparison.diff_pixels,
        expected.pixel_count(),
        comparison.diff_ratio() * 100.0,
        comparison.anti_aliased_pixels,
        case.source,
        artifacts
    );
}

fn reference_path(case: &CssPaintCase) -> (PathBuf, &'static str) {
    let screenshots = screenshots_in(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !screenshots.is_updating(),
        "CSS atlas references reject FLASHBULB_UPDATE_SNAPSHOTS; browser \
         references come from Playwright and native references require \
         {UPDATE_NATIVE_ENV}=1"
    );

    if std::env::var_os(AUDIT_ENV).is_some() {
        let directory = std::env::var_os(REFERENCE_DIR_ENV).unwrap_or_else(|| {
            panic!(
                "{REFERENCE_DIR_ENV} must point at temporary all-case Chromium \
                 references during a CSS-paint audit"
            )
        });
        return (
            PathBuf::from(directory).join(format!("{}.png", case.name)),
            "temporary Chromium audit",
        );
    }

    assert!(
        std::env::var_os(REFERENCE_DIR_ENV).is_none(),
        "{REFERENCE_DIR_ENV} is only valid together with {AUDIT_ENV}"
    );
    match case.expectation {
        Expectation::BrowserMatch => (screenshots.path(&["css-paint", case.name]), "Chromium"),
        Expectation::NativeSnapshot { .. } => (
            screenshots.path(&["css-paint-native", case.name]),
            "native Pulsar/Parley",
        ),
        Expectation::Skip { .. } => {
            panic!("{}: skipped cases need CSS-paint audit mode", case.name)
        }
    }
}

fn update_native_reference(name: &str, actual: &Image) {
    assert!(
        std::env::var_os(AUDIT_ENV).is_none() && std::env::var_os(REFERENCE_DIR_ENV).is_none(),
        "{UPDATE_NATIVE_ENV}=1 cannot update references during an audit"
    );
    let screenshots = screenshots_in(env!("CARGO_MANIFEST_DIR"));
    assert!(
        !screenshots.is_updating(),
        "{UPDATE_NATIVE_ENV}=1 must not be combined with FLASHBULB_UPDATE_SNAPSHOTS"
    );
    let path = screenshots.path(&["css-paint-native", name]);
    let parent = path
        .parent()
        .expect("a native CSS-paint snapshot has a parent directory");
    std::fs::create_dir_all(parent)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
    actual
        .write_png(&path)
        .unwrap_or_else(|error| panic!("cannot update {}: {error}", path.display()));
    eprintln!("updated native CSS-paint snapshot {}", path.display());
}

fn native_update_enabled() -> bool {
    match std::env::var(UPDATE_NATIVE_ENV) {
        Ok(value) => {
            assert_eq!(
                value, "1",
                "{UPDATE_NATIVE_ENV} must be unset or exactly `1`"
            );
            true
        }
        Err(std::env::VarError::NotPresent) => false,
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("{UPDATE_NATIVE_ENV} must contain Unicode `1`")
        }
    }
}

fn assert_flashbulb_update_disabled() {
    assert!(
        !screenshots_in(env!("CARGO_MANIFEST_DIR")).is_updating(),
        "CSS atlas references reject FLASHBULB_UPDATE_SNAPSHOTS; browser \
         references come from Playwright and native references require \
         {UPDATE_NATIVE_ENV}=1"
    );
}

fn init_gpu() -> Mutex<Headless> {
    let gpu = Headless::new()
        .unwrap_or_else(|error| panic!("mandatory GPU initialization failed: {error}"));
    Mutex::new(gpu)
}

fn render_shard(shard: usize) -> ShardOutcome {
    let mut gpu = GPU
        .get_or_init(init_gpu)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let result = catch_unwind(AssertUnwindSafe(|| build_and_render(shard, &mut gpu)));
    match result {
        Ok(Ok(image)) => ShardOutcome::Ready(image),
        Ok(Err(error)) => ShardOutcome::Failed(Arc::from(error)),
        Err(payload) => ShardOutcome::Failed(Arc::from(panic_message(payload.as_ref()))),
    }
}

fn build_and_render(shard: usize, gpu: &mut Headless) -> Result<Image, String> {
    let mut atlas = Scene::new();
    let first = shard * CASES_PER_SHARD;
    let include_skipped = std::env::var_os(AUDIT_ENV).is_some();

    for slot in 0..CASES_PER_SHARD {
        let case = &generated::CASES[first + slot];
        if matches!(case.expectation, Expectation::Skip { .. }) && !include_skipped {
            continue;
        }
        let mut document = html::parse(case.fragment, CELL_SIZE_F32, CELL_SIZE_F32);
        if case.category == "text" {
            let registered = document
                .dom
                .register_fonts(dom::FontBlob::from_static(AHEM));
            if registered != 1 {
                return Err(format!(
                    "{}: expected to register one Ahem face, got {registered}",
                    case.name
                ));
            }
        }
        document.dom.render();
        let child = document.dom.scene();

        let column = u32::try_from(slot % GRID).expect("an atlas column fits u32");
        let row = u32::try_from(slot / GRID).expect("an atlas row fits u32");
        let x = f64::from(column) * CELL_SIZE_F64;
        let y = f64::from(row) * CELL_SIZE_F64;
        let cell = Rect::new(x, y, x + CELL_SIZE_F64, y + CELL_SIZE_F64);
        atlas.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &cell,
        );
        atlas.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &cell);
        atlas.append(&child, Some(Affine::translate((x, y))));
        atlas.pop_layer();
    }

    let pixels = gpu
        .render(&atlas, ATLAS_SIZE, ATLAS_SIZE, Color::WHITE)
        .map_err(|error| error.to_string())?;
    Image::from_rgba8(ATLAS_SIZE, ATLAS_SIZE, pixels).map_err(|error| error.to_string())
}

fn crop_cell(atlas: &Image, slot: usize) -> Image {
    assert_eq!((atlas.width(), atlas.height()), (ATLAS_SIZE, ATLAS_SIZE));
    let x = u32::try_from(slot % GRID).expect("an atlas column fits u32") * CELL_SIZE;
    let y = u32::try_from(slot / GRID).expect("an atlas row fits u32") * CELL_SIZE;
    let mut pixels = Vec::with_capacity((CELL_SIZE * CELL_SIZE * 4) as usize);
    for row in y..y + CELL_SIZE {
        let start = ((row * atlas.width() + x) * 4) as usize;
        let end = start + (CELL_SIZE * 4) as usize;
        pixels.extend_from_slice(&atlas.pixels()[start..end]);
    }
    Image::from_rgba8(CELL_SIZE, CELL_SIZE, pixels)
        .expect("a checked atlas crop has the declared dimensions")
}

fn append_audit(
    path: &Path,
    index: usize,
    case: &CssPaintCase,
    comparison: &flashbulb::Comparison,
) {
    let _guard = AUDIT_WRITE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = AUDIT_TARGET.get_or_init(|| {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("cannot create {}: {error}", parent.display()));
        }
        std::fs::File::create(path)
            .unwrap_or_else(|error| panic!("cannot initialize {}: {error}", path.display()));
        path.to_path_buf()
    });
    assert_eq!(
        target, path,
        "{AUDIT_ENV} changed while the test process was running"
    );
    let mut line = String::new();
    writeln!(
        line,
        "{index:04}\t{}\t{}\t{}\t{}\t{}\t{:.8}\t{}",
        case.name,
        case.category,
        case.source,
        comparison.diff_pixels,
        comparison.anti_aliased_pixels,
        comparison.diff_ratio(),
        if comparison.is_match() {
            "match"
        } else {
            "mismatch"
        }
    )
    .expect("writing to a String cannot fail");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|error| panic!("cannot append {}: {error}", path.display()));
    file.write_all(line.as_bytes())
        .unwrap_or_else(|error| panic!("cannot append {}: {error}", path.display()));
}

fn write_artifacts(name: &str, expected: &Image, actual: &Image, diff: &Image) -> String {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/artifacts/css-paint");
    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", directory.display()));
    let expected_path = directory.join(format!("{name}-expected.png"));
    let actual_path = directory.join(format!("{name}-actual.png"));
    let diff_path = directory.join(format!("{name}-diff.png"));
    expected
        .write_png(&expected_path)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", expected_path.display()));
    actual
        .write_png(&actual_path)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", actual_path.display()));
    diff.write_png(&diff_path)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", diff_path.display()));
    format!(
        "  expected: {}\n  actual:   {}\n  diff:     {}",
        expected_path.display(),
        actual_path.display(),
        diff_path.display()
    )
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else {
        "CSS atlas shard panicked with a non-string payload".to_owned()
    }
}

#[test]
fn css_paint_asset_inventory() {
    assert_inventory_mode();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let browser_golden_directory = manifest.join("tests/screenshots/css-paint");
    let native_golden_directory = manifest.join("tests/screenshots/css-paint-native");
    let fixture_directory = manifest.join("tests/fixtures/css-paint-differences");
    let difference_registry = manifest.join("tests/css-paint-differences.tsv");

    let mut browser_matches = BTreeSet::new();
    let mut native_snapshots = BTreeSet::new();
    let mut skipped = BTreeSet::new();
    let mut expected_difference_registry = BTreeMap::new();
    let mut raster_or_sampling = 0;
    let mut ua_choices = 0;
    let mut gaps = 0;
    let mut non_w3c = 0;
    for case in &generated::CASES {
        match case.expectation {
            Expectation::BrowserMatch => {
                assert!(
                    browser_matches.insert(case.name.to_owned()),
                    "duplicate case {}",
                    case.name
                );
            }
            Expectation::NativeSnapshot { kind, .. } => {
                assert!(
                    kind.uses_native_snapshot(),
                    "{}: only W3C-correct raster/sampling differences or permitted UA choices may \
                     own native snapshots",
                    case.name
                );
                assert!(
                    native_snapshots.insert(case.name.to_owned()),
                    "duplicate case {}",
                    case.name
                );
            }
            Expectation::Skip { kind, .. } => {
                assert!(
                    !kind.uses_native_snapshot(),
                    "{}: W3C-correct raster/sampling differences and permitted UA choices must be \
                     active native snapshots",
                    case.name
                );
                assert!(
                    skipped.insert(case.name.to_owned()),
                    "duplicate case {}",
                    case.name
                );
            }
        }

        if let Some((kind, issue)) = case.expectation.difference() {
            assert!(
                !issue.is_empty(),
                "{}: difference issue is empty",
                case.name
            );
            assert_eq!(
                expected_difference_registry.insert(case.name.to_owned(), issue),
                None,
                "duplicate difference-registry case {}",
                case.name
            );
            match kind {
                DifferenceKind::RasterOrSampling => raster_or_sampling += 1,
                DifferenceKind::UaChoice => ua_choices += 1,
                DifferenceKind::W3cGap => gaps += 1,
                DifferenceKind::NonW3cCompatibility => non_w3c += 1,
            }
        }
    }

    assert_eq!(generated::CASES.len(), CASE_COUNT);
    assert_eq!(browser_matches.len(), 666);
    assert_eq!(native_snapshots.len(), 145);
    assert_eq!(skipped.len(), 189);
    assert!(browser_matches.is_disjoint(&native_snapshots));
    assert!(browser_matches.is_disjoint(&skipped));
    assert!(native_snapshots.is_disjoint(&skipped));
    assert_eq!(
        (raster_or_sampling, ua_choices, gaps, non_w3c),
        (84, 61, 170, 19)
    );
    assert_eq!(
        read_difference_registry(&difference_registry),
        expected_difference_registry,
        "checked difference registry must match generated case metadata"
    );

    validate_asset_basenames(
        &browser_golden_directory,
        &native_golden_directory,
        &fixture_directory,
        &browser_matches,
        &native_snapshots,
        &skipped,
    );
    validate_difference_fixtures(&fixture_directory);
}

fn assert_inventory_mode() {
    assert_flashbulb_update_disabled();
    assert!(
        !native_update_enabled(),
        "{UPDATE_NATIVE_ENV}=1 must use the `css_native_` test filter; run \
         the ordinary suite afterward to validate the completed inventory"
    );
}

fn validate_asset_basenames(
    browser_golden_directory: &Path,
    native_golden_directory: &Path,
    fixture_directory: &Path,
    browser_matches: &BTreeSet<String>,
    native_snapshots: &BTreeSet<String>,
    skipped: &BTreeSet<String>,
) {
    let browser_golden_names = asset_basenames(browser_golden_directory, "png");
    assert_eq!(
        &browser_golden_names, browser_matches,
        "committed browser PNG basenames must equal the BrowserMatch set"
    );
    let native_golden_names = asset_basenames(native_golden_directory, "png");
    assert_eq!(
        &native_golden_names, native_snapshots,
        "committed native PNG basenames must equal the NativeSnapshot set"
    );
    let fixture_names = asset_basenames(fixture_directory, "html");
    let difference_names = native_snapshots
        .union(skipped)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fixture_names, difference_names,
        "committed difference-fixture basenames must equal the difference registry"
    );
}

fn validate_difference_fixtures(fixture_directory: &Path) {
    for case in generated::CASES
        .iter()
        .filter(|case| case.expectation.difference().is_some())
    {
        let fixture = fixture_directory.join(format!("{}.html", case.name));
        let source = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", fixture.display()));
        let marker = format!(r#"<meta name="css-paint-case" content="{}">"#, case.name);
        assert!(
            source.contains(&marker),
            "{}: fixture does not contain its case marker",
            case.name
        );
        let (kind, issue) = case
            .expectation
            .difference()
            .expect("the iterator retains only difference cases");
        let kind_marker = format!(
            r#"<meta name="css-paint-difference-kind" content="{}">"#,
            kind.as_str()
        );
        assert!(
            source.contains(&kind_marker),
            "{}: fixture does not contain its difference-kind marker",
            case.name
        );
        let issue_marker = format!(r#"<meta name="css-paint-issue" content="{issue}">"#);
        assert!(
            source.contains(&issue_marker),
            "{}: fixture does not contain its issue marker",
            case.name
        );
        assert!(
            source.contains(case.fragment),
            "{}: fixture does not retain the generated probe fragment",
            case.name
        );
    }
}

fn read_difference_registry(path: &Path) -> BTreeMap<String, &'static str> {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let mut result = BTreeMap::new();
    for (line_index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, issue) = line.split_once('\t').unwrap_or_else(|| {
            panic!(
                "{}:{}: expected <case> TAB <issue>",
                path.display(),
                line_index + 1
            )
        });
        assert!(
            !issue.contains('\t'),
            "{}:{}: too many TSV columns",
            path.display(),
            line_index + 1
        );
        let generated_issue = generated::CASES
            .iter()
            .find(|case| case.name == name)
            .and_then(|case| case.expectation.difference().map(|(_, issue)| issue))
            .unwrap_or_else(|| {
                panic!(
                    "{}:{}: unknown or browser-match case {name}",
                    path.display(),
                    line_index + 1
                )
            });
        assert_eq!(
            issue,
            generated_issue,
            "{}:{}: issue does not match generated metadata",
            path.display(),
            line_index + 1
        );
        assert!(
            result.insert(name.to_owned(), generated_issue).is_none(),
            "{}:{}: duplicate case {name}",
            path.display(),
            line_index + 1
        );
    }
    result
}

fn asset_basenames(directory: &Path, extension: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for entry in std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
    {
        let path = entry
            .unwrap_or_else(|error| panic!("cannot enumerate {}: {error}", directory.display()))
            .path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some(extension) {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_else(|| panic!("{} has no UTF-8 basename", path.display()));
        assert!(
            result.insert(stem.to_owned()),
            "duplicate asset basename {stem}"
        );
    }
    result
}
