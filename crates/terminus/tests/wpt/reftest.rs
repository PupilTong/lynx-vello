//! Reftests: the test and each of its references rendered by the engine at
//! 800×600 on white and compared pixel by pixel.
//!
//! A `match` reference must render identically, within the page's
//! `<meta name="fuzzy">` allowance (the largest per-channel difference and
//! the number of differing pixels it tolerates); a `mismatch` reference
//! must not. Both sides go through the same harness sheet and renderer, so
//! a pass says the engine draws the test the way it draws the reference —
//! which is what the reference was written to show.

use std::path::Path;

use prime_radiant::peniko::Color;
use prime_radiant::{Image, capture_document, headless};
use terminus::link::NoImages;
use terminus::link::render::gpu::Headless;

use super::loader::{Page, Paths, load};

/// One `<meta name="fuzzy">` allowance: inclusive upper bounds.
#[derive(Clone, Copy, Debug, Default)]
struct Fuzzy {
    max_difference: u8,
    total_pixels: usize,
}

fn upper(range: &str) -> Option<usize> {
    let range = range.trim();
    let upper = range.rsplit('-').next()?.trim();
    upper.parse().ok()
}

/// Parses `maxDifference=0-2;totalPixels=0-100`, `0-2;0-100`, optionally
/// prefixed by a reference URL and a colon.
fn fuzzy(page: &Page, reference: &str) -> Fuzzy {
    let mut allowance = Fuzzy::default();
    for (name, content) in &page.metas {
        if name != "fuzzy" {
            continue;
        }
        let mut content = content.as_str();
        if let Some((url, rest)) = content.split_once(':')
            && !url.contains('=')
            && !url.contains(';')
        {
            if !reference.ends_with(url.trim()) {
                continue;
            }
            content = rest;
        }
        let parts: Vec<&str> = content.split(';').collect();
        for (index, part) in parts.iter().enumerate() {
            let (key, value) = match part.split_once('=') {
                Some((key, value)) => (key.trim(), value),
                None => (
                    if index == 0 {
                        "maxDifference"
                    } else {
                        "totalPixels"
                    },
                    *part,
                ),
            };
            match key {
                "maxDifference" => {
                    allowance.max_difference =
                        upper(value).and_then(|v| u8::try_from(v).ok()).unwrap_or(0);
                }
                "totalPixels" => allowance.total_pixels = upper(value).unwrap_or(0),
                _ => {}
            }
        }
    }
    allowance
}

fn render(gpu: &mut Headless, page: &mut Page) -> Result<Image, String> {
    capture_document(gpu, &mut page.dom, Color::WHITE, &NoImages).map_err(|e| e.to_string())
}

/// A per-channel difference the comparison ignores everywhere: the
/// rasterizer's coverage rounding moves an anti-aliased edge pixel by one
/// or two levels between two scenes that draw the same geometry through
/// different paint orders. WPT's own `fuzzy` allowances sit on top of it.
const RASTER_NOISE: u8 = 2;

/// `(differing pixels, largest channel difference)`, ignoring
/// [`RASTER_NOISE`].
fn difference(a: &Image, b: &Image) -> (usize, u8) {
    let mut pixels = 0;
    let mut largest = 0;
    for (x, y) in a.pixels().chunks_exact(4).zip(b.pixels().chunks_exact(4)) {
        let channel = x
            .iter()
            .zip(y)
            .map(|(p, q)| p.abs_diff(*q))
            .max()
            .unwrap_or(0);
        if channel > RASTER_NOISE {
            pixels += 1;
            largest = largest.max(channel);
        }
    }
    (pixels, largest)
}

/// Runs the reftest at `file`, whose references are `(rel, href)`.
pub(crate) fn run(
    gpu: &mut Option<Headless>,
    paths: &Paths,
    file: &Path,
    page: &mut Page,
    references: &[(String, String)],
) -> Result<(), String> {
    let gpu = gpu.get_or_insert_with(|| headless("wpt"));
    let actual = render(gpu, page)?;
    for (rel, href) in references {
        let path = paths
            .resolve(file, href)
            .ok_or_else(|| format!("unresolvable reference {href}"))?;
        let mut reference = load(paths, &path)?;
        let expected = render(gpu, &mut reference)?;
        let (pixels, largest) = difference(&actual, &expected);
        if pixels > 0
            && let Some(dir) = std::env::var_os("WPT_DUMP")
        {
            let dir = std::path::PathBuf::from(dir);
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let _ = std::fs::create_dir_all(&dir);
            let _ = actual.write_png(dir.join(format!("{stem}.test.png")));
            let _ = expected.write_png(dir.join(format!("{stem}.ref.png")));
        }
        let allowance = fuzzy(page, href);
        let matches = pixels == 0
            || (largest <= allowance.max_difference && pixels <= allowance.total_pixels);
        match (rel.as_str(), matches) {
            ("match", false) => {
                return Err(format!(
                    "differs from {href}: {pixels} pixels, max channel difference {largest}"
                ));
            }
            ("mismatch", true) => return Err(format!("renders the same as mismatch {href}")),
            _ => {}
        }
    }
    Ok(())
}
