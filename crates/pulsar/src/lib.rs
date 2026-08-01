//! `pulsar` — the vello-backed paint engine.
//!
//! [`Painter::paint`] turns one [`dom::visual::PaintOrder`] frame into a
//! [`vello::Scene`]: it walks the flat back-to-front item list, threads the
//! overflow/`contain: paint` clip chains and group-effect
//! [`RenderLayer`](dom::visual::RenderLayer)s through vello's layer
//! stack, and paints each box's CSS fragments (box-shadow, background
//! color/gradient/image layers, borders, outline) and each text run's
//! retained Parley glyphs. [`gpu`] owns the wgpu side: device/queue
//! management plus headless render-to-texture with readback for tests and
//! embedders without a surface.
//! [`Pulsar`] is the retained integration used by Bobcat documents: it owns a
//! reusable painter and image store, implements
//! [`dom::visual::DocumentRenderer`], and lends its latest scene through that
//! trait's GAT output without cloning or dynamic renderer dispatch.
//!
//! Coordinate model: `PaintOrder` speaks viewport CSS px; the document
//! device's pixel ratio — the same value that drove layout rounding — is
//! applied once as a root transform, so every painter works in CSS px.
//! Viewport and scale are read from [`dom::Document::device`] rather
//! than passed in, so they can never disagree with the layout they
//! produced. Fragment geometry (border/padding/content
//! boxes) comes from the document's rounded layouts; per-item style access
//! uses [`dom::Document::paint_style`], the post-flush borrow that
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
//!   at three border-box corners (vello transforms are affine); hit testing in `dom` stays exact.
//! - Lynx's `background-clip: border-area` skips its layer. `background-clip: text` clips via
//!   glyph-silhouette `SrcIn` sandwiches over the element's descendant text; the silhouette is
//!   glyph ink only (decorations excluded) and ignores descendant `transform`s.
//! - Gradient-valued `color` (Lynx's text-gradient sugar) fills the glyph ink with a gradient brush
//!   anchored to the styled element's padding box. Decorations stay solid: `currentcolor` resolves
//!   through the fork's parallel solid color (opaque black), as it does in Lynx's own style engine.
//! - `text-shadow` paints offset and color but not blur; `overline` is compiled out of the fork's
//!   `text-decoration-line`.
//! - `outline` paints a flush ring with its element (the fork's lynx grammar deliberately has no
//!   `outline-offset` — Lynx outlines are flush); CSS2 Appendix E step 10 would batch outlines atop
//!   the whole stacking context instead.
//! - `mask-*` honors the full geometry longhands but paints the first `mask-image` layer only
//!   (`mask-composite` ignored; `mask-mode: luminance` treated as alpha via the `SrcIn` sandwich).
//! - Replaced content honors `object-fit`, `object-position` and `image-rendering`, and derives its
//!   concrete object size from the element's natural size rather than the decoded pixel dimensions,
//!   so decode-time downsampling cannot skew it. `image-rendering` never selects vello's bicubic
//!   sampler: the fork's grammar has only `auto`, `crisp-edges` and `pixelated`, which map to
//!   bilinear and nearest.
//! - The fork's grammar has no `image-orientation`, so the CSS initial value `from-image` cannot be
//!   authored away: EXIF orientation is applied by the decoder (`crates/image`), and the natural
//!   size a replaced box lays out at is already the oriented one.

// The coverage run compiles with `--cfg coverage_nightly` and the test
// modules opt out via `#[coverage(off)]`, which needs this experimental
// feature (same pattern as every other workspace crate).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The paint modules convert f32 CSS px into f64 kurbo geometry pervasively;
// truncation/precision lints would drown the real signal.
#![allow(clippy::cast_possible_truncation, clippy::cast_lossless)]

use std::cell::Ref;

use dom::Document;
use dom::visual::{DocumentRenderer, PaintOrder};
use vello::Scene;

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
    pub fn paint<T, R>(
        &mut self,
        document: &Document<T, R>,
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

/// The renderer injected into a runtime [`Document`].
///
/// It retains both scene-building allocations and decoded image registrations
/// with the document instead of making each embedder assemble a parallel
/// frame pipeline.
#[derive(Debug, Default)]
pub struct Pulsar {
    painter: Painter,
    images: ImageStore,
}

impl Pulsar {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn images(&self) -> &ImageStore {
        &self.images
    }

    pub const fn images_mut(&mut self) -> &mut ImageStore {
        &mut self.images
    }
}

impl<T> DocumentRenderer<T> for Pulsar {
    type Output<'a>
        = Ref<'a, Scene>
    where
        Self: 'a,
        T: 'a;

    fn render(&mut self, document: &Document<T, Self>, frame: &PaintOrder) {
        self.painter.paint(document, frame, &self.images);
    }

    fn output<'a>(renderer: Ref<'a, Self>) -> Self::Output<'a>
    where
        T: 'a,
    {
        Ref::map(renderer, |renderer| renderer.painter.scene())
    }
}
