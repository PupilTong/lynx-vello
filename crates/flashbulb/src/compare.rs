//! Perceptual image comparison, ported from the `pixelmatch` algorithm
//! Playwright uses for `expect(page).toHaveScreenshot()`.
//!
//! The port is deliberate rather than incidental: lynx-stack's screenshot
//! suites are the compatibility reference this repo is measured against, so a
//! difference our comparator calls a failure should be one theirs would too.
//! Three properties carry over exactly:
//!
//! 1. **Per-pixel distance is squared YIQ**, compared against `35215 * threshold²` (35215 being the
//!    largest possible YIQ difference). Not sRGB, not CIELAB.
//! 2. **Anti-aliased pixels are detected and excluded.** A pixel that looks like an anti-aliasing
//!    artifact in *either* image is drawn yellow in the diff and not counted, which is what keeps
//!    GPU-rasterizer edge noise from failing an otherwise identical frame.
//! 3. **The budget is the smaller of `max_diff_pixels` and `max_diff_pixel_ratio`**, both optional;
//!    with neither set, the budget is zero differing pixels.

use crate::image::Image;

/// Tolerances for one comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompareOptions {
    /// Per-pixel YIQ sensitivity in `0.0..=1.0`; smaller is stricter.
    /// Playwright's default is `0.2`.
    pub threshold: f64,
    /// Count anti-aliased pixels as differences instead of ignoring them.
    pub include_anti_aliasing: bool,
    /// Absolute budget of differing pixels.
    pub max_diff_pixels: Option<usize>,
    /// Budget as a fraction of the image, in `0.0..=1.0`.
    pub max_diff_pixel_ratio: Option<f64>,
    /// How strongly the unchanged parts of the source show through the diff
    /// image, in `0.0..=1.0`.
    pub diff_background_alpha: f64,
}

impl Default for CompareOptions {
    /// Playwright's per-pixel default (`threshold: 0.2`) with a zero-pixel
    /// budget — the same effective setting lynx-stack's own helpers use when
    /// they pass `maxDiffPixelRatio: 0`.
    fn default() -> Self {
        Self {
            threshold: 0.2,
            include_anti_aliasing: false,
            max_diff_pixels: None,
            max_diff_pixel_ratio: None,
            diff_background_alpha: 0.1,
        }
    }
}

impl CompareOptions {
    #[must_use]
    pub const fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    #[must_use]
    pub const fn with_max_diff_pixels(mut self, max_diff_pixels: usize) -> Self {
        self.max_diff_pixels = Some(max_diff_pixels);
        self
    }

    #[must_use]
    pub const fn with_max_diff_pixel_ratio(mut self, max_diff_pixel_ratio: f64) -> Self {
        self.max_diff_pixel_ratio = Some(max_diff_pixel_ratio);
        self
    }

    /// The number of differing pixels this comparison tolerates over an image
    /// of `pixel_count` pixels.
    #[must_use]
    pub fn budget(self, pixel_count: usize) -> usize {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a pixel budget is inherently approximate and non-negative"
        )]
        // Playwright multiplies the ratio unclamped, so a negative one makes
        // even an identical pair "fail". Clamping is a deliberate divergence:
        // a typo'd ratio should mean "no tolerance", not "always fail".
        let from_ratio = self
            .max_diff_pixel_ratio
            .map(|ratio| (pixel_count as f64 * ratio.clamp(0.0, 1.0)) as usize);
        match (self.max_diff_pixels, from_ratio) {
            (Some(absolute), Some(ratio)) => absolute.min(ratio),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => 0,
        }
    }
}

/// What a comparison found.
#[derive(Debug)]
pub struct Comparison {
    /// Pixels that differ beyond `threshold` and are not anti-aliasing.
    pub diff_pixels: usize,
    /// Pixels excluded as anti-aliasing artifacts.
    pub anti_aliased_pixels: usize,
    /// The tolerated budget this comparison was measured against.
    pub budget: usize,
    /// A visualization: unchanged content dimmed to gray, real differences in
    /// red, anti-aliasing in yellow.
    pub diff: Image,
}

impl Comparison {
    /// Whether the two images matched within the budget.
    #[must_use]
    pub const fn is_match(&self) -> bool {
        self.diff_pixels <= self.budget
    }

    /// `diff_pixels` as a fraction of the compared area.
    #[must_use]
    pub fn diff_ratio(&self) -> f64 {
        let total = self.diff.pixel_count();
        if total == 0 {
            0.0
        } else {
            #[allow(
                clippy::cast_precision_loss,
                reason = "pixel counts stay far below f64's exact-integer range"
            )]
            {
                self.diff_pixels as f64 / total as f64
            }
        }
    }
}

/// The largest possible squared YIQ difference between two colors — the
/// normalization constant `threshold` is expressed against.
const MAX_YIQ_DELTA: f64 = 35215.0;

const DIFF_COLOR: [u8; 3] = [255, 0, 0];
const ANTI_ALIAS_COLOR: [u8; 3] = [255, 255, 0];

/// Compares two same-sized images.
///
/// # Panics
///
/// Panics if the images differ in size; call [`Image::has_same_size`] first —
/// a size mismatch is a different kind of failure than a pixel mismatch and
/// callers report it differently.
#[must_use]
pub fn compare(expected: &Image, actual: &Image, options: CompareOptions) -> Comparison {
    assert!(
        expected.has_same_size(actual),
        "compare requires equally sized images"
    );

    let width = expected.width();
    let height = expected.height();
    let mut diff = Image::transparent(width, height);
    let mut diff_pixels = 0;
    let mut anti_aliased_pixels = 0;
    let max_delta = MAX_YIQ_DELTA * options.threshold * options.threshold;

    for y in 0..height {
        for x in 0..width {
            let position = offset(width, x, y);
            let delta = color_delta(
                expected.pixels(),
                actual.pixels(),
                position,
                position,
                false,
            );
            // Written as pixelmatch writes it (`> maxDelta` selects the diff
            // branch) rather than as the negation, so a non-ordered threshold
            // such as NaN takes the same branch there as here.
            if delta.abs() > max_delta {
                let is_anti_aliasing = !options.include_anti_aliasing
                    && (is_anti_aliased(expected, actual, x, y)
                        || is_anti_aliased(actual, expected, x, y));
                if is_anti_aliasing {
                    anti_aliased_pixels += 1;
                    draw(diff.pixels_mut(), position, ANTI_ALIAS_COLOR);
                } else {
                    diff_pixels += 1;
                    draw(diff.pixels_mut(), position, DIFF_COLOR);
                }
            } else {
                draw_gray(
                    expected.pixels(),
                    position,
                    options.diff_background_alpha,
                    diff.pixels_mut(),
                );
            }
        }
    }

    Comparison {
        diff_pixels,
        anti_aliased_pixels,
        budget: options.budget(expected.pixel_count()),
        diff,
    }
}

const fn offset(width: u32, x: u32, y: u32) -> usize {
    ((y as usize) * (width as usize) + (x as usize)) * 4
}

fn draw(target: &mut [u8], position: usize, color: [u8; 3]) {
    target[position] = color[0];
    target[position + 1] = color[1];
    target[position + 2] = color[2];
    target[position + 3] = 255;
}

/// Writes the source pixel as dimmed gray, so real differences stand out
/// against a ghost of the original.
///
/// Unlike the comparison path, the source channels are *not* composited over
/// white first — pixelmatch's `drawGrayPixel` takes their luminance raw and
/// folds alpha into the dimming factor instead.
fn draw_gray(source: &[u8], position: usize, alpha: f64, target: &mut [u8]) {
    let luminance = rgb_to_y_components(
        f64::from(source[position]),
        f64::from(source[position + 1]),
        f64::from(source[position + 2]),
    );
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the result is a rounded 8-bit channel value"
    )]
    let value = blend(luminance, alpha * f64::from(source[position + 3]) / 255.0) as u8;
    draw(target, position, [value, value, value]);
}

/// The signed squared YIQ distance between two pixels.
///
/// The sign records which pixel is brighter — the anti-aliasing detector below
/// needs the direction, the diff count needs only the magnitude.
fn color_delta(first: &[u8], second: &[u8], at: usize, other: usize, luminance_only: bool) -> f64 {
    if first[at..at + 4] == second[other..other + 4] {
        return 0.0;
    }

    let (r1, g1, b1) = unpremultiply(first, at);
    let (r2, g2, b2) = unpremultiply(second, other);

    let y = rgb_to_y_components(r1, g1, b1) - rgb_to_y_components(r2, g2, b2);
    if luminance_only {
        return y;
    }
    let i = rgb_to_i(r1, g1, b1) - rgb_to_i(r2, g2, b2);
    let q = rgb_to_q(r1, g1, b1) - rgb_to_q(r2, g2, b2);
    let delta = 0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q;
    if y > 0.0 { -delta } else { delta }
}

/// Composites a pixel over white, the background pixelmatch assumes, so that
/// two differently transparent pixels are compared as they would look.
fn unpremultiply(pixels: &[u8], at: usize) -> (f64, f64, f64) {
    let alpha = f64::from(pixels[at + 3]);
    let (r, g, b) = (
        f64::from(pixels[at]),
        f64::from(pixels[at + 1]),
        f64::from(pixels[at + 2]),
    );
    if alpha >= 255.0 {
        return (r, g, b);
    }
    let alpha = alpha / 255.0;
    (blend(r, alpha), blend(g, alpha), blend(b, alpha))
}

fn blend(channel: f64, alpha: f64) -> f64 {
    255.0 + (channel - 255.0) * alpha
}

fn rgb_to_y_components(r: f64, g: f64, b: f64) -> f64 {
    r * 0.298_895_31 + g * 0.586_622_47 + b * 0.114_482_23
}

fn rgb_to_i(r: f64, g: f64, b: f64) -> f64 {
    r * 0.595_977_99 - g * 0.274_176_10 - b * 0.321_801_89
}

fn rgb_to_q(r: f64, g: f64, b: f64) -> f64 {
    r * 0.211_470_17 - g * 0.522_617_11 + b * 0.311_146_94
}

/// Whether the pixel at (`x`, `y`) in `image` looks like an anti-aliasing
/// artifact, using Vysniauskas's intensity-slope test: an anti-aliased pixel
/// is a local brightness extreme with few equal neighbours, and its extreme
/// neighbour is a solid-colored pixel in *both* images.
fn is_anti_aliased(image: &Image, other: &Image, x: u32, y: u32) -> bool {
    let width = image.width();
    let height = image.height();
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x2 = (x + 1).min(width - 1);
    let y2 = (y + 1).min(height - 1);
    let position = offset(width, x, y);
    // Pixels on the image border have fewer neighbours; count the missing ones
    // as equal so an edge pixel is not mistaken for an extreme.
    let mut equal_neighbours = u32::from(x == x0 || x == x2 || y == y0 || y == y2);

    let mut min = 0.0;
    let mut max = 0.0;
    let mut min_at = (0, 0);
    let mut max_at = (0, 0);

    // x-outer / y-inner, matching pixelmatch. The order is load-bearing: `min`
    // and `max` are tracked with strict comparisons, so the first neighbour
    // visited wins a tie, and a transposed walk can pick a different extreme
    // pixel and reach the opposite anti-aliasing verdict.
    for neighbour_x in x0..=x2 {
        for neighbour_y in y0..=y2 {
            if neighbour_x == x && neighbour_y == y {
                continue;
            }
            let delta = color_delta(
                image.pixels(),
                image.pixels(),
                position,
                offset(width, neighbour_x, neighbour_y),
                true,
            );
            if delta == 0.0 {
                equal_neighbours += 1;
                if equal_neighbours > 2 {
                    return false;
                }
            } else if delta < min {
                min = delta;
                min_at = (neighbour_x, neighbour_y);
            } else if delta > max {
                max = delta;
                max_at = (neighbour_x, neighbour_y);
            }
        }
    }

    if min == 0.0 || max == 0.0 {
        return false;
    }

    (has_many_siblings(image, min_at.0, min_at.1) && has_many_siblings(other, min_at.0, min_at.1))
        || (has_many_siblings(image, max_at.0, max_at.1)
            && has_many_siblings(other, max_at.0, max_at.1))
}

/// Whether a pixel has three or more identically colored neighbours, i.e. sits
/// in a solid region rather than on an edge.
fn has_many_siblings(image: &Image, x: u32, y: u32) -> bool {
    let width = image.width();
    let height = image.height();
    let x0 = x.saturating_sub(1);
    let y0 = y.saturating_sub(1);
    let x2 = (x + 1).min(width - 1);
    let y2 = (y + 1).min(height - 1);
    let position = offset(width, x, y);
    let mut equal_neighbours = u32::from(x == x0 || x == x2 || y == y0 || y == y2);

    for neighbour_x in x0..=x2 {
        for neighbour_y in y0..=y2 {
            if neighbour_x == x && neighbour_y == y {
                continue;
            }
            let other = offset(width, neighbour_x, neighbour_y);
            if image.pixels()[position..position + 4] == image.pixels()[other..other + 4] {
                equal_neighbours += 1;
                if equal_neighbours > 2 {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{CompareOptions, compare};
    use crate::image::Image;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Image {
        let pixels = color
            .iter()
            .copied()
            .cycle()
            .take((width as usize) * (height as usize) * 4)
            .collect();
        Image::from_rgba8(width, height, pixels).unwrap()
    }

    #[test]
    fn identical_images_have_no_differences() {
        let image = solid(8, 8, [12, 34, 56, 255]);
        let result = compare(&image, &image, CompareOptions::default());
        assert_eq!(result.diff_pixels, 0);
        assert!(result.is_match());
        assert!(result.diff_ratio().abs() < f64::EPSILON);
    }

    #[test]
    fn a_flat_color_change_is_counted_everywhere() {
        let expected = solid(4, 4, [0, 0, 0, 255]);
        let actual = solid(4, 4, [255, 255, 255, 255]);
        let result = compare(&expected, &actual, CompareOptions::default());
        assert_eq!(result.diff_pixels, 16);
        assert!(!result.is_match());
        assert!((result.diff_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_change_under_the_threshold_is_ignored() {
        let expected = solid(4, 4, [100, 100, 100, 255]);
        let actual = solid(4, 4, [101, 100, 100, 255]);
        let result = compare(&expected, &actual, CompareOptions::default());
        assert_eq!(result.diff_pixels, 0);
    }

    #[test]
    fn a_tighter_threshold_catches_what_the_default_ignores() {
        let expected = solid(4, 4, [100, 100, 100, 255]);
        let actual = solid(4, 4, [104, 100, 100, 255]);
        assert_eq!(
            compare(&expected, &actual, CompareOptions::default()).diff_pixels,
            0
        );
        assert_eq!(
            compare(
                &expected,
                &actual,
                CompareOptions::default().with_threshold(0.0)
            )
            .diff_pixels,
            16
        );
    }

    #[test]
    fn the_budget_is_the_smaller_of_the_two_limits() {
        let options = CompareOptions::default()
            .with_max_diff_pixels(10)
            .with_max_diff_pixel_ratio(0.5);
        assert_eq!(options.budget(100), 10);

        let options = CompareOptions::default()
            .with_max_diff_pixels(90)
            .with_max_diff_pixel_ratio(0.5);
        assert_eq!(options.budget(100), 50);

        assert_eq!(CompareOptions::default().budget(100), 0);
    }

    #[test]
    fn a_difference_within_budget_still_matches() {
        let expected = solid(4, 4, [0, 0, 0, 255]);
        let actual = solid(4, 4, [255, 255, 255, 255]);
        let result = compare(
            &expected,
            &actual,
            CompareOptions::default().with_max_diff_pixels(16),
        );
        assert_eq!(result.diff_pixels, 16);
        assert!(result.is_match());
    }

    #[test]
    fn the_diff_image_paints_differences_red_and_the_rest_gray() {
        let expected = solid(2, 1, [0, 0, 0, 255]);
        let mut actual_pixels = expected.pixels().to_vec();
        actual_pixels[4..8].copy_from_slice(&[255, 255, 255, 255]);
        let actual = Image::from_rgba8(2, 1, actual_pixels).unwrap();

        let result = compare(&expected, &actual, CompareOptions::default());
        assert_eq!(result.diff_pixels, 1);
        // Pixel 0 matched: dimmed gray with equal channels.
        let diff = result.diff.pixels();
        assert_eq!(diff[0], diff[1]);
        assert_eq!(diff[1], diff[2]);
        // Pixel 1 differed: solid red.
        assert_eq!(&diff[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn anti_aliasing_on_an_edge_is_excluded_from_the_count() {
        // A hard black/white vertical edge, with one column of the boundary
        // shaded differently in each image — the shape an anti-aliased edge
        // takes when two rasterizers disagree about coverage.
        const WIDTH: u32 = 5;
        const HEIGHT: u32 = 5;
        let build = |edge: u8| {
            let mut pixels = Vec::with_capacity((WIDTH as usize) * (HEIGHT as usize) * 4);
            for _ in 0..HEIGHT {
                for x in 0..WIDTH {
                    let value = match x {
                        0 | 1 => 0,
                        2 => edge,
                        _ => 255,
                    };
                    pixels.extend_from_slice(&[value, value, value, 255]);
                }
            }
            Image::from_rgba8(WIDTH, HEIGHT, pixels).unwrap()
        };

        let result = compare(&build(100), &build(180), CompareOptions::default());
        assert_eq!(result.diff_pixels, 0);
        assert!(result.anti_aliased_pixels > 0);
        assert!(result.is_match());

        // With detection off, the same change is a plain difference.
        let strict = CompareOptions {
            include_anti_aliasing: true,
            ..CompareOptions::default()
        };
        let result = compare(&build(100), &build(180), strict);
        assert!(result.diff_pixels > 0);
        assert_eq!(result.anti_aliased_pixels, 0);
    }

    /// The 3x3 case where the neighbour-iteration order decides the verdict.
    ///
    /// Two neighbours of the changed pixel tie on luminance delta, and `min`
    /// and `max` update on strict comparisons, so whichever is visited first
    /// wins — and only one of them has enough equal siblings to make the pixel
    /// read as anti-aliasing. Walking y-outer instead of x-outer flips this
    /// from "0 differences" (what Playwright reports) to "1 difference", which
    /// with the default zero-pixel budget is a spurious failure.
    #[test]
    fn tie_breaking_matches_playwright_neighbour_order() {
        let build = |top_left: u8| {
            let rows: [[u8; 3]; 3] = [[top_left, 0, 255], [0, 255, 128], [255, 0, 128]];
            let mut pixels = Vec::with_capacity(3 * 3 * 4);
            for row in rows {
                for value in row {
                    pixels.extend_from_slice(&[value, value, value, 255]);
                }
            }
            Image::from_rgba8(3, 3, pixels).unwrap()
        };

        for threshold in [0.05, 0.1, 0.2] {
            let result = compare(
                &build(255),
                &build(128),
                CompareOptions::default().with_threshold(threshold),
            );
            assert_eq!(
                result.diff_pixels, 0,
                "threshold {threshold}: pixelmatch reports no differences here"
            );
            assert_eq!(result.anti_aliased_pixels, 1, "threshold {threshold}");
            // Playwright paints that pixel yellow, not red.
            assert_eq!(&result.diff.pixels()[0..4], &[255, 255, 0, 255]);
        }
    }

    #[test]
    #[should_panic(expected = "equally sized images")]
    fn comparing_different_sizes_panics() {
        let _ = compare(
            &solid(2, 2, [0, 0, 0, 255]),
            &solid(3, 3, [0, 0, 0, 255]),
            CompareOptions::default(),
        );
    }
}
