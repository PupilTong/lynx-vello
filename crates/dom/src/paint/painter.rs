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
//! - The grammar has no `image-orientation`; the embedder's `ImageStore` is expected to apply EXIF
//!   orientation before it publishes pixels and natural size.

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
            document.image_store().as_ref(),
        );
        let (fragments, program, pool) = assembly.finish();
        self.spare_scenes = pool;
        // The committed-offset composition is built here, at commit, from
        // pooled storage — or skipped entirely for the one-fragment frame
        // shape, whose single untranslated append would only copy the
        // encoding it already holds.
        let committed = if matches!(
            program.as_slice(),
            [crate::paint::compose::ComposeOp::Fragment { index: 0, chain }]
                if *chain == crate::paint::compose::ComposeChain::default()
        ) {
            crate::visual::frame::CommittedScene::Whole
        } else {
            let mut scene = self.spare_scenes.pop().unwrap_or_default();
            scene.reset();
            // Committed values throughout: no offset overrides, and every
            // animation slot sampled as the committed instant.
            let samples = frame.sample_animations(None);
            crate::paint::compose::replay(
                &mut scene,
                &fragments,
                &program,
                frame.slots(),
                &samples,
                device_pixel_ratio,
                &|_| None,
            );
            crate::visual::frame::CommittedScene::Composed(scene)
        };
        let committed = Arc::new(CommittedFrame {
            order: frame,
            presentation: crate::visual::frame::Presentation::new(fragments, program, committed),
            animations_active,
            needs_main_ticks,
            viewport,
            device_pixel_ratio,
        });
        // Reclaiming here, past the point where the walk can fail, is what
        // keeps a frame retained at every instant. A frame that retired
        // while it was still published comes back one paint later, once the
        // publish after it released the outside copy; one still shared even
        // then is given up rather than waited on.
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
        let (mut fragments, mut program, composed) = inner.presentation.into_parts();
        for mut scene in fragments.drain(..) {
            scene.reset();
            self.spare_scenes.push(scene);
        }
        self.spare_fragments = fragments;
        program.clear();
        self.spare_program = program;
        if let Some(mut scene) = composed {
            scene.reset();
            self.spare_scenes.push(scene);
        }
    }

    /// The spare frame buffers' and the build scratch's capacities, for the
    /// reuse tests.
    #[cfg(test)]
    pub(crate) fn storage_capacities(&self) -> ([usize; 5], Vec<usize>) {
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
