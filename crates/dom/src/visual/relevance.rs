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
//! bound, a box a curve scaling through 0 moves with no clip moving along to
//! bound it — counts as relevant. Skipping needs a proof; painting does not.
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
//!
//! # What the commit leaves for the event
//!
//! [css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event)
//! fires `contentvisibilityautostatechange` "when the rendering state changes
//! and the element either becomes or stops being relevant to the user". That
//! is exactly the move this pass reports: the one
//! [`RelevanceTable::determine`](crate::layout::relevance::RelevanceTable)
//! answers `true` for — a change in *skipping*, not in relevance, which is
//! what makes the spec's initial state fall out rather than need a rule of
//! its own. An undetermined box skips, so the first determination of an
//! on-screen box is a change (skipped → not skipped) and the first
//! determination of an off-screen box is not.
//!
//! Every such change is queued as a [`ContentVisibilityChange`], in frame
//! order, and fired by
//! [`Document::dispatch_content_visibility_changes`] — a walk over the
//! standard's path to the engine's own components, never to script (see
//! [`crate::event`]). *When* it is called is not this module's and not this
//! crate's: the spec dispatches the event "by posting a task at the time
//! when the state change occurs", so the host calls it from the task it
//! posts and the event is never delivered inside the commit that produced
//! it. A node freed in between stays queued and resolves to nothing at
//! dispatch — a [`NodeId`] is retired on free and never reissued.

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

/// One element whose skipping state changed in a commit: what
/// [css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event)
/// fires `contentvisibilityautostatechange` for.
///
/// The queue is published as well as dispatched: it is the record of one
/// commit, so a caller that wants the facts without the walk drains it with
/// [`Document::take_content_visibility_changes`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentVisibilityChange {
    /// The element the state change happened to, which is the event's target.
    pub node: NodeId,
    /// The state it changed **to**, which is the event's `skipped`
    /// attribute: `true` when the element now skips its contents.
    pub skipped: bool,
}

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
                // css-contain-2 §4.4: the skipping state changed, so this
                // element owes a `contentvisibilityautostatechange`. The
                // freshness guard above is what makes one commit push at
                // most one change per element, and the loop is over
                // `auto_boxes()`, so they queue in frame order.
                self.content_visibility_changes
                    .push(ContentVisibilityChange {
                        node: auto.node,
                        skipped: state.skips(),
                    });
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

impl<T> Document<T> {
    /// Takes the `content-visibility: auto` skipping changes the commits
    /// since the last call produced, in frame order, and leaves the queue
    /// empty.
    ///
    /// One entry per element per commit, because a commit moves an element's
    /// bit at most once; two commits without a drain between them leave two
    /// entries for an element that changed in both, in the order the commits
    /// ran, because both are events a listener is owed.
    ///
    /// A commit that changed nothing hands back an empty `Vec`: no
    /// allocation, and the queue's own storage is left where it is rather
    /// than moved out and rebuilt. A drain that does carry changes hands the
    /// queue's storage over with them, which is the one allocation a batch of
    /// events costs.
    #[must_use]
    pub fn take_content_visibility_changes(&mut self) -> Vec<ContentVisibilityChange> {
        if self.content_visibility_changes.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.content_visibility_changes)
    }

    /// Whether any commit since the last drain changed an element's skipping
    /// state — the cheap question a host asks once per entry, before it
    /// decides to post the task that delivers them.
    #[must_use]
    pub fn has_pending_content_visibility_changes(&self) -> bool {
        !self.content_visibility_changes.is_empty()
    }

    /// Fires one `contentvisibilityautostatechange` per queued change, in
    /// frame order, and leaves the queue empty.
    ///
    /// [css-contain-2 §4.4](https://drafts.csswg.org/css-contain-2/#content-visibility-auto-state-change-event)
    /// dispatches the event "by posting a task at the time when the state
    /// change occurs": the queue is this crate's half of that, and calling
    /// this from the task is the host's. Nothing here checks *when* it is
    /// called, so a host that called it inside the commit would merely be
    /// wrong rather than refused — which is why the rule is stated where the
    /// call site is.
    ///
    /// Each change is one [`Document::dispatch_element_event`], so it reaches
    /// engine components alone and never script: `bubbles` is `true` (a
    /// recorded choice where the spec is silent — Chromium bubbles while Safari
    /// and Firefox do not, and w3c/csswg-drafts#11310 is open), `composed` is
    /// `false`, and nothing is cancelable. An element freed between the
    /// commit and this call dispatches nothing at all.
    ///
    /// The queue is drained whether or not anything listens, because it is
    /// the record of one commit rather than a mailbox: a document that
    /// defines no components pays the drain and one `is_empty` check per
    /// change.
    pub fn dispatch_content_visibility_changes(&mut self) {
        // Taken first, so a handler that renders — and queues changes of its
        // own — leaves them for the next drain instead of extending this
        // walk.
        for change in self.take_content_visibility_changes() {
            self.dispatch_element_event(
                change.node,
                crate::event::ElementEventKind::ContentVisibilityAutoStateChange {
                    skipped: change.skipped,
                },
                true,
                false,
            );
        }
    }
}
