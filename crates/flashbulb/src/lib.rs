//! `flashbulb` — screenshot testing infrastructure for lynx-vello.
//!
//! This crate is to lynx-vello's render tests what Playwright's
//! `expect(page).toHaveScreenshot()` is to lynx-stack's `web-core-e2e` and
//! `web-elements` suites: take the picture, compare it to a golden with real
//! tolerances, and leave something useful behind when it does not match.
//!
//! ```no_run
//! # #[cfg(feature = "render")]
//! # fn example(document: &mut dom::Document<()>) {
//! use flashbulb::vello::peniko::Color;
//! use flashbulb::{Screenshots, capture_document, headless};
//!
//! let mut gpu = headless("my_test");
//! let image = capture_document(&mut gpu, document, Color::WHITE).expect("capture");
//! Screenshots::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/screenshots"))
//!     .assert_matches(&["my-case", "index"], &image);
//! # }
//! ```
//!
//! # What it borrows from Playwright, and why
//!
//! - **Comparison is a port of `pixelmatch`** — squared-YIQ per-pixel distance against `35215 *
//!   threshold²`, with anti-aliasing detection — rather than byte equality on the PNG container.
//!   Byte equality is strictly stronger than pixel equality, which is already stronger than
//!   anything a GPU rasterizer guarantees across drivers; and it contradicts the project's own
//!   compatibility bar, which is behavioral rather than pixel-perfect. See [`compare`].
//! - **Captures cover the whole painted frame** — `viewport * device_pixel_ratio` device pixels
//!   (see [`frame_size`]). Playwright captures at `scale: 'css'` instead, downsampling to one image
//!   pixel per CSS pixel; we have no resampler, so the two agree exactly at a device pixel ratio of
//!   1, which is what lynx-stack pins for determinism anyway (`--force-device-scale-factor=1` on
//!   Chromium, `deviceScaleFactor: 1` on Firefox).
//! - **Failures leave `-expected`, `-actual`, and `-diff` PNGs**, and the panic message names all
//!   three plus the exact pixel counts, instead of only offering blanket re-acceptance.
//! - **A newly written golden fails its own run.** Playwright's first run writes and fails too; the
//!   alternative is an unreviewed baseline that passes CI forever.
//!
//! # What it deliberately does not borrow
//!
//! - **No platform or backend suffix on golden filenames.** Playwright commits only `-linux`
//!   goldens and regenerates elsewhere. lynx-vello has one committed golden per case, so
//!   cross-platform rasterizer differences are absorbed by tolerance instead of by per-platform
//!   baselines. If that stops holding, the fix is a suffix in [`Screenshots::path`], not a tighter
//!   threshold.
//! - **No stability loop.** Playwright re-captures until two screenshots agree because a live
//!   browser animates; a `dom` document's internal paint pipeline is a pure function of its own
//!   state.
//!
//! # Features
//!
//! - default: [`Image`], [`compare`], [`Screenshots`] — pixels in, verdict out, no render stack.
//! - `render`: adds [`capture_document`] and [`headless`], which pull in `dom` and `pulsar`.
//!
//! # Captures need a GPU
//!
//! [`headless`] requires a usable adapter. A missing adapter fails the test,
//! including in CI, so a green screenshot suite always means pixels were
//! rendered and compared.

mod compare;
mod golden;
mod image;
#[cfg(feature = "render")]
mod render;

/// Re-exported so capture callers name colors through the same `peniko` the
/// render stack was built against, never a second copy of it.
#[cfg(feature = "render")]
pub use pulsar::vello;

pub use crate::compare::{CompareOptions, Comparison, compare};
pub use crate::golden::{
    Artifacts, GoldenError, GoldenOutcome, Screenshots, UPDATE_ENV, screenshots_in,
};
pub use crate::image::{Image, ImageError};
#[cfg(feature = "render")]
pub use crate::render::{
    CaptureError, capture_document, capture_document_sized, capture_scene, capture_scene_sized,
    frame_size, headless,
};
