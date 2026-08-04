//! Private document-owned scene builder.
//!
//! The painter walks DOM's private back-to-front visual order, applies the
//! document Device's pixel ratio once at the root, threads clip chains and
//! group-effect layers through Vello, and paints box fragments plus retained
//! Parley glyph runs. It reads computed styles and rounded layouts directly
//! from the same document, so viewport, scale, geometry, resources, and scene
//! construction cannot be assembled from divergent owners.
//!
//! Deliberate v1 limits (the compatibility bar is behavioral, not
//! pixel-perfect):
//!
//! - `filter: blur()` needs an offscreen texture pass and is ignored. Color filters use
//!   blend-composite approximations; factors above one are only partially expressible.
//! - Perspective-projected items use the affine map agreeing with the true projection at three
//!   border-box corners because Vello transforms are affine; hit testing remains projectively
//!   exact.
//! - Lynx's `background-clip: border-area` skips its layer. `background-clip: text` uses
//!   glyph-silhouette `SrcIn` sandwiches over descendant text; decorations and descendant
//!   transforms are excluded.
//! - Gradient-valued `color` fills glyph ink from the styled element's padding box. Decorations
//!   remain solid through the fork's parallel color.
//! - `text-shadow` paints offset and color but not blur; `overline` is compiled out of the Lynx
//!   Stylo grammar.
//! - `outline` paints a flush ring with its element. CSS2 Appendix E would batch outlines atop the
//!   whole stacking context.
//! - `mask-*` honors geometry longhands but paints only the first non-`none` image;
//!   `mask-composite` is ignored and luminance mode is treated as alpha.
//! - Replaced content honors `object-fit`, `object-position`, and `image-rendering`. Concrete size
//!   comes from the node's natural size; `auto` maps to bilinear and `crisp-edges`/`pixelated` to
//!   nearest sampling.
//! - The grammar has no `image-orientation`; decoders apply EXIF orientation before publishing
//!   pixels and natural size.

use crate::vello::Scene;
use crate::visual::frame::Frame;
use crate::{ImageStore, Vector2D};

/// Reusable paint state owned by exactly one [`Document`].
///
/// This is deliberately crate-private: DOM mutation, frame construction,
/// image lookup, and scene generation are one scheduling boundary. Embedders
/// can request a render and borrow its result, but cannot drive the painter
/// against an unrelated or stale document snapshot.
#[derive(Default)]
pub(crate) struct Painter {
    scene: Scene,
    scratch: crate::paint::walker::Scratch,
    images: ImageStore,
    /// The document visual epoch represented by `scene`. `None` means this
    /// painter has never completed a frame.
    scene_epoch: Option<u64>,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Painter").finish_non_exhaustive()
    }
}

impl Painter {
    /// Paints one resolved [`Frame`] at the scroll position it was built
    /// with.
    pub(crate) fn paint(&mut self, frame: &Frame) {
        self.paint_scrolled(frame, &[]);
    }

    /// [`Self::paint`] with the renderer's own scroll offsets, indexed by the
    /// frame's scroll arena.
    ///
    /// Where an offset differs from a scroll node's baked one, the enclosed
    /// content is translated to match — so a host can scroll a frame it
    /// already holds, at its own frame rate, without waiting for the document
    /// to restyle, relayout, and republish. `&[]` is exactly [`Self::paint`].
    ///
    /// The correction is a pure viewport-space translation per scroll chain
    /// (see [`ScrollNode`](crate::ScrollNode)), exact for affine ancestor
    /// transforms. It re-runs no layout, so scrolling this way cannot reveal
    /// content the document has not laid out.
    pub(crate) fn paint_scrolled(&mut self, frame: &Frame, offsets: &[Vector2D<f32>]) {
        self.scene_epoch = None;
        self.scene.reset();
        crate::paint::walker::walk(
            &mut self.scene,
            &mut self.scratch,
            frame,
            &self.images,
            offsets,
        );
        // Publish freshness only after the walk completed. If painting
        // panics, the partial scene must remain stale and be rebuilt on the
        // next attempt.
        self.scene_epoch = Some(frame.visual_epoch());
    }

    pub(crate) fn needs_render(&self, visual_epoch: u64) -> bool {
        self.scene_epoch != Some(visual_epoch)
    }

    pub(crate) const fn scene(&self) -> &Scene {
        &self.scene
    }

    pub(crate) const fn images_mut(&mut self) -> &mut ImageStore {
        &mut self.images
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use crate::{Document, StylesheetOrigin};

    #[test]
    fn a_stale_order_fails_closed_before_it_can_reach_the_painter() {
        // The painter is frame-driven now, and a `Frame` owns its own styles,
        // geometry, and paragraphs — so it cannot disagree with itself and
        // there is nothing left to fail closed *at paint time*. The check
        // moved one step earlier, to the resolution boundary, which is where
        // a paint order and the live document actually get mixed.
        let mut document = Document::new(crate::tree::document::tests::device());
        document.add_stylesheet(
            "page { width: 10px; height: 10px; background-color: teal; }",
            StylesheetOrigin::Author,
        );
        let root = document.create_element("page", ());
        document.append_document_element(root);
        let order = document.build_paint_order();

        document.set_inline_style(root, "display: none");
        document.layout();

        let current_epoch = document.visual_epoch();
        let result = catch_unwind(AssertUnwindSafe(|| document.resolve_frame(order)));
        assert!(result.is_err(), "the stale order must fail closed");

        // And the retained scene is untouched by the attempt: nothing was
        // painted, so the next render still has work to do.
        assert!(document.painter.borrow().needs_render(current_epoch));
    }
}
