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

/// Reusable document-owned scene builder state.
#[derive(Default)]
pub(crate) struct Painter {
    scene: Scene,
    scratch: crate::paint::walker::Scratch,
    build_scratch: crate::visual::BuildScratch,
    images: ImageStore,
    scene_epoch: Option<u64>,
    frame: Option<PaintOrder>,
    /// Storage reclaimed from the frame this painter last retired, held for
    /// the next build.
    ///
    /// Kept apart from `frame` because `frame` is the one thing hit testing
    /// can read between renders: emptying it in place would leave the
    /// document frameless for the length of a build, and permanently
    /// frameless if that build panicked.
    spare: crate::visual::FrameBuffers,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Painter").finish_non_exhaustive()
    }
}

/// Painted frames between two replaced-content liveness sweeps.
///
/// The sweep is what bounds `ImageStore`'s node key space: an entry whose
/// owner node no longer exists can never be looked up again, because the
/// arena retires that handle for good. Running it on every frame would cost
/// one arena probe per registered image per frame to reclaim bytes nothing is
/// waiting on, so it runs on one frame in this many instead.
const NODE_SWEEP_INTERVAL_FRAMES: u64 = 64;

impl Painter {
    pub(crate) fn paint<T>(&mut self, document: &Document<T>, frame: PaintOrder) {
        self.scene_epoch = None;
        self.scene.reset();
        self.images.begin_frame();
        crate::paint::walker::walk(
            &mut self.scene,
            &mut self.scratch,
            document,
            &frame,
            &self.images,
        );
        // The sweep reads the document while the painter is mutably borrowed
        // (`Document::render` calls this through `painter.borrow_mut()`), the
        // same way the walk above reads styles and layouts. Nothing it calls
        // may reach back into `Document::painter`: the walk would fail on
        // every frame, but the sweep would fail on one frame in
        // `NODE_SWEEP_INTERVAL_FRAMES`, which is the harder failure to find.
        if self
            .images
            .frame_index()
            .is_multiple_of(NODE_SWEEP_INTERVAL_FRAMES)
        {
            self.images.retain_nodes(|owner| {
                // An owner key that does not decode is kept, not dropped. The
                // registry's key space is opaque `u64`, and `NodeId::from_bits`
                // also refuses a key whose generation field has outgrown its
                // 21 bits, which a long-lived document that recycles one arena
                // slot enough times will reach. Reading either as "the node is
                // gone" would blank a live element's pixels for good, whereas
                // keeping an undecodable key costs only the bytes, and the
                // embedder can still drop it through `remove_node`.
                crate::NodeId::from_bits(owner).is_none_or(|node| document.contains_node(node))
            });
        }
        self.scene_epoch = Some(frame.visual_epoch());
        // Reclaiming here, past the point where the walk can fail, is what
        // keeps a frame retained at every instant.
        if let Some(retired) = self.frame.replace(frame) {
            self.spare = retired.into_buffers();
        }
    }

    /// The spare frame buffers' and the build scratch's capacities, for the
    /// reuse tests.
    #[cfg(test)]
    pub(crate) fn storage_capacities(&self) -> ([usize; 3], Vec<usize>) {
        (self.spare.capacities(), self.build_scratch.capacities())
    }

    pub(crate) fn take_build_scratch(&mut self) -> crate::visual::BuildScratch {
        std::mem::take(&mut self.build_scratch)
    }

    pub(crate) fn restore_build_scratch(&mut self, scratch: crate::visual::BuildScratch) {
        self.build_scratch = scratch;
    }

    pub(crate) fn take_spare_buffers(&mut self) -> crate::visual::FrameBuffers {
        std::mem::take(&mut self.spare)
    }

    pub(crate) const fn frame(&self) -> Option<&PaintOrder> {
        self.frame.as_ref()
    }

    pub(crate) fn needs_render(&self, visual_epoch: u64) -> bool {
        self.scene_epoch != Some(visual_epoch)
    }

    pub(crate) const fn scene(&self) -> &Scene {
        &self.scene
    }

    pub(crate) const fn images(&self) -> &ImageStore {
        &self.images
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
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { width: 10px; height: 10px; background-color: teal; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        let frame = document.build_paint_order();

        document.set_inline_style(root, "display: none");
        document.layout();

        let mut painter = document.painter.take();
        let current_epoch = document.visual_epoch();
        painter.scene_epoch = Some(current_epoch);
        let result = catch_unwind(AssertUnwindSafe(|| painter.paint(&document, frame)));

        assert!(result.is_err(), "the stale frame must fail closed");
        assert!(painter.needs_render(current_epoch));
    }

    #[test]
    fn a_freed_replaced_element_stops_retaining_its_pixels() {
        use crate::vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { width: 20px; height: 20px; } img { width: 2px; height: 2px; }",
            StylesheetOrigin::Author,
        );
        let page = document.document_element().id();
        let image = document.create_element("img", ());
        document.insert_before(page, image, None);
        document.images_mut().insert_node(
            image.to_bits(),
            ImageData {
                data: Blob::from(vec![0_u8; 16]),
                format: ImageFormat::Rgba8,
                alpha_type: ImageAlphaType::Alpha,
                width: 2,
                height: 2,
            },
        );
        document.render();
        assert_eq!(document.painter.borrow().images.node_bytes(), 16);

        document.drop_element(image);
        // The sweep is paced, so the pixels must still be there right up to
        // the frame it runs on. Asserting that pins the interval: an
        // implementation that swept every frame would pass the final
        // assertion on its own.
        let interval = super::NODE_SWEEP_INTERVAL_FRAMES;
        let start = document.painter.borrow().images.frame_index();
        for _ in 0..(interval - 1 - start % interval) {
            document.images_mut();
            document.render();
            assert_eq!(
                document.painter.borrow().images.node_bytes(),
                16,
                "the sweep must not run before its frame",
            );
        }
        document.images_mut();
        document.render();

        assert_eq!(
            document.painter.borrow().images.node_bytes(),
            0,
            "an owner key the arena retired can never be looked up again"
        );
    }
}
