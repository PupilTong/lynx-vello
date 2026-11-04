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

pub const UPDATE_ENV: &str = "PRIME_RADIANT_UPDATE_SNAPSHOTS";

/// A directory of golden PNGs plus the tolerances they are compared with.
///
/// A store may carry a *variant*, the name of the backend that rendered the
/// images it checks (see [`Self::with_variant`]). Each case then has the
/// shared golden `<name>.png` and, only where that backend's correct output
/// differs from it beyond the store's tolerance, an override
/// `<name>.<variant>.png` beside it.
#[derive(Debug)]
pub struct Screenshots {
    goldens: PathBuf,
    artifacts: PathBuf,
    options: CompareOptions,
    update: bool,
    variant: Option<&'static str>,
}

impl Screenshots {
    /// Opens a golden directory with a sibling failure-artifact directory.
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
            variant: None,
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

    /// Names the backend whose renderings this store checks; `None`, the
    /// default, keeps only shared goldens.
    ///
    /// With a variant, [`Self::check`] compares against `<name>.<variant>.png`
    /// when it exists and against the shared `<name>.png` otherwise, and
    /// update mode writes the override only where the rendering differs from
    /// the shared golden. A variant never modifies a shared golden that
    /// exists. Panics when the variant is not a plain, non-empty file-name
    /// component.
    #[must_use]
    pub fn with_variant(mut self, variant: Option<&'static str>) -> Self {
        if let Some(variant) = variant {
            assert!(
                !variant.is_empty() && !variant.contains(['/', '\\', '.']),
                "screenshot variant {variant:?} must be a plain name without dots"
            );
        }
        self.variant = variant;
        self
    }

    /// The backend this store's override goldens are named for.
    #[must_use]
    pub const fn variant(&self) -> Option<&'static str> {
        self.variant
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

    /// Resolves path segments to a PNG inside the golden directory.
    /// Panics when the name is empty or a segment can escape the store.
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

    /// Resolves path segments to this store's backend override,
    /// `<name>.<variant>.png` beside [`Self::path`]; `None` without a variant.
    /// Panics as [`Self::path`] does.
    #[must_use]
    pub fn variant_path(&self, name: &[&str]) -> Option<PathBuf> {
        let shared = self.path(name);
        let variant = self.variant?;
        let last = name.last().expect("checked by `path`");
        Some(shared.with_file_name(format!("{last}.{variant}.png")))
    }

    /// The golden a [`Self::check`] of `name` compares against: the backend
    /// override when this store has a variant and the override exists, the
    /// shared golden otherwise. Panics as [`Self::path`] does.
    #[must_use]
    pub fn golden_path(&self, name: &[&str]) -> PathBuf {
        self.variant_path(name)
            .filter(|path| path.exists())
            .unwrap_or_else(|| self.path(name))
    }

    /// Compares an image with its golden ([`Self::golden_path`]) or accepts
    /// it in update mode ([`Self::accept`]). A missing golden is written as
    /// the shared one.
    pub fn check(&self, name: &[&str], actual: &Image) -> Result<GoldenOutcome, GoldenError> {
        if self.update {
            return self.accept(name, actual);
        }
        let path = self.golden_path(name);
        if !path.exists() {
            actual.write_png(&path).map_err(GoldenError::Io)?;
            return Ok(GoldenOutcome::Written { path });
        }

        let expected = Image::read_png(&path).map_err(GoldenError::Io)?;
        let stem = self.artifact_stem(name, &path);
        if !expected.has_same_size(actual) {
            let artifacts = self.write_artifacts(&stem, &expected, actual, None)?;
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
        let artifacts = self.write_artifacts(&stem, &expected, actual, Some(&comparison))?;
        Ok(GoldenOutcome::Differed {
            path,
            comparison,
            artifacts,
        })
    }

    /// Accepts `actual` as the rendering of `name`: what [`Self::check`] does
    /// in update mode, offered directly to suites that own their own update
    /// switch.
    ///
    /// Without a variant the shared golden is overwritten. With one, the
    /// shared golden is never modified: when `actual` matches it under this
    /// store's options, a stale override is deleted and nothing is written
    /// ([`GoldenOutcome::Kept`]); when it does not, the override is written.
    /// Only a missing shared golden is written as the shared one (and any
    /// override beside it deleted).
    pub fn accept(&self, name: &[&str], actual: &Image) -> Result<GoldenOutcome, GoldenError> {
        let shared = self.path(name);
        let Some(variant) = self.variant_path(name) else {
            actual.write_png(&shared).map_err(GoldenError::Io)?;
            return Ok(GoldenOutcome::Updated { path: shared });
        };
        if !shared.exists() {
            actual.write_png(&shared).map_err(GoldenError::Io)?;
            // The new shared golden is this rendering; an override left from
            // before it would shadow it.
            remove_if_present(&variant)?;
            return Ok(GoldenOutcome::Updated { path: shared });
        }
        let expected = Image::read_png(&shared).map_err(GoldenError::Io)?;
        if !expected.has_same_size(actual) || !compare(&expected, actual, self.options).is_match() {
            actual.write_png(&variant).map_err(GoldenError::Io)?;
            return Ok(GoldenOutcome::Updated { path: variant });
        }
        let removed = remove_if_present(&variant)?.then_some(variant);
        Ok(GoldenOutcome::Kept {
            path: shared,
            removed,
        })
    }

    /// Checks a golden and panics with a failure report.
    /// Panics unless the golden matches or update mode accepts the image.
    pub fn assert_matches(&self, name: &[&str], actual: &Image) {
        match self.check(name, actual) {
            Ok(GoldenOutcome::Matched { .. }) => {}
            Ok(outcome @ (GoldenOutcome::Updated { .. } | GoldenOutcome::Kept { .. })) => {
                eprintln!("{outcome}");
            }
            Ok(outcome) => panic!("{outcome}"),
            Err(error) => panic!("{error}"),
        }
    }

    /// The failure-artifact stem for a comparison against `golden`: the name's
    /// segments joined by `-`, plus `.<variant>` when `golden` is the
    /// override, so the artifacts name the golden that was compared.
    fn artifact_stem(&self, name: &[&str], golden: &Path) -> String {
        let stem = name.join("-");
        match self.variant {
            Some(variant) if self.variant_path(name).as_deref() == Some(golden) => {
                format!("{stem}.{variant}")
            }
            _ => stem,
        }
    }

    fn write_artifacts(
        &self,
        stem: &str,
        expected: &Image,
        actual: &Image,
        comparison: Option<&Comparison>,
    ) -> Result<Artifacts, GoldenError> {
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

/// Deletes `path`, reporting whether there was a file to delete.
fn remove_if_present(path: &Path) -> Result<bool, GoldenError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(GoldenError::Io(error.into())),
    }
}

/// Where the three failure PNGs were written.
#[derive(Debug)]
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

#[derive(Debug)]
#[non_exhaustive]
pub enum GoldenOutcome {
    Matched {
        anti_aliased_pixels: usize,
    },
    Written {
        path: PathBuf,
    },
    /// Update mode wrote `path`: the shared golden, or a variant store's
    /// override where its rendering differs from the shared one.
    Updated {
        path: PathBuf,
    },
    /// Update mode on a variant store found the rendering matching the shared
    /// golden at `path`, left it as it was, and deleted the override that no
    /// longer differed from it, if there was one.
    Kept {
        path: PathBuf,
        removed: Option<PathBuf>,
    },
    Differed {
        path: PathBuf,
        comparison: Comparison,
        artifacts: Artifacts,
    },
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
            Self::Kept { path, removed } => {
                write!(
                    formatter,
                    "the rendering matches the shared golden {}",
                    path.display()
                )?;
                if let Some(removed) = removed {
                    write!(formatter, "; removed the override {}", removed.display())?;
                }
                Ok(())
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

    /// Gives each test call an isolated golden directory.
    fn store(label: &str) -> Screenshots {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "prime-radiant-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        // The unique root isolates both goldens and their sibling artifacts.
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
        // Dotted names that `set_extension` would collapse remain distinct.
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

    /// Turns update mode on without the process-wide environment switch.
    fn updating(store: Screenshots) -> Screenshots {
        Screenshots {
            update: true,
            ..store
        }
    }

    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    #[test]
    fn a_variant_override_sits_beside_the_shared_golden() {
        let store = store("variant-path").with_variant(Some("gpu"));
        let path = store.variant_path(&["group", "case"]).unwrap();
        assert!(path.ends_with("group/case.gpu.png"), "{}", path.display());
        assert_eq!(path.parent(), store.path(&["group", "case"]).parent());
        assert_eq!(store.with_variant(None).variant_path(&["case"]), None);
    }

    #[test]
    #[should_panic(expected = "plain name")]
    fn a_variant_cannot_carry_a_dot() {
        let _ = store("variant-dot").with_variant(Some("a.b"));
    }

    #[test]
    fn a_variant_check_with_no_golden_writes_the_shared_one() {
        let store = store("variant-missing").with_variant(Some("gpu"));
        let image = solid(2, 2, BLACK);
        let outcome = store.check(&["case"], &image).unwrap();
        let GoldenOutcome::Written { path } = outcome else {
            panic!("expected a written golden, got {outcome:?}");
        };
        assert_eq!(path, store.path(&["case"]));
        assert!(!store.variant_path(&["case"]).unwrap().exists());
    }

    #[test]
    fn a_variant_check_without_an_override_compares_the_shared_golden() {
        let store = store("variant-shared").with_variant(Some("gpu"));
        solid(2, 2, BLACK).write_png(store.path(&["case"])).unwrap();

        assert!(matches!(
            store.check(&["case"], &solid(2, 2, BLACK)).unwrap(),
            GoldenOutcome::Matched { .. }
        ));
        let outcome = store.check(&["case"], &solid(2, 2, WHITE)).unwrap();
        let GoldenOutcome::Differed {
            path, artifacts, ..
        } = outcome
        else {
            panic!("expected a difference, got {outcome:?}");
        };
        assert_eq!(path, store.path(&["case"]));
        assert!(artifacts.expected.ends_with("case-expected.png"));
    }

    #[test]
    fn a_variant_check_prefers_its_override() {
        let store = store("variant-override").with_variant(Some("gpu"));
        let shared = store.path(&["case"]);
        let variant = store.variant_path(&["case"]).unwrap();
        solid(2, 2, BLACK).write_png(&shared).unwrap();
        solid(2, 2, WHITE).write_png(&variant).unwrap();
        assert_eq!(store.golden_path(&["case"]), variant);

        assert!(matches!(
            store.check(&["case"], &solid(2, 2, WHITE)).unwrap(),
            GoldenOutcome::Matched { .. }
        ));
        let outcome = store.check(&["case"], &solid(2, 2, BLACK)).unwrap();
        let GoldenOutcome::Differed {
            path, artifacts, ..
        } = outcome
        else {
            panic!("expected a difference, got {outcome:?}");
        };
        assert_eq!(path, variant);
        assert!(artifacts.expected.ends_with("case.gpu-expected.png"));
        assert_eq!(
            Image::read_png(&artifacts.expected).unwrap(),
            solid(2, 2, WHITE)
        );
        // A store without the variant still reads only the shared golden.
        let plain = Screenshots::new(shared.parent().unwrap());
        assert!(matches!(
            plain.check(&["case"], &solid(2, 2, BLACK)).unwrap(),
            GoldenOutcome::Matched { .. }
        ));
    }

    #[test]
    fn a_matching_variant_update_keeps_the_shared_golden_and_drops_a_stale_override() {
        let store = updating(store("variant-update-match").with_variant(Some("gpu")));
        let shared = store.path(&["case"]);
        let variant = store.variant_path(&["case"]).unwrap();
        solid(2, 2, BLACK).write_png(&shared).unwrap();
        solid(2, 2, WHITE).write_png(&variant).unwrap();
        let before = std::fs::read(&shared).unwrap();

        let outcome = store.check(&["case"], &solid(2, 2, BLACK)).unwrap();
        let GoldenOutcome::Kept { path, removed } = outcome else {
            panic!("expected the shared golden kept, got {outcome:?}");
        };
        assert_eq!(path, shared);
        assert_eq!(removed.as_ref(), Some(&variant));
        assert!(!variant.exists());
        assert_eq!(std::fs::read(&shared).unwrap(), before);

        // With no override to drop, nothing is written or removed.
        let outcome = store.check(&["case"], &solid(2, 2, BLACK)).unwrap();
        assert!(
            matches!(outcome, GoldenOutcome::Kept { removed: None, .. }),
            "{outcome:?}"
        );
        assert!(!variant.exists());
    }

    #[test]
    fn a_differing_variant_update_writes_only_the_override() {
        let store = updating(store("variant-update-differ").with_variant(Some("gpu")));
        let shared = store.path(&["case"]);
        let variant = store.variant_path(&["case"]).unwrap();
        solid(2, 2, BLACK).write_png(&shared).unwrap();
        let before = std::fs::read(&shared).unwrap();

        for actual in [solid(2, 2, WHITE), solid(3, 3, BLACK)] {
            let outcome = store.check(&["case"], &actual).unwrap();
            let GoldenOutcome::Updated { path } = outcome else {
                panic!("expected an override written, got {outcome:?}");
            };
            assert_eq!(path, variant);
            assert_eq!(Image::read_png(&variant).unwrap(), actual);
            assert_eq!(std::fs::read(&shared).unwrap(), before);
        }
    }

    #[test]
    fn a_variant_update_with_no_shared_golden_writes_the_shared_one() {
        let store = updating(store("variant-update-missing").with_variant(Some("gpu")));
        let image = solid(2, 2, BLACK);
        let outcome = store.check(&["case"], &image).unwrap();
        let GoldenOutcome::Updated { path } = outcome else {
            panic!("expected the shared golden written, got {outcome:?}");
        };
        assert_eq!(path, store.path(&["case"]));
        assert_eq!(Image::read_png(&path).unwrap(), image);
        assert!(!store.variant_path(&["case"]).unwrap().exists());
    }

    #[test]
    fn an_update_without_a_variant_overwrites_the_shared_golden() {
        let store = updating(store("plain-update"));
        solid(2, 2, BLACK).write_png(store.path(&["case"])).unwrap();
        let outcome = store.check(&["case"], &solid(2, 2, WHITE)).unwrap();
        let GoldenOutcome::Updated { path } = outcome else {
            panic!("expected the shared golden written, got {outcome:?}");
        };
        assert_eq!(path, store.path(&["case"]));
        assert_eq!(Image::read_png(&path).unwrap(), solid(2, 2, WHITE));
    }
}
