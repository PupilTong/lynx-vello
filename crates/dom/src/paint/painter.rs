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
use crate::visual::PaintOrder;
use crate::{Document, ImageStore};

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
    pub(crate) fn paint<T>(&mut self, document: &Document<T>, frame: &PaintOrder) {
        self.scene_epoch = None;
        self.scene.reset();
        crate::paint::walker::walk(
            &mut self.scene,
            &mut self.scratch,
            document,
            frame,
            &self.images,
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
    fn a_failed_paint_cannot_leave_a_partial_scene_marked_current() {
        let mut document = Document::new(crate::tree::document::tests::device());
        document.add_stylesheet(
            "page { width: 10px; height: 10px; background-color: teal; }",
            StylesheetOrigin::Author,
        );
        let root = document.create_element("page", ());
        document.append_document_element(root);
        let frame = document.build_paint_order();

        document.set_inline_style(root, "display: none");
        document.layout();

        let mut painter = document.painter.take();
        let current_epoch = document.visual_epoch();
        painter.scene_epoch = Some(current_epoch);
        let result = catch_unwind(AssertUnwindSafe(|| painter.paint(&document, &frame)));

        assert!(result.is_err(), "the stale frame must fail closed");
        assert!(painter.needs_render(current_epoch));
    }
}
