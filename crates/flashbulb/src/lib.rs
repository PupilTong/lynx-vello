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
//! use flashbulb::{ImageStore, Screenshots, capture_document, headless_or_skip};
//!
//! let Some(mut gpu) = headless_or_skip("my_test") else {
//!     return;
//! };
//! let images = ImageStore::new();
//! let image = capture_document(&mut gpu, document, Color::WHITE, &images).expect("capture");
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
//!   browser animates; a `dom` document rendered by `pulsar` is a pure function of its own state.
//!
//! # Features
//!
//! - default: [`Image`], [`compare`], [`Screenshots`] — pixels in, verdict out, no renderer.
//! - `render`: adds [`capture_document`] and [`headless_or_skip`], which pull in `dom` and
//!   `pulsar`.
//!
//! # Captures need a GPU
//!
//! [`headless_or_skip`] returns `None` on a machine with no usable adapter and
//! writes `SKIP <test>` straight to the process's stderr, because libtest
//! discards a passing test's captured output. A green run on such a machine
//! has not compared anything; set `FLASHBULB_REQUIRE_GPU=1` where a missing
//! adapter should fail instead.

mod compare;
mod golden;
mod image;
#[cfg(feature = "render")]
mod render;

/// Re-exported so capture callers name colors through the same `peniko` the
/// renderer was built against, never a second copy of it, and can name the
/// image store the capture functions take without dev-depending on `pulsar`.
#[cfg(feature = "render")]
pub use pulsar::{ImageStore, vello};

pub use crate::compare::{CompareOptions, Comparison, compare};
pub use crate::golden::{
    Artifacts, GoldenError, GoldenOutcome, Screenshots, UPDATE_ENV, screenshots_in,
};
pub use crate::image::{Image, ImageError};
#[cfg(feature = "render")]
pub use crate::render::{
    CaptureError, capture_document, capture_document_sized, capture_frame, capture_frame_sized,
    capture_scene, capture_scene_sized, frame_size, headless_or_skip,
};
