//! The `content-visibility: auto` rendering update
//! ([css-contain-2 §4.1](https://drafts.csswg.org/css-contain-2/#relevant-to-the-user)):
//! which `auto` boxes are relevant to the user, decided inside the commit
//! that draws them.
//!
//! # What "relevant" means here
//!
//! The spec's relevance test is "intersects the viewport, *or* an
//! implementation-defined margin around it". This engine's margin starts at
//! the painter's **encode window**: the region
//! [`ScrollSlot::encode_window`](crate::visual::ScrollSlot::encode_window)
//! admits around every enclosing scroll container's committed offset, which
//! is exactly the region the walk's culling admits and the compositor may
//! scroll to without a new commit. Choosing the same region twice is not a
//! coincidence — it makes *"an `auto` box is relevant wherever its contents
//! could reach the scene"* true by construction rather than by agreement:
//! the test is [`CullPlan::admits_auto_box`], the very call
//! [`plan_frame`](crate::paint::walker) makes for its own first cull test,
//! over the same resolved clip chains, windows and group blur reaches.
//!
//! The region is then stretched toward where each container is going
//! ([`ScrollSlot::relevance_window`](crate::visual::ScrollSlot::relevance_window)):
//! by the target of its pending programmatic scroll, else by the reach the
//! painter last posted for it — a fling's remaining travel, a glide's
//! remaining distance
//! ([`ScrollEntry::reach`](crate::painter::ScrollEntry::reach)) — on the
//! side it points to, at most
//! [`RELEVANCE_REACH_SCROLLPORTS`](crate::visual::frame::RELEVANCE_REACH_SCROLLPORTS)
//! and never past the range. The stretch only ever grows the region, so the
//! relevance region always contains the encode window and the sentence
//! above still holds by construction; what it adds is the margin chosen from
//! where the container is going. A fling's landing is decided relevant in
//! the commit the fling's own recentering asks for, so the boxes it is about
//! to show have their real sizes before it gets there, and a jump's landing
//! in the commit that carries the request. The walk itself keeps the encode
//! window; this pass alone resolves the stretched one
//! ([`CullPlan::resolve_for_relevance`]).
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
//! in a **view transition**. The top layer exists (`tree::top_layer`), and
//! its clause sits beside the geometric test in
//! [`Document::determine_relevance`]. The other three are N/A: `terminus` has no
//! focus model, no selection, and no view transitions. When any of them
//! arrives, it belongs there too, beside the geometric test, never instead
//! of it.
//!
//! # The pass loop
//!
//! [`Document::render`](crate::tree::document::Document::render) claims one commit id, builds
//! one frame, and asks this module. A pass that flips nothing is the end of
//! the render — the overwhelmingly common case, and the reason an idle
//! document still builds exactly once. A pass that flips something invalidates
//! layout at the flipped nodes through the ordinary relayout machinery and the
//! frame is rebuilt. The next pass asks the boxes this render has not
//! determined yet — the nested `auto` boxes that only now got an item — and
//! asks again the ones an earlier pass skipped, but only whether they are now
//! relevant: a reveal swaps a box's `contain-intrinsic-size` for its real
//! size, which moves every box after it, and a skipped box the move brought
//! into the region must not stay blank there. Nothing this render revealed or
//! newly skipped is asked again ([`Freshness`]), so each box's bit moves at
//! most once per commit and the loop ends.
//!
//! [`RELEVANCE_PASSES`] caps the builds one commit spends. A frame published
//! at the cap still carries a skipped box the region admits whenever a list
//! of placeholders reveals a few boxes per pass — an estimate one scrollport
//! tall, the `<list>` default, reveals about three. Such a frame says so
//! ([`Document::relevance_pending`]): it asks for main-thread ticks, and the
//! host's next frame tick calls [`Document::resume_relevance`], whose render
//! continues where this one stopped. HTML's rendering update does the same:
//! a determination other than the initial one "take[s] effect at the next
//! rendering opportunity".
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
//! order, for the host: it drains them with
//! [`Document::take_content_visibility_changes`] and fires each through its
//! own dispatch (`bobcat-core` reaches components and realms alike), or has
//! [`Document::dispatch_content_visibility_changes`] fire them over the
//! standard's path to the engine's own components alone (see
//! [`crate::event`]). *When* that happens is not this module's and not this
//! crate's: the spec dispatches the event "by posting a task at the time
//! when the state change occurs", so the host calls it from the task it
//! posts and the event is never delivered inside the commit that produced
//! it. A node freed in between stays queued and resolves to nothing at
//! dispatch — a [`NodeId`] is retired on free and never reissued.

use crate::layout::relevance::{Freshness, Relevance};
use crate::paint::walker::CullPlan;
use crate::tree::document::{Document, NodeId};
use crate::visual::{AutoBox, PaintOrder};

pub(crate) const RELEVANCE_PASSES: u32 = 4;

const CONTENT_VISIBILITY_AUTO_STATE_CHANGE: &str = "contentvisibilityautostatechange";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentVisibilityChange {
    pub node: NodeId,
    pub skipped: bool,
}

#[derive(Debug, Default)]
pub(crate) struct RelevanceScratch {
    plan: CullPlan,
    flips: Vec<NodeId>,
}

impl<T: Sync> Document<T> {
    pub(crate) fn determine_relevance(
        &mut self,
        scratch: &mut RelevanceScratch,
        frame: &PaintOrder,
    ) -> bool {
        if frame.auto_boxes().is_empty() {
            return false;
        }
        scratch.plan.resolve_for_relevance(self, frame);
        scratch.flips.clear();
        for auto in frame.auto_boxes() {
            let Some(slot) = self.slot(auto.node) else {
                continue;
            };
            let freshness = self.arenas().relevance_freshness(slot);
            if freshness == Freshness::Final {
                continue;
            }
            let relevant = self.is_relevant(&scratch.plan, frame, auto);
            if freshness == Freshness::Revealable && !relevant {
                continue;
            }
            let state = Relevance::of(relevant);
            if self.arenas_mut().determine_relevance(slot, state) {
                scratch.flips.push(auto.node);
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
        for &node in &scratch.flips {
            self.invalidate_layout(node);
        }
        true
    }

    pub(crate) fn relevance_pending(
        &mut self,
        scratch: &mut RelevanceScratch,
        frame: &PaintOrder,
    ) -> bool {
        if frame.auto_boxes().is_empty() {
            return false;
        }
        scratch.plan.resolve_for_relevance(self, frame);
        frame.auto_boxes().iter().any(|auto| {
            self.slot(auto.node).is_some_and(|slot| {
                self.arenas().relevance_freshness(slot) != Freshness::Final
                    && self.arenas().relevance(slot).skips()
                    && self.is_relevant(&scratch.plan, frame, auto)
            })
        })
    }

    fn is_relevant(&self, plan: &CullPlan, frame: &PaintOrder, auto: &AutoBox) -> bool {
        plan.admits_auto_box(frame, auto)
            || self.anchor_keeps_relevant(auto.node)
            || self.in_top_layer(auto.node)
    }

    pub(crate) fn settle_relevance(&mut self) {
        self.arenas_mut().settle_relevance();
    }
}

impl<T> Document<T> {
    #[must_use]
    pub fn take_content_visibility_changes(&mut self) -> Vec<ContentVisibilityChange> {
        if self.content_visibility_changes.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.content_visibility_changes)
    }

    #[must_use]
    pub fn has_pending_content_visibility_changes(&self) -> bool {
        !self.content_visibility_changes.is_empty()
    }

    pub fn dispatch_content_visibility_changes(&mut self, timestamp: f64) {
        for change in self.take_content_visibility_changes() {
            self.dispatch_element_event(
                change.node,
                CONTENT_VISIBILITY_AUTO_STATE_CHANGE,
                &crate::event::EventDetail::ContentVisibility {
                    skipped: change.skipped,
                },
                true,
                false,
                timestamp,
            );
        }
    }
}
