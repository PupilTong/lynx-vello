//! The `content-visibility: auto` rendering update
//! ([css-contain-2 §4.1](https://drafts.csswg.org/css-contain-2/#relevant-to-the-user)):
//! which `auto` boxes are relevant to the user, decided inside the commit
//! that draws them.
//!
//! # What "relevant" means here
//!
//! The spec's relevance test is "intersects the viewport, *or* an
//! implementation-defined margin around it". This engine's margin is the
//! painter's **encode window**: the region
//! [`ScrollSlot::encode_window`](crate::visual::ScrollSlot::encode_window)
//! admits around every enclosing scroll container's committed offset, which
//! is exactly the region the walk's culling admits and the compositor may
//! scroll to without a new commit. Choosing the same region twice is not a
//! coincidence — it makes *"an `auto` box is relevant wherever its contents
//! could reach the scene"* true by construction rather than by agreement:
//! the test is [`CullPlan::admits_auto_box`], the very call
//! [`plan_frame`](crate::paint::walker) makes for its own first cull test,
//! over the same resolved clip chains, encode windows and group blur reaches.
//!
//! The element's own border box is the right box to ask about, because
//! `content-visibility: auto` implies `contain: paint`: descendants are
//! clipped to its padding box, so no content of a box whose border box the
//! region rejects can put ink anywhere. Its own background, border and shadow
//! still paint — skipping contents is not skipping the box.
//!
//! Anything the test cannot decide — a singular transform, a non-finite
//! bound, a chain a sampled animation delta moves — counts as relevant.
//! Skipping needs a proof; painting does not.
//!
//! # Which boxes are asked
//!
//! Every `auto` element the build *reached*: the ones not under a skipped or
//! `display: none` ancestor. That is deliberately not "every `auto` element
//! with a paint item". A `visibility: hidden` element emits no item at all,
//! and relevance is a geometric fact about its border box, not about whether
//! it paints — it decides whether its contents, which may be
//! `visibility: visible`, are skipped. So the builder records an
//! [`AutoBox`](crate::visual::AutoBox) before it decides whether to emit an
//! item, and this pass reads those. An element with no record — under a
//! skipped ancestor, or hidden — keeps whatever state it had, which is how a
//! nested `auto` box waits for the pass after its ancestor revealed.
//!
//! # Conditions this engine has none of
//!
//! css-contain-2 also makes a box relevant when it is in the **top layer**,
//! contains the **focused** element, contains a **selection**, or is captured
//! in a **view transition**. All four are N/A: `dom` has no top layer (no
//! dialog, no fullscreen), no focus model, no selection, and no view
//! transitions. When any of them arrives, it belongs in
//! [`determine`] beside the geometric test, never instead of it.
//!
//! # The pass loop
//!
//! [`Document::render`](crate::Document::render) claims one commit id, builds
//! one frame, and asks this module. A pass that flips nothing is the end of
//! the render — the overwhelmingly common case, and the reason an idle
//! document still builds exactly once. A pass that flips something invalidates
//! layout at the flipped nodes through the ordinary relayout machinery and the
//! frame is rebuilt; the next pass then asks only about boxes this render has
//! *not* determined yet, which are exactly the nested `auto` boxes that only
//! now got an item. So each box's bit moves at most once per commit, the
//! number of passes is bounded by nesting depth, and
//! [`RELEVANCE_PASSES`] caps it regardless. The published frame is always the
//! last pass's, so no frame is ever published with a flip pending.

use crate::NodeId;
use crate::layout::relevance::Relevance;
use crate::paint::walker::CullPlan;
use crate::tree::document::Document;
use crate::visual::PaintOrder;

/// How many times one render may build the paint order.
///
/// A flip only ever reveals *nested* `auto` boxes — a newly skipped box hides
/// its subtree, which cannot introduce an undetermined box — so the passes
/// needed equal the depth of the deepest chain of `auto` boxes that all
/// reveal at once. Four covers every page anyone has, and the cap is what
/// makes the loop's termination a property of the code rather than of the
/// content: past it the last frame built is published with whatever the bits
/// say, one commit behind at worst.
pub(crate) const RELEVANCE_PASSES: u32 = 4;

/// Reusable per-render storage, so a settled page determines relevance with
/// zero allocation.
#[derive(Debug, Default)]
pub(crate) struct RelevanceScratch {
    plan: CullPlan,
    /// The nodes whose bit moved in the pass just run.
    flips: Vec<NodeId>,
}

impl<T: Sync> Document<T> {
    /// Runs one determination pass over `frame`, applies whatever moved, and
    /// answers whether the frame is now stale.
    ///
    /// `false` means the render is finished: either no `auto` box is in the
    /// frame at all, or every one of them is already in the state this frame
    /// was built under.
    pub(crate) fn determine_relevance(
        &mut self,
        scratch: &mut RelevanceScratch,
        frame: &PaintOrder,
    ) -> bool {
        if frame.auto_boxes().is_empty() {
            return false;
        }
        scratch.plan.resolve_for(self, frame);
        scratch.flips.clear();
        for auto in frame.auto_boxes() {
            let Some(slot) = self.slot(auto.node) else {
                continue;
            };
            // Determined by an earlier pass of *this* render: its answer
            // stands, whatever a later layout moved, which is what bounds
            // the loop.
            if self.arenas().relevance_is_fresh(slot) {
                continue;
            }
            let state = Relevance::of(scratch.plan.admits_auto_box(frame, auto));
            if self.arenas_mut().determine_relevance(slot, state) {
                scratch.flips.push(auto.node);
            }
        }
        if scratch.flips.is_empty() {
            return false;
        }
        // Every bit is in place before the first invalidation, because the
        // walk `invalidate_layout` makes reads `skips_contents` on the way
        // up: a half-applied set would stop at an ancestor still claiming to
        // skip.
        for &node in &scratch.flips {
            self.invalidate_layout(node);
        }
        true
    }

    /// Ends the render: the determinations it made stop being *this*
    /// render's, so the next one asks again.
    pub(crate) fn settle_relevance(&mut self) {
        self.arenas_mut().settle_relevance();
    }
}
