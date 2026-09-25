//! Private document-owned scene builder.
//!
//! The painter walks DOM's private back-to-front visual order, applies the
//! document Device's pixel ratio once at the root, threads clip chains and
//! group-effect layers through Vello, and paints box fragments plus retained
//! Parley glyph runs. It reads computed styles and rounded layouts directly
//! from the same document, so viewport, scale, geometry, resources, and scene
//! construction cannot be assembled from divergent owners. What it produces
//! is an immutable [`CommittedFrame`] behind an `Arc`: retained here for the
//! document's own hit queries, and published as-is to whichever thread
//! composites and routes input without the document.
//!
//! Deliberate v1 limits (the compatibility bar is behavioral, not
//! pixel-perfect):
//!
//! - `filter: blur()` bakes the group offscreen and blurs it on the GPU (`render/blur.rs`), which
//!   costs four recorded approximations: sigma is isotropic, scaled by the arithmetic mean of the
//!   two singular values of the group's local-to-viewport linear map, so a non-uniform scale or a
//!   skew gets one sigma where the spec's filter region is anisotropic; `filter` is never exported
//!   as a composite curve, so an animated blur recommits and re-bakes every tick; the bakes share a
//!   device-pixel area budget and a group past it renders *unblurred* rather than not at all; and
//!   several `blur()` functions in one list fold into the first by variance addition. Color filters
//!   use blend-composite approximations; factors above one are only partially expressible.
//! - `backdrop-filter` bakes the same way, over the prefix of the frame painted before the element
//!   inside its nearest Backdrop Root, and carries every one of those approximations plus six of
//!   its own: `will-change` roots are not honored, so a `backdrop-filter` element inside a
//!   `will-change: opacity` wrapper sees through it (ruled; `isolation: isolate` is not in the
//!   spec's Backdrop Root list at all, and is absent from the fork's grammar besides); the mirror
//!   edge mode applies at the element's axis-aligned *device* bounding box, so a rotated element
//!   mirrors at its bbox rather than at its rotated border box — which is what Chromium does; items
//!   the walk culled are absent from the crop of an element straddling the viewport; there is no
//!   composite curve; an element past the shared area budget draws no backdrop at all, leaving the
//!   *unfiltered* backdrop showing; and a backdrop inside an `opacity`, `mask-image`, `clip-path`
//!   or blend root reads that root's content already cut by the root's ancestors' clips, which a
//!   `filter` or `backdrop-filter` root does not do. `docs/tracking/deviations.md` records the set.
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
//! - The grammar has no `image-orientation`; the embedder's resource system is expected to apply
//!   EXIF orientation before it reports natural size and serves pixels.

use std::sync::Arc;

use euclid::default::Size2D;

use crate::Document;
use crate::vello::Scene;
use crate::visual::{CommittedFrame, PaintOrder};

/// Reusable document-owned scene builder state.
#[derive(Default)]
pub(crate) struct Painter {
    scratch: crate::paint::walker::Scratch,
    build_scratch: crate::visual::BuildScratch,
    /// The `content-visibility: auto` determination's working storage, kept
    /// here for the same reason `build_scratch` is: a settled page must
    /// determine relevance without allocating.
    relevance_scratch: crate::visual::relevance::RelevanceScratch,
    /// The frame the last successful paint committed — the hit-test snapshot,
    /// and the object [`Document::commit`](crate::Document::commit) publishes.
    frame: Option<Arc<CommittedFrame>>,
    /// A frame retired while something else still held it — in the engine's
    /// flow the frame hub always still holds the previous commit at the
    /// moment it retires here, and releases it only when the commit after it
    /// publishes. Reclaimed at the next paint, one retirement late.
    retiring: Option<Arc<CommittedFrame>>,
    /// Storage reclaimed from a retired frame nobody else was still holding,
    /// kept for the next build.
    ///
    /// Kept apart from `frame` because `frame` is the one thing hit testing
    /// can read between renders: emptying it in place would leave the
    /// document frameless for the length of a build, and permanently
    /// frameless if that build panicked.
    spare: crate::visual::FrameBuffers,
    /// Retired frames' scenes — fragments and committed compositions —
    /// emptied but with their encoding capacity intact, pooled for the next
    /// paint's fragments and composition.
    spare_scenes: Vec<Scene>,
    /// A retired frame's emptied fragment container, capacity intact.
    spare_fragments: Vec<Scene>,
    /// A retired frame's emptied compose program, capacity intact.
    spare_program: Vec<crate::paint::compose::ComposeOp>,
    /// A retired frame's emptied image-draw table, capacity intact.
    spare_image_draws: Vec<crate::paint::compose::ImageDraw>,
    /// A retired frame's emptied filter-group table, capacity intact.
    spare_filter_groups: Vec<crate::paint::compose::FilterGroup>,
    /// A retired frame's emptied composed-slot lists, capacity intact.
    spare_composed: crate::visual::ComposedSlots,
    /// Scratch for marking the slots each committed program composes.
    composed_marks: crate::visual::ComposedMarks,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Painter").finish_non_exhaustive()
    }
}

impl Painter {
    pub(crate) fn paint<T>(
        &mut self,
        document: &Document<T>,
        frame: PaintOrder,
        animations_active: bool,
        needs_main_ticks: bool,
        viewport: Size2D<f32>,
        device_pixel_ratio: f32,
    ) {
        let mut assembly = crate::paint::compose::ComposeAssembly::with_storage(
            std::mem::take(&mut self.spare_fragments),
            std::mem::take(&mut self.spare_program),
            std::mem::take(&mut self.spare_image_draws),
            std::mem::take(&mut self.spare_filter_groups),
            std::mem::take(&mut self.spare_scenes),
        );
        // A panicking walk drops the half-encoded assembly here and leaves
        // the previously committed frame retained: the frame either advances
        // whole or not at all.
        crate::paint::walker::walk_compose(
            &mut assembly,
            &mut self.scratch,
            document,
            &frame,
            document.images(),
        );
        let crate::paint::compose::Finished {
            fragments,
            program,
            image_draws,
            filter_groups,
            pool,
        } = assembly.finish();
        self.spare_scenes = pool;
        let mut composed = std::mem::take(&mut self.spare_composed);
        frame.mark_composed_spaces(
            &program,
            &filter_groups,
            &mut self.composed_marks,
            &mut composed,
        );
        let earliest_expiry = frame.earliest_expiry();
        let committed = Arc::new(CommittedFrame {
            order: frame,
            presentation: crate::visual::frame::Presentation {
                fragments,
                program,
                image_draws,
                filter_groups,
                composed,
            },
            animations_active,
            needs_main_ticks,
            earliest_expiry,
            viewport,
            device_pixel_ratio,
        });
        // Reclaiming here, past the point where the walk can fail, is what
        // keeps a frame retained at every instant. A frame still shared at
        // reclaim time is given up rather than waited on.
        if let Some(waiting) = self.retiring.take()
            && let Ok(inner) = Arc::try_unwrap(waiting)
        {
            self.reclaim(inner);
        }
        if let Some(retired) = self.frame.replace(committed) {
            match Arc::try_unwrap(retired) {
                Ok(inner) => self.reclaim(inner),
                Err(shared) => self.retiring = Some(shared),
            }
        }
    }

    fn reclaim(&mut self, inner: CommittedFrame) {
        self.spare = inner.order.into_buffers();
        let crate::visual::frame::Presentation {
            mut fragments,
            mut program,
            mut image_draws,
            mut filter_groups,
            mut composed,
        } = inner.presentation;
        for mut scene in fragments.drain(..) {
            scene.reset();
            self.spare_scenes.push(scene);
        }
        self.spare_fragments = fragments;
        program.clear();
        self.spare_program = program;
        image_draws.clear();
        self.spare_image_draws = image_draws;
        filter_groups.clear();
        self.spare_filter_groups = filter_groups;
        composed.clear();
        self.spare_composed = composed;
    }

    /// The spare frame buffers' and the build scratch's capacities, for the
    /// reuse tests.
    #[cfg(test)]
    pub(crate) fn storage_capacities(&self) -> ([usize; 7], Vec<usize>) {
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

    /// Takes back the storage of a paint order that was built and then
    /// discarded — a relevance pass whose flips voided it — so the pass that
    /// replaces it reuses the very buffers it just filled.
    pub(crate) fn restore_spare_buffers(&mut self, discarded: PaintOrder) {
        self.spare = discarded.into_buffers();
    }

    pub(crate) fn take_relevance_scratch(&mut self) -> crate::visual::relevance::RelevanceScratch {
        std::mem::take(&mut self.relevance_scratch)
    }

    pub(crate) fn restore_relevance_scratch(
        &mut self,
        scratch: crate::visual::relevance::RelevanceScratch,
    ) {
        self.relevance_scratch = scratch;
    }

    pub(crate) const fn frame(&self) -> Option<&Arc<CommittedFrame>> {
        self.frame.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use crate::{Document, StylesheetOrigin};

    #[test]
    fn a_failed_paint_cannot_leave_a_partial_frame_committed() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            "page { width: 10px; height: 10px; background-color: teal; }",
            StylesheetOrigin::Author,
        );
        let root = document.document_element().id();
        assert!(document.render(), "the first frame commits");
        let first = document.committed_frame().expect("a frame is retained");
        let stale = document.build_paint_order();

        document.set_inline_style(root, "display: none");
        document.layout();

        let mut painter = document.painter.take();
        let result = catch_unwind(AssertUnwindSafe(|| {
            painter.paint(
                &document,
                stale,
                false,
                false,
                document.viewport_size(),
                1.0,
            );
        }));

        assert!(result.is_err(), "the stale frame must fail closed");
        assert_eq!(
            painter
                .frame()
                .map(|frame| frame.commit_id())
                .expect("the retained frame survives the failed paint"),
            first.commit_id(),
            "the failed paint left the previous commit in place"
        );
    }
}
