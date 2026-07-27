//! Golden-file management: resolve, compare, accept, and report.
//!
//! Modelled on Playwright's `toHaveScreenshot`, which lynx-stack's suites use:
//! goldens live in a directory tree beside the test, an environment switch
//! accepts the current rendering, and a failure leaves `-expected`, `-actual`,
//! and `-diff` PNGs behind to look at.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::compare::{CompareOptions, Comparison, compare};
use crate::image::{Image, ImageError};

/// Set this to `1` to overwrite goldens with the current rendering.
pub const UPDATE_ENV: &str = "FLASHBULB_UPDATE_SNAPSHOTS";

/// A directory of golden PNGs plus the tolerances they are compared with.
#[derive(Clone, Debug)]
pub struct Screenshots {
    goldens: PathBuf,
    artifacts: PathBuf,
    options: CompareOptions,
    update: bool,
}

impl Screenshots {
    /// Opens the golden directory at `goldens`, writing failure artifacts into
    /// a sibling `artifacts` directory (git-ignored, and easy to upload from
    /// CI — the equivalent of Playwright's `test-results/`).
    ///
    /// Tests normally build the path from their own crate:
    /// `Screenshots::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/screenshots"))`.
    #[must_use]
    pub fn new(goldens: impl Into<PathBuf>) -> Self {
        let goldens = goldens.into();
        let artifacts = goldens.parent().map_or_else(
            || goldens.join("artifacts"),
            |parent| parent.join("artifacts"),
        );
        Self {
            goldens,
            artifacts,
            options: CompareOptions::default(),
            update: std::env::var(UPDATE_ENV).as_deref() == Ok("1"),
        }
    }

    /// Overrides where failure artifacts are written.
    #[must_use]
    pub fn with_artifacts_dir(mut self, artifacts: impl Into<PathBuf>) -> Self {
        self.artifacts = artifacts.into();
        self
    }

    /// Overrides the comparison tolerances for every golden in this store.
    #[must_use]
    pub const fn with_options(mut self, options: CompareOptions) -> Self {
        self.options = options;
        self
    }

    #[must_use]
    pub const fn options(&self) -> CompareOptions {
        self.options
    }

    /// Whether this run overwrites goldens instead of checking them.
    #[must_use]
    pub const fn is_updating(&self) -> bool {
        self.update
    }

    /// The path a name resolves to.
    ///
    /// `name` is a path-segment list, like Playwright's snapshot-name array:
    /// `["basic-flex", "index"]` becomes `<goldens>/basic-flex/index.png`.
    ///
    /// The extension is *appended*, never substituted, so a segment containing
    /// a dot keeps it — `["1.5x-scale"]` is `1.5x-scale.png`, not `1.png`.
    /// `set_extension` would silently collapse those onto one file.
    ///
    /// # Panics
    ///
    /// Panics on an empty name, or on a segment that is empty, absolute, or
    /// contains a path separator or `..` — a golden must name one file inside
    /// the store, and a caller that computed a segment from data should not be
    /// able to escape it.
    #[must_use]
    pub fn path(&self, name: &[&str]) -> PathBuf {
        assert!(
            !name.is_empty(),
            "a screenshot name needs at least one segment"
        );
        let mut path = self.goldens.clone();
        for (index, segment) in name.iter().enumerate() {
            assert!(
                !segment.is_empty()
                    && *segment != ".."
                    && !segment.contains('/')
                    && !segment.contains('\\'),
                "screenshot name segment {index} ({segment:?}) must be a plain file name"
            );
            if index + 1 == name.len() {
                path.push(format!("{segment}.png"));
            } else {
                path.push(segment);
            }
        }
        path
    }

    /// Compares `actual` against the golden `name`, or accepts it when the
    /// update switch is set.
    ///
    /// A missing golden is written and reported as [`GoldenOutcome::Written`]
    /// rather than failing, matching Playwright's first-run behavior.
    pub fn check(&self, name: &[&str], actual: &Image) -> Result<GoldenOutcome, GoldenError> {
        let path = self.path(name);
        if self.update {
            actual.write_png(&path).map_err(GoldenError::Io)?;
            return Ok(GoldenOutcome::Updated { path });
        }
        if !path.exists() {
            actual.write_png(&path).map_err(GoldenError::Io)?;
            return Ok(GoldenOutcome::Written { path });
        }

        let expected = Image::read_png(&path).map_err(GoldenError::Io)?;
        if !expected.has_same_size(actual) {
            let artifacts = self.write_artifacts(name, &expected, actual, None)?;
            return Ok(GoldenOutcome::SizeMismatch {
                path,
                expected: (expected.width(), expected.height()),
                actual: (actual.width(), actual.height()),
                artifacts,
            });
        }

        let comparison = compare(&expected, actual, self.options);
        if comparison.is_match() {
            return Ok(GoldenOutcome::Matched {
                anti_aliased_pixels: comparison.anti_aliased_pixels,
            });
        }
        let artifacts = self.write_artifacts(name, &expected, actual, Some(&comparison))?;
        Ok(GoldenOutcome::Differed {
            path,
            comparison,
            artifacts,
        })
    }

    /// [`Self::check`], but panics with a report on anything but a match.
    ///
    /// This is the entry point a `#[test]` normally calls.
    ///
    /// # Panics
    ///
    /// Panics when the golden differs, is a different size, was created by
    /// this run, or could not be read. A golden that had to be *created* fails
    /// deliberately, so an unreviewed baseline cannot pass CI silently; a
    /// golden deliberately *accepted* through [`UPDATE_ENV`] passes, since
    /// setting that variable is the review.
    pub fn assert_matches(&self, name: &[&str], actual: &Image) {
        match self.check(name, actual) {
            Ok(GoldenOutcome::Matched { .. }) => {}
            Ok(outcome @ GoldenOutcome::Updated { .. }) => eprintln!("{outcome}"),
            Ok(outcome) => panic!("{outcome}"),
            Err(error) => panic!("{error}"),
        }
    }

    fn write_artifacts(
        &self,
        name: &[&str],
        expected: &Image,
        actual: &Image,
        comparison: Option<&Comparison>,
    ) -> Result<Artifacts, GoldenError> {
        let stem = name.join("-");
        let write = |suffix: &str, image: &Image| -> Result<PathBuf, GoldenError> {
            let path = self.artifacts.join(format!("{stem}-{suffix}.png"));
            image.write_png(&path).map_err(GoldenError::Io)?;
            Ok(path)
        };
        Ok(Artifacts {
            expected: write("expected", expected)?,
            actual: write("actual", actual)?,
            diff: match comparison {
                Some(comparison) => Some(write("diff", &comparison.diff)?),
                None => None,
            },
        })
    }
}

/// Where the three failure PNGs were written.
#[derive(Clone, Debug)]
pub struct Artifacts {
    pub expected: PathBuf,
    pub actual: PathBuf,
    /// Absent when the images were different sizes and could not be diffed.
    pub diff: Option<PathBuf>,
}

impl fmt::Display for Artifacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "  expected: {}\n  actual:   {}",
            self.expected.display(),
            self.actual.display()
        )?;
        if let Some(diff) = &self.diff {
            write!(formatter, "\n  diff:     {}", diff.display())?;
        }
        Ok(())
    }
}

/// What checking a golden produced.
#[derive(Debug)]
#[non_exhaustive]
pub enum GoldenOutcome {
    /// The rendering matched within tolerance.
    Matched { anti_aliased_pixels: usize },
    /// No golden existed, so one was written from this run.
    Written { path: PathBuf },
    /// The golden was deliberately replaced because [`UPDATE_ENV`] was set.
    Updated { path: PathBuf },
    /// The rendering differed beyond the budget.
    Differed {
        path: PathBuf,
        comparison: Comparison,
        artifacts: Artifacts,
    },
    /// The rendering is a different size than the golden.
    SizeMismatch {
        path: PathBuf,
        expected: (u32, u32),
        actual: (u32, u32),
        artifacts: Artifacts,
    },
}

impl fmt::Display for GoldenOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matched {
                anti_aliased_pixels,
            } => write!(
                formatter,
                "screenshot matched ({anti_aliased_pixels} anti-aliased pixels ignored)"
            ),
            Self::Written { path } => write!(
                formatter,
                "wrote a new golden at {}\n\
                 Review it and re-run; this run fails so an unreviewed baseline cannot pass.",
                path.display()
            ),
            Self::Updated { path } => {
                write!(
                    formatter,
                    "accepted a new rendering into {}",
                    path.display()
                )
            }
            Self::Differed {
                path,
                comparison,
                artifacts,
            } => write!(
                formatter,
                "screenshot differs from {}\n\
                 {} of {} pixels differ ({:.4}%), budget {}; {} anti-aliased pixels ignored\n\
                 {artifacts}\n\
                 Set {UPDATE_ENV}=1 to accept the current rendering.",
                path.display(),
                comparison.diff_pixels,
                comparison.diff.pixel_count(),
                comparison.diff_ratio() * 100.0,
                comparison.budget,
                comparison.anti_aliased_pixels,
            ),
            Self::SizeMismatch {
                path,
                expected,
                actual,
                artifacts,
            } => write!(
                formatter,
                "screenshot is {}\u{d7}{} but {} is {}\u{d7}{}\n\
                 {artifacts}\n\
                 Set {UPDATE_ENV}=1 to accept the current rendering.",
                actual.0,
                actual.1,
                path.display(),
                expected.0,
                expected.1,
            ),
        }
    }
}

/// A golden could not be read or written.
#[derive(Debug)]
#[non_exhaustive]
pub enum GoldenError {
    Io(ImageError),
}

impl fmt::Display for GoldenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "golden file error: {error}"),
        }
    }
}

impl std::error::Error for GoldenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
        }
    }
}

/// A convenience for tests that keep their goldens under
/// `<crate>/tests/screenshots`.
#[must_use]
pub fn screenshots_in(manifest_dir: &str) -> Screenshots {
    Screenshots::new(Path::new(manifest_dir).join("tests").join("screenshots"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{GoldenOutcome, Screenshots};
    use crate::compare::CompareOptions;
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

    /// A scratch golden directory unique to this call, so tests in one binary
    /// — and two concurrent `cargo test` processes sharing a `TMPDIR` — cannot
    /// delete each other's goldens mid-comparison.
    fn store(label: &str) -> Screenshots {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("flashbulb-{label}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // The default artifacts dir is a sibling of the goldens dir; keeping it
        // inside this unique root isolates it too.
        Screenshots::new(root.join("screenshots"))
    }

    #[test]
    fn name_segments_become_directories() {
        let store = store("path");
        let path = store.path(&["group", "case"]);
        assert!(path.ends_with("group/case.png"), "{}", path.display());
    }

    #[test]
    fn a_dot_in_a_name_is_kept_rather_than_treated_as_an_extension() {
        let store = store("dots");
        assert!(
            store.path(&["1.5x-scale"]).ends_with("1.5x-scale.png"),
            "{}",
            store.path(&["1.5x-scale"]).display()
        );
        // Two names that `set_extension` would collapse onto one file stay
        // distinct.
        assert_ne!(store.path(&["a.b"]), store.path(&["a.c"]));
    }

    #[test]
    #[should_panic(expected = "at least one segment")]
    fn an_empty_name_is_rejected() {
        let _ = store("empty").path(&[]);
    }

    #[test]
    #[should_panic(expected = "plain file name")]
    fn a_name_segment_cannot_escape_the_store() {
        let _ = store("escape").path(&["..", "outside"]);
    }

    #[test]
    fn a_missing_golden_is_written_and_reported() {
        let store = store("missing");
        let image = solid(2, 2, [1, 2, 3, 255]);
        let outcome = store.check(&["new"], &image).unwrap();
        let GoldenOutcome::Written { path } = outcome else {
            panic!("expected a written golden, got {outcome:?}");
        };
        assert!(path.exists());
        assert_eq!(Image::read_png(&path).unwrap(), image);
    }

    #[test]
    fn an_identical_rendering_matches() {
        let store = store("match");
        let image = solid(2, 2, [1, 2, 3, 255]);
        store.check(&["case"], &image).unwrap();
        assert!(matches!(
            store.check(&["case"], &image).unwrap(),
            GoldenOutcome::Matched { .. }
        ));
        store.assert_matches(&["case"], &image);
    }

    #[test]
    fn a_changed_rendering_differs_and_leaves_artifacts() {
        let store = store("differ");
        store
            .check(&["case"], &solid(2, 2, [0, 0, 0, 255]))
            .unwrap();

        let outcome = store
            .check(&["case"], &solid(2, 2, [255, 255, 255, 255]))
            .unwrap();
        let GoldenOutcome::Differed {
            comparison,
            artifacts,
            ..
        } = outcome
        else {
            panic!("expected a difference");
        };
        assert_eq!(comparison.diff_pixels, 4);
        assert!(artifacts.expected.exists());
        assert!(artifacts.actual.exists());
        assert!(artifacts.diff.as_ref().is_some_and(|path| path.exists()));
    }

    #[test]
    fn a_resized_rendering_reports_the_size_rather_than_a_diff() {
        let store = store("resize");
        store
            .check(&["case"], &solid(2, 2, [0, 0, 0, 255]))
            .unwrap();

        let outcome = store
            .check(&["case"], &solid(3, 3, [0, 0, 0, 255]))
            .unwrap();
        let GoldenOutcome::SizeMismatch {
            expected,
            actual,
            artifacts,
            ..
        } = outcome
        else {
            panic!("expected a size mismatch");
        };
        assert_eq!(expected, (2, 2));
        assert_eq!(actual, (3, 3));
        assert!(artifacts.diff.is_none());
    }

    #[test]
    fn a_difference_inside_the_configured_budget_matches() {
        let store =
            store("budget").with_options(CompareOptions::default().with_max_diff_pixel_ratio(1.0));
        store
            .check(&["case"], &solid(2, 2, [0, 0, 0, 255]))
            .unwrap();
        assert!(matches!(
            store
                .check(&["case"], &solid(2, 2, [255, 255, 255, 255]))
                .unwrap(),
            GoldenOutcome::Matched { .. }
        ));
    }

    #[test]
    fn a_fresh_golden_fails_the_run_that_created_it() {
        let store = store("fresh");
        let result = std::panic::catch_unwind(|| {
            store.assert_matches(&["case"], &solid(2, 2, [0, 0, 0, 255]));
        });
        assert!(result.is_err(), "a newly written golden must not pass");
    }
}
