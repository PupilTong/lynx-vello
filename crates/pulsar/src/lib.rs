//! `pulsar` — the vello-backed paint engine.
//!
//! [`Painter::paint`] turns one [`w3c_dom::visual::PaintOrder`] frame into a
//! [`vello::Scene`]: it walks the flat back-to-front item list, threads the
//! overflow/`contain: paint` clip chains and group-effect
//! [`RenderLayer`](w3c_dom::visual::RenderLayer)s through vello's layer
//! stack, and paints each box's CSS fragments (box-shadow, background
//! color/gradient/image layers, borders, outline) and each text run's
//! retained Parley glyphs. [`gpu`] owns the wgpu side: device/queue
//! management plus headless render-to-texture with readback for tests and
//! embedders without a surface.
//!
//! Coordinate model: `PaintOrder` speaks viewport CSS px; the document
//! device's pixel ratio — the same value that drove layout rounding — is
//! applied once as a root transform, so every painter works in CSS px.
//! Viewport and scale are read from [`w3c_dom::Document::device`] rather
//! than passed in, so they can never disagree with the layout they
//! produced. Fragment geometry (border/padding/content
//! boxes) comes from the document's rounded layouts; per-item style access
//! uses [`w3c_dom::Document::paint_style`], the post-flush borrow that
//! neither re-enters Stylo's borrow checker nor bumps a style `Arc`.
//!
//! Deliberate v1 limits (the compat bar is behavioral, not pixel-perfect —
//! see AGENTS.md):
//! - `filter: blur()` needs an offscreen texture pass vello scenes don't express; it is ignored.
//!   Color filters run as blend-mode composites inside the group layer: `brightness(f ≤ 1)` and
//!   `contrast(f < 1)` are exact, `grayscale`/`saturate` use HSL saturation rather than the spec's
//!   luminance matrix, `brightness(f > 1)` screens approximately, and `saturate(f > 1)` /
//!   `contrast(f > 1)` are inexpressible with flat blends and skipped.
//! - Perspective-projected items are painted with the affine map agreeing with the true projection
//!   at three border-box corners (vello transforms are affine); hit testing in `w3c-dom` stays
//!   exact.
//! - Lynx's `background-clip: border-area` skips its layer, and gradient-valued `color`
//!   (text-gradient sugar) paints the fork's parallel solid color. `background-clip: text` clips
//!   via glyph-silhouette `SrcIn` sandwiches over the element's descendant text; the silhouette is
//!   glyph ink only (decorations excluded) and ignores descendant `transform`s.
//! - `text-shadow` paints offset and color but not blur; `overline` is compiled out of the fork's
//!   `text-decoration-line`.
//! - `outline` paints a flush ring with its element (the fork's lynx grammar deliberately has no
//!   `outline-offset` — Lynx outlines are flush); CSS2 Appendix E step 10 would batch outlines atop
//!   the whole stacking context instead.
//! - `mask-*` honors the full geometry longhands but paints the first `mask-image` layer only
//!   (`mask-composite` ignored; `mask-mode: luminance` treated as alpha via the `SrcIn` sandwich).

// The coverage run compiles with `--cfg coverage_nightly` and the test
// modules opt out via `#[coverage(off)]`, which needs this experimental
// feature (same pattern as every other workspace crate).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The paint modules convert f32 CSS px into f64 kurbo geometry pervasively;
// truncation/precision lints would drown the real signal.
#![allow(clippy::cast_possible_truncation, clippy::cast_lossless)]

use vello::Scene;
use w3c_dom::Document;
use w3c_dom::visual::PaintOrder;

mod convert;
pub mod gpu;
mod images;
mod paint;
mod shape;
mod walker;

pub use images::ImageStore;
/// Embedders configure wgpu/peniko/kurbo exclusively through this re-export
/// (one shared copy, version-matched to vello).
pub use vello;

/// Reusable scene builder. Holding one `Painter` across frames reuses the
/// scene and scratch allocations; [`Self::paint`] rebuilds the scene from
/// scratch each call (retained/damage-driven encoding is a recorded
/// follow-up keyed on `StyleDamage`'s repaint class).
#[derive(Default)]
pub struct Painter {
    scene: Scene,
    scratch: walker::Scratch,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Painter").finish_non_exhaustive()
    }
}

impl Painter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Paints one frame. Computed styles, rounded layouts, and text
    /// layouts are read live from `document`, so the frame must still
    /// truthfully name its nodes: rebuild it after any structural
    /// mutation.
    ///
    /// # Panics
    ///
    /// Panics when the document saw **any** visual-affecting mutation after
    /// `frame` was built (`PaintOrder::assert_visually_fresh` — stricter
    /// than hit testing's removal-only rule, because painting resolves the
    /// frame's geometry snapshot against live styles/layouts/text), and
    /// when a completed style traversal is missing
    /// ([`Document::paint_style`]'s readiness gate).
    pub fn paint<T>(
        &mut self,
        document: &Document<T>,
        frame: &PaintOrder,
        images: &ImageStore,
    ) -> &Scene {
        self.scene.reset();
        walker::walk(&mut self.scene, &mut self.scratch, document, frame, images);
        &self.scene
    }

    /// The scene built by the last [`Self::paint`] call.
    #[must_use]
    pub fn scene(&self) -> &Scene {
        &self.scene
    }
}
