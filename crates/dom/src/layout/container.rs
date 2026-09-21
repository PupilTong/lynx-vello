//! The **size query container**
//! ([css-contain-3 §2.1](https://drafts.csswg.org/css-contain-3/#container-type)):
//! the box `cqw` and `cqh` resolve against.
//!
//! Stylo owns the lookup. `container_relative_to_computed_value` asks
//! `Context::get_container_size_query()`, which walks up from the element's
//! *parent* and, at the first ancestor whose `container-type` is a size
//! container type, calls
//! [`TElement::query_container_size`](stylo::dom::TElement::query_container_size).
//! This module is the answer to that call and nothing else: a slot-keyed table
//! of the content-box size each size container had at the end of the last
//! layout that committed it.
//!
//! # Why a table, and why these two halves
//!
//! The answer has to be reachable from `&Node`, because that is all a
//! [`TElement`](stylo::dom::TElement) is — the same structural reason the
//! relevance and last-remembered-size tables live on
//! [`TreeArenas`](crate::tree::TreeArenas). But unlike those two, this one is
//! read **from the style traversal**, which runs on the style pool's worker
//! threads. A `RefCell` read concurrently from several of them races on its
//! borrow flag, so the published half is a plain `Vec` that only an
//! exclusively borrowed `&mut TreeArenas` ever writes.
//!
//! The recording half then needs somewhere to put a size while layout holds
//! the arenas *shared*, which is what `pending` is: writes land there during
//! the pass and [`ContainerSizeTable::apply`] moves them across afterwards,
//! from [`Document::layout`](crate::Document::layout), which has the exclusive
//! borrow. A box whose recorded size is unchanged — every box on a page with
//! no query container at all, because "not a container" records nothing and
//! nothing is what the table already holds — never touches `pending`, so the
//! feature costs a page that does not use it one enum test per committing box.
//!
//! # What is recorded
//!
//! The **content box** in unrounded CSS px (`size - padding - border`), which
//! is what css-contain-3 means by the container's size: "the query container's
//! content box". Device rounding is a property of the frame, not of the
//! element, so it plays no part here.
//!
//! Per axis, only what the container actually supplies:
//! `container-type: size` records both, `inline-size` records the width alone.
//! The engine is horizontal-writing-mode only, so inline is width — and
//! Stylo's own `evaluate_potential_size_container` hard-codes `height: None`
//! for an inline-size container anyway, so recording the height would be
//! recording a number nothing can read, at the price of a recascade every time
//! the container's contents changed its height.
//!
//! # The recording moment
//!
//! One committing layout run of one box, beside the last remembered size and
//! for the same reason: it is where the numbers are produced. Gecko instead
//! re-resolves container query styles after layout in
//! `UpdateContainerQueryStyles`; the loop in
//! [`Document::layout`](crate::Document::layout) is this engine's version of
//! that, driven by the ids this module collects.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use hughie::compute::{used_border, used_padding};
use hughie::geometry::Size;
use hughie::style::{ContainerType, CoreStyle};
use hughie::tree::LayoutInput;

use super::style::StyleView;
use crate::tree::document::{NodeId, NodeSlot, TreeArenas};

/// How many times one [`Document::layout`](crate::Document::layout) call may
/// lay the document out.
///
/// A size query container's own size never answers to its contents —
/// css-contain-3 §2.1 contains the axes it supplies, so the contents cannot
/// feed back into them — which is what makes the loop converge rather than
/// oscillate. What it converges in is the depth of the deepest chain of
/// containers that all move at once, because a container's size can still
/// depend on its *ancestors'*: the outer one settles first, its descendants
/// re-resolve, and only then can an inner one's `cqw`-derived size be final.
/// Four covers every page anyone has, and the cap makes termination a
/// property of the code rather than of the content: past it the last layout
/// stands and the marks it left are resolved by the next flush, one commit
/// behind at worst.
pub(crate) const CONTAINER_PASSES: u32 = 4;

/// One size query container's content box, per physical axis, in unrounded
/// CSS px.
///
/// An axis reads `None` when the container does not supply it — an
/// `inline-size` container's block axis, and both axes of an element that is
/// not a size container at all. Stylo merges a `None` axis with the next
/// container up the chain and falls back to the small viewport when the walk
/// ends without one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ContainerSize {
    pub(crate) width: Option<f32>,
    pub(crate) height: Option<f32>,
}

impl ContainerSize {
    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.width.is_none() && self.height.is_none()
    }
}

/// The slot-keyed query-container-size table: a published half the style
/// traversal reads and a pending half layout writes.
#[derive(Debug, Default)]
pub(crate) struct ContainerSizeTable {
    /// What [`query_container_size`](stylo::dom::TElement::query_container_size)
    /// answers with. Written only through `&mut self`, so the parallel style
    /// traversal's reads are plain shared reads.
    ///
    /// Lazily sized: an absent entry reads as "not a container", so a page
    /// with no `container-type` allocates nothing here.
    entries: Vec<ContainerSize>,
    /// One record per committing run of a size query container in the layout
    /// pass in flight, waiting for [`Self::apply`].
    ///
    /// Bounded by how many times the pass committed a query container: an
    /// ordinary box records nothing over nothing and never reaches here, so
    /// this stays empty — and unborrowed — for every box of every page that
    /// has none.
    pending: RefCell<Vec<(NodeId, ContainerSize)>>,
    /// Whether this document has ever cascaded a style that resolved a
    /// container-relative unit (`ComputedValueFlags::USES_CONTAINER_UNITS`).
    ///
    /// Sticky, and the gate on the post-layout recascade loop: a page that
    /// has query containers but writes no `cqw`/`cqh` has nothing to
    /// re-resolve when one of them resizes. Set from the style traversal,
    /// which is why it is an atomic.
    uses_container_units: AtomicBool,
}

impl ContainerSizeTable {
    #[inline]
    pub(crate) fn get(&self, key: usize) -> ContainerSize {
        self.entries.get(key).copied().unwrap_or_default()
    }

    /// Records `size` for `id`, for [`Self::apply`] to publish.
    ///
    /// An empty `size` is "this box is not a size container", which is what
    /// every ordinary box records and what a container that turns `normal`
    /// records once.
    ///
    /// The one thing filtered out here is the box that records nothing and
    /// had nothing recorded — every box of every page with no query
    /// container, which is what keeps the feature free for them. Everything
    /// else is appended unconditionally, including a second record for a box
    /// that commits twice in a pass: keeping one record per element would
    /// mean scanning what is already staged on every container commit, which
    /// is quadratic in the number of containers, to save at most one extra
    /// recascade of one subtree. [`Self::apply`] replays the records in
    /// order, so the last one still decides what is published.
    pub(crate) fn note(&self, id: NodeId, size: ContainerSize) {
        if size.is_empty() && self.get(id.arena_key()).is_empty() {
            return;
        }
        self.pending.borrow_mut().push((id, size));
    }

    /// Publishes the pass's recordings and appends every container whose size
    /// moved to `resized`.
    ///
    /// Each record is compared against what is published at the moment it is
    /// replayed, so a box that committed twice in one pass and ended where it
    /// started lands in `resized` all the same — as may a box that committed
    /// twice, twice over. Neither is a correctness question: what is
    /// published is the last record, and `resized` only decides *whose*
    /// descendants are marked for recascade — a mark
    /// `Document::mark_descendants_recascade` is idempotent in.
    pub(crate) fn apply(&mut self, resized: &mut Vec<NodeId>) {
        // Taken out and handed back so the buffer keeps its capacity: a page
        // whose container animates records into the same allocation forever.
        let mut pending = std::mem::take(self.pending.get_mut());
        for (id, size) in pending.drain(..) {
            let key = id.arena_key();
            if self.entries.len() <= key {
                if size.is_empty() {
                    continue;
                }
                self.entries.resize(key + 1, ContainerSize::default());
            }
            if self.entries[key] != size {
                self.entries[key] = size;
                resized.push(id);
            }
        }
        *self.pending.get_mut() = pending;
    }

    /// Resets a freed slot, so the key's next occupant is no container.
    pub(crate) fn reset(&mut self, key: usize) {
        if let Some(entry) = self.entries.get_mut(key) {
            *entry = ContainerSize::default();
        }
    }

    #[inline]
    pub(crate) fn units_flag(&self) -> &AtomicBool {
        &self.uses_container_units
    }

    #[inline]
    pub(crate) fn uses_container_units(&self) -> bool {
        self.uses_container_units.load(Ordering::Relaxed)
    }
}

/// The recording moment: one committing layout run of one box.
///
/// `size` is that run's own border-box output, unrounded, exactly as
/// [`crate::layout::remembered::record`] receives it.
pub(crate) fn record<T>(
    tree: &TreeArenas<T>,
    node: NodeSlot,
    view: &StyleView<'_, T>,
    input: LayoutInput,
    size: Size<f32>,
) {
    let container_type = view.container_type();
    let id = tree.at(node).id();
    if !container_type.is_size_container_type() {
        tree.note_container_size(id, ContainerSize::default());
        return;
    }
    let padding = used_padding(view, input.parent_size.width);
    let border = used_border(view);
    let width = (size.width - padding.horizontal_sum() - border.horizontal_sum()).max(0.0);
    let height = (size.height - padding.vertical_sum() - border.vertical_sum()).max(0.0);
    tree.note_container_size(
        id,
        ContainerSize {
            width: Some(width),
            height: container_type
                .intersects(ContainerType::SIZE)
                .then_some(height),
        },
    );
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{ContainerSize, ContainerSizeTable};
    use crate::tree::document::NodeId;

    fn id(key: u64) -> NodeId {
        NodeId::from_bits(key).expect("a bare key is a well-shaped handle")
    }

    fn size(width: f32, height: Option<f32>) -> ContainerSize {
        ContainerSize {
            width: Some(width),
            height,
        }
    }

    #[test]
    fn a_page_with_no_query_container_never_touches_the_table() {
        let mut table = ContainerSizeTable::default();
        for key in 1..8 {
            table.note(id(key), ContainerSize::default());
        }
        let mut resized = Vec::new();
        table.apply(&mut resized);
        assert!(resized.is_empty());
        assert!(table.get(4).is_empty());
        assert!(!table.uses_container_units());
    }

    #[test]
    fn a_size_is_published_once_and_reports_every_move() {
        let mut table = ContainerSizeTable::default();
        let id = id(3);
        let mut resized = Vec::new();

        table.note(id, size(200.0, Some(100.0)));
        table.apply(&mut resized);
        assert_eq!(resized, vec![id]);
        assert_eq!(table.get(3), size(200.0, Some(100.0)));

        // Re-recording the published size is not a move.
        resized.clear();
        table.note(id, size(200.0, Some(100.0)));
        table.apply(&mut resized);
        assert!(resized.is_empty());

        // Two commits in one pass publish the last of them. Ending where
        // they started still reports a move — once per record replayed —
        // which costs one recascade of a subtree that did not need it and
        // nothing else.
        table.note(id, size(50.0, Some(100.0)));
        table.note(id, size(200.0, Some(100.0)));
        table.apply(&mut resized);
        assert_eq!(table.get(3), size(200.0, Some(100.0)));
        assert_eq!(resized, vec![id, id]);
        resized.clear();

        // Losing `container-type` is a move, and so is a freed slot.
        table.note(id, ContainerSize::default());
        table.apply(&mut resized);
        assert_eq!(resized, vec![id]);
        assert!(table.get(3).is_empty());

        table.note(id, size(10.0, None));
        table.apply(&mut resized);
        table.reset(3);
        assert!(table.get(3).is_empty());
        table.reset(99);
    }
}
