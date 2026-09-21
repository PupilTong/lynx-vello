//! The **last committed content box**: the content box each element had at
//! the end of the last layout that committed it, and the two CSS features
//! that read it back.
//!
//! Both features ask the same question of layout — what was this box's inner
//! size the last time it was laid out for real — and both are answered from
//! one slot-keyed table, recorded once per committing run:
//!
//! * the **last remembered size** ([css-sizing-4 §5.2.1](https://drafts.csswg.org/css-sizing-4/#last-remembered)),
//!   what an `auto` `contain-intrinsic-size` answers with once the box starts skipping its
//!   contents;
//! * the **size query container** ([css-contain-3 §2.1](https://drafts.csswg.org/css-contain-3/#container-type)),
//!   the box `cqw` and `cqh` resolve against.
//!
//! They differ only in what they keep, which is why [`CommittedBox`] has two
//! fields and not one: the remembered half freezes while a box is
//! size-contained and is removed when the `auto` keyword goes away, while the
//! container half is simply whatever the last commit produced, per axis the
//! container's `container-type` supplies.
//!
//! # The two readers
//!
//! The remembered half is read through [`contain_intrinsic_width`] /
//! [`contain_intrinsic_height`], which a [`StyleView`] hands to `hughie` in
//! place of the computed value. `hughie` needs none of this vocabulary: its
//! `contain_intrinsic_length` already reads `AutoLength(l)` as `l` and
//! `AutoNone` as no explicit size, so answering `Length(remembered)` is the
//! whole substitution. The container half is read by Stylo, whose
//! `container_relative_to_computed_value` asks
//! `Context::get_container_size_query()`, which walks up from the element's
//! *parent* and, at the first ancestor whose `container-type` is a size
//! container type, calls
//! [`TElement::query_container_size`](stylo::dom::TElement::query_container_size).
//!
//! Both readers reach the table through `&Node`, which is all a
//! [`TElement`](stylo::dom::TElement) and all a [`StyleView`] are — the
//! structural reason this table lives on
//! [`TreeArenas`](crate::tree::TreeArenas) beside the `content-visibility:
//! auto` relevance table, rather than in
//! [`LayoutSlot`](hughie::tree::LayoutSlot) or `NodeLayoutState`, neither of
//! which a style view can reach.
//!
//! # What is recorded, and when
//!
//! The **content box** in unrounded CSS px (`size - padding - border`): "the
//! current inner dimensions of its principal box" in one spec and "the query
//! container's content box" in the other are the same box. Device rounding
//! happens afterwards, in the rounding tail, and is a property of the frame
//! rather than of the element.
//!
//! **This engine has no `ResizeObserver`, so the recording moment is the
//! commit's own layout run** — [`record`], called from the layout host after
//! an algorithm's committing run, on a cache miss. That is the same box and
//! the same numbers a `ResizeObserver` would have been handed, one step
//! earlier than a browser delivers them: a browser observes after the layout
//! that produced the size, so a same-frame change that starts the box skipping
//! still uses the previous frame's value, while here the commit that laid the
//! box out is the one that remembers it. Gecko likewise re-resolves container
//! query styles after layout, in `UpdateContainerQueryStyles`; the loop in
//! [`Document::layout`](crate::Document::layout) is this engine's version of
//! that, driven by the ids [`CommittedBoxTable::apply`] collects.
//!
//! # Publication, and why staging both halves is sound
//!
//! The published half is a plain `Vec` that only an exclusively borrowed
//! `&mut TreeArenas` ever writes, because the container half is read **from
//! the style traversal**, which runs on the style pool's worker threads: a
//! `RefCell` read concurrently from several of them races on its borrow flag.
//! Recording then needs somewhere to put a box while layout holds the arenas
//! *shared*, which is what `pending` is — [`CommittedBoxTable::apply`] moves
//! the pass's records across afterwards, from
//! [`Document::layout`](crate::Document::layout), which has the exclusive
//! borrow, once per layout pass.
//!
//! The remembered half is read only from layout, which is single-threaded, so
//! it could have been published at record time. It is not, and need not be,
//! because **no read in a pass can observe a record made earlier in that same
//! pass**:
//!
//! * [`axis_value`] looks the table up only for a box that `has_auto` *and* is currently skipping
//!   its contents. A skipping box has `Contain::SIZE` — which is `INLINE_SIZE | BLOCK_SIZE` — so
//!   [`record`]'s frozen branch is the one it takes, and a frozen box's record is what is already
//!   published. The one write a skipping box can make is the removal, and a box with no `auto`
//!   keyword left never reaches the lookup.
//! * The lookup [`record`] itself does, for a frozen or partly contained axis, carries the
//!   published value forward. Containment, `container-type` and skipping are all pure functions of
//!   computed style and of the relevance table, none of which move during a layout pass, so every
//!   commit of one box in one pass takes the same branch and reads the same published value; the
//!   last record replayed still decides what is published, exactly as it would have with an
//!   immediate write.
//!
//! Across passes there is nothing to prove: `apply` runs after every
//! `run_layout`, so pass *n + 1* reads what pass *n* recorded.
//!
//! # csswg-drafts#8407
//!
//! [An element with `content-visibility: auto`](https://github.com/w3c/csswg-drafts/issues/8407)
//! behaves as if its `contain-intrinsic-*` values carried `auto`. The fork
//! owns the mapping (`ContainIntrinsicSize::add_auto_if_needed`) but applies
//! it in `StyleAdjuster::adjust_for_contain_intrinsic_size`, which is
//! `#[cfg(feature = "gecko")]` and so never runs in this build — the fold is
//! therefore this module's, on the computed value, on the way into layout.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use hughie::compute::{used_border, used_padding};
use hughie::geometry::Size;
use hughie::style::{Contain, ContainIntrinsicSize, ContainerType, ContentVisibility, CoreStyle};
use hughie::tree::LayoutInput;
use stylo::properties::ComputedValues;
use stylo::values::computed::Length;
use stylo::values::generics::NonNegative;

use super::style::{StyleView, skips_contents};
use crate::tree::document::{NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

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

/// One element's content box, per physical axis, in unrounded CSS px, each
/// axis reading `None` when it carries nothing — which the two halves of
/// [`CommittedBox`] mean their own kind of nothing by.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ContentBox {
    pub(crate) width: Option<f32>,
    pub(crate) height: Option<f32>,
}

impl ContentBox {
    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.width.is_none() && self.height.is_none()
    }
}

/// The css-sizing-4 §5.2.1 last remembered size.
///
/// An axis reads `None` when nothing is remembered for it — the state the
/// spec's "remove its last remembered size" leaves behind, and the state
/// every element starts in.
pub(crate) type RememberedSize = ContentBox;

/// The css-contain-3 §2.1 size a query container supplies: `container-type:
/// size` records both axes, `inline-size` the width alone (this engine is
/// horizontal-writing-mode only, so inline is width, and Stylo's own
/// `evaluate_potential_size_container` hard-codes `height: None` for an
/// inline-size container anyway — recording it would cost a recascade every
/// time the contents changed a height nothing can read).
///
/// An axis reads `None` when the container does not supply it, and both do
/// for an element that is no size container at all. Stylo merges a `None`
/// axis with the next container up the chain and falls back to the small
/// viewport when the walk ends without one.
pub(crate) type ContainerSize = ContentBox;

/// What one element's last committing layout run left behind.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct CommittedBox {
    pub(crate) remembered: RememberedSize,
    pub(crate) container: ContainerSize,
}

impl CommittedBox {
    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.remembered.is_empty() && self.container.is_empty()
    }
}

/// The slot-keyed table: a published half both readers see and a pending half
/// the layout pass writes.
#[derive(Debug, Default)]
pub(crate) struct CommittedBoxTable {
    /// What [`query_container_size`](stylo::dom::TElement::query_container_size)
    /// and the `contain-intrinsic-*` substitution answer from. Written only
    /// through `&mut self`, so the parallel style traversal's reads are plain
    /// shared reads.
    ///
    /// Lazily sized: an absent entry reads as "remembers nothing, contains
    /// nothing", so a page with neither feature allocates nothing here.
    entries: Vec<CommittedBox>,
    /// One record per committing run of the layout pass in flight, waiting
    /// for [`Self::apply`].
    ///
    /// Bounded by how many times the pass committed a box that either
    /// remembers a size or is a query container: an ordinary box records
    /// nothing over nothing and never reaches here, so this stays empty — and
    /// unborrowed — for every box of every page that uses neither feature.
    pending: RefCell<Vec<(NodeId, CommittedBox)>>,
    /// Whether this document has ever cascaded a style that resolved a
    /// container-relative unit (`ComputedValueFlags::USES_CONTAINER_UNITS`).
    ///
    /// Sticky, and the gate on the post-layout recascade loop: a page that
    /// has query containers but writes no `cqw`/`cqh` has nothing to
    /// re-resolve when one of them resizes. Set from the style traversal,
    /// which is why it is an atomic.
    uses_container_units: AtomicBool,
}

impl CommittedBoxTable {
    #[inline]
    pub(crate) fn get(&self, key: usize) -> CommittedBox {
        self.entries.get(key).copied().unwrap_or_default()
    }

    /// Records `committed` for `id`, for [`Self::apply`] to publish.
    ///
    /// The one thing filtered out is the box that records nothing and had
    /// nothing recorded — every box of every page that uses neither feature,
    /// which is what keeps both free for them. Everything else is appended
    /// unconditionally, including a second record for a box that commits
    /// twice in a pass: keeping one record per element would mean scanning
    /// what is already staged on every commit, which is quadratic in the
    /// number of recording boxes, to save at most one extra recascade of one
    /// subtree. [`Self::apply`] replays the records in order, so the last one
    /// still decides what is published.
    pub(crate) fn note(&self, id: NodeId, committed: CommittedBox) {
        if committed.is_empty() && self.get(id.arena_key()).is_empty() {
            return;
        }
        self.pending.borrow_mut().push((id, committed));
    }

    /// Publishes the pass's recordings and appends every *query container*
    /// whose size moved to `resized`.
    ///
    /// Each record is compared against what is published at the moment it is
    /// replayed, so a box that committed twice in one pass and ended where it
    /// started lands in `resized` all the same. That is not a correctness
    /// question: what is published is the last record, and `resized` only
    /// decides *whose* descendants are marked for recascade — a mark
    /// `Document::mark_descendants_recascade` is idempotent in. The
    /// remembered half needs no such report: nothing but layout reads it.
    pub(crate) fn apply(&mut self, resized: &mut Vec<NodeId>) {
        // Taken out and handed back so the buffer keeps its capacity: a page
        // whose container animates records into the same allocation forever.
        let mut pending = std::mem::take(self.pending.get_mut());
        for (id, committed) in pending.drain(..) {
            let key = id.arena_key();
            if self.entries.len() <= key {
                if committed.is_empty() {
                    continue;
                }
                self.entries.resize(key + 1, CommittedBox::default());
            }
            let entry = &mut self.entries[key];
            if entry.container != committed.container {
                entry.container = committed.container;
                resized.push(id);
            }
            entry.remembered = committed.remembered;
        }
        *self.pending.get_mut() = pending;
    }

    /// Resets a freed slot, so the key's next occupant remembers nothing and
    /// is no container.
    pub(crate) fn reset(&mut self, key: usize) {
        if let Some(entry) = self.entries.get_mut(key) {
            *entry = CommittedBox::default();
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

/// csswg-drafts#8407: an element with `content-visibility: auto` behaves as if
/// its `contain-intrinsic-*` values carried the `auto` keyword.
fn effective(
    value: ContainIntrinsicSize,
    content_visibility: ContentVisibility,
) -> ContainIntrinsicSize {
    if content_visibility == ContentVisibility::Auto {
        value.add_auto_if_needed().unwrap_or(value)
    } else {
        value
    }
}

/// Whether a computed value carries the `auto` keyword.
const fn has_auto(value: &ContainIntrinsicSize) -> bool {
    matches!(
        value,
        ContainIntrinsicSize::AutoNone | ContainIntrinsicSize::AutoLength(..)
    )
}

/// Whether the *effective* value carries it — the same question [`effective`]
/// answers, without building the value, since #8407 adds the keyword to every
/// value an `auto` element has.
///
/// This is what the recording side asks, once per axis per committing run of
/// every box on the page, so it reads the two computed values in place rather
/// than cloning them.
const fn has_effective_auto(
    value: &ContainIntrinsicSize,
    content_visibility: ContentVisibility,
) -> bool {
    matches!(content_visibility, ContentVisibility::Auto) || has_auto(value)
}

/// The effective `contain-intrinsic-*` value for one axis, with #8407 applied
/// and the last remembered size substituted where the spec calls for it.
fn axis_value<T>(
    node: &Node<T>,
    style: &ComputedValues,
    computed: ContainIntrinsicSize,
    axis: impl FnOnce(RememberedSize) -> Option<f32>,
) -> ContainIntrinsicSize {
    let value = effective(computed, style.clone_content_visibility());
    // "and is currently skipping its contents": a `contain: size` box that is
    // *not* skipping keeps its `<length>` (or `none`) however recently it was
    // laid out with real contents.
    if !has_auto(&value) || !skips_contents(node, style) {
        return value;
    }
    match axis(node.arenas().committed_box(node.id()).remembered) {
        Some(px) => ContainIntrinsicSize::Length(NonNegative(Length::new(px))),
        None => value,
    }
}

pub(crate) fn contain_intrinsic_width<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> ContainIntrinsicSize {
    axis_value(node, style, style.clone_contain_intrinsic_width(), |size| {
        size.width
    })
}

pub(crate) fn contain_intrinsic_height<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> ContainIntrinsicSize {
    axis_value(
        node,
        style,
        style.clone_contain_intrinsic_height(),
        |size| size.height,
    )
}

/// The recording moment: one committing layout run of one box, filling both
/// halves of its entry from the one content box that run produced.
///
/// `size` is that run's own border-box output, unrounded. The host calls this
/// only for a run that commits — a measurement establishes no rendered size —
/// and only on a cache **miss**, because a box served from the cache produced
/// the same size it already recorded under the same input.
pub(crate) fn record<T>(
    tree: &TreeArenas<T>,
    node: NodeSlot,
    view: &StyleView<'_, T>,
    input: LayoutInput,
    size: Size<f32>,
) {
    let id = tree.at(node).id();
    let style = view.values();
    let container_type = view.container_type();
    let is_container = container_type.is_size_container_type();
    let content_visibility = style.clone_content_visibility();
    let position = style.get_position();
    let auto = Size::new(
        has_effective_auto(&position.contain_intrinsic_width, content_visibility),
        has_effective_auto(&position.contain_intrinsic_height, content_visibility),
    );

    if !is_container && !auto.width && !auto.height {
        // The empty record, which both halves mean something by: "if an
        // element has a last remembered size but does not have auto keyword
        // in contain-intrinsic-size property, remove its last remembered
        // size", and "this box is not a size query container". Free for the
        // overwhelming majority of elements, which never reached the table at
        // all.
        tree.note_committed_box(id, CommittedBox::default());
        return;
    }

    // "but does not have size containment". A size-contained box was laid out
    // as if it had no contents, so this run's inner dimensions are the
    // substituted estimate rather than anything its contents produced —
    // recording it would overwrite the measurement with the guess. Every
    // *skipping* box is in here too (skipping turns `SIZE` on), which is
    // exactly what lets a remembered size survive for as long as the box goes
    // on skipping. The spec's sentence names whole-box size containment, but
    // containment is per axis and so is this: under `contain: inline-size`
    // (and under `container-type: inline-size`, which implies it) the height
    // this run produced *is* the contents' own, and only the width is the
    // estimate. An axis records what its contents made, or keeps what it last
    // remembered.
    let containment = view.containment();
    let contained = Size::new(
        containment.contains(Contain::INLINE_SIZE),
        containment.contains(Contain::BLOCK_SIZE),
    );
    let published = tree.committed_box(id);
    if contained.width && contained.height && !is_container && published.container.is_empty() {
        // Every axis is frozen and neither this box nor what is published for
        // it is a query container, so the record would be the published entry
        // itself: the skipping box's pass, costing one enum test and one
        // lookup. Sound to skip rather than stage because every branch this
        // run took is a pass-constant, so a box that commits twice in a pass
        // reaches here on both of them or on neither.
        return;
    }

    // "the current inner dimensions of its principal box" and "the query
    // container's content box" are the same box, resolved the way the
    // algorithms themselves resolve it — percentages against the containing
    // block's inline size, `none`/`hidden` border sides reading zero.
    let padding = used_padding(view, input.parent_size.width);
    let border = used_border(view);
    let inner = Size::new(
        (size.width - padding.horizontal_sum() - border.horizontal_sum()).max(0.0),
        (size.height - padding.vertical_sum() - border.vertical_sum()).max(0.0),
    );

    // A contained axis keeps what it last remembered; an axis without the
    // keyword remembers nothing; the rest record what they just measured.
    let axis = |auto: bool, contained: bool, published: Option<f32>, inner: f32| {
        if !auto {
            return None;
        }
        if contained { published } else { Some(inner) }
    };
    let remembered = if !auto.width && !auto.height {
        RememberedSize::default()
    } else if contained.width && contained.height {
        published.remembered
    } else {
        RememberedSize {
            width: axis(
                auto.width,
                contained.width,
                published.remembered.width,
                inner.width,
            ),
            height: axis(
                auto.height,
                contained.height,
                published.remembered.height,
                inner.height,
            ),
        }
    };
    tree.note_committed_box(
        id,
        CommittedBox {
            remembered,
            container: if is_container {
                ContainerSize {
                    width: Some(inner.width),
                    height: container_type
                        .intersects(ContainerType::SIZE)
                        .then_some(inner.height),
                }
            } else {
                ContainerSize::default()
            },
        },
    );
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use stylo::values::computed::Length;
    use stylo::values::generics::NonNegative;

    use super::{
        CommittedBox, CommittedBoxTable, ContainIntrinsicSize, ContainerSize, ContentVisibility,
        RememberedSize, effective, has_auto, has_effective_auto,
    };
    use crate::tree::document::NodeId;

    fn id(key: u64) -> NodeId {
        NodeId::from_bits(key).expect("a bare key is a well-shaped handle")
    }

    fn length(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::Length(NonNegative(Length::new(value)))
    }

    fn auto_length(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::AutoLength(NonNegative(Length::new(value)))
    }

    fn container(width: f32, height: Option<f32>) -> CommittedBox {
        CommittedBox {
            remembered: RememberedSize::default(),
            container: ContainerSize {
                width: Some(width),
                height,
            },
        }
    }

    fn remembered(width: Option<f32>, height: Option<f32>) -> CommittedBox {
        CommittedBox {
            remembered: RememberedSize { width, height },
            container: ContainerSize::default(),
        }
    }

    /// Published only by `apply`, and then whole.
    fn publish(table: &mut CommittedBoxTable) -> Vec<NodeId> {
        let mut resized = Vec::new();
        table.apply(&mut resized);
        resized
    }

    #[test]
    fn a_page_that_uses_neither_feature_never_touches_the_table() {
        let mut table = CommittedBoxTable::default();
        for key in 1..8 {
            table.note(id(key), CommittedBox::default());
        }
        assert!(publish(&mut table).is_empty());
        assert!(table.get(4).is_empty());
        assert!(!table.uses_container_units());
    }

    #[test]
    fn a_container_size_is_published_once_and_reports_every_move() {
        let mut table = CommittedBoxTable::default();
        let id = id(3);

        table.note(id, container(200.0, Some(100.0)));
        assert_eq!(publish(&mut table), vec![id]);
        assert_eq!(table.get(3), container(200.0, Some(100.0)));

        // Re-recording the published size is not a move.
        table.note(id, container(200.0, Some(100.0)));
        assert!(publish(&mut table).is_empty());

        // Two commits in one pass publish the last of them. Ending where
        // they started still reports a move — once per record replayed —
        // which costs one recascade of a subtree that did not need it and
        // nothing else.
        table.note(id, container(50.0, Some(100.0)));
        table.note(id, container(200.0, Some(100.0)));
        assert_eq!(publish(&mut table), vec![id, id]);
        assert_eq!(table.get(3), container(200.0, Some(100.0)));

        // Losing `container-type` is a move, and so is a freed slot.
        table.note(id, CommittedBox::default());
        assert_eq!(publish(&mut table), vec![id]);
        assert!(table.get(3).is_empty());

        table.note(id, container(10.0, None));
        publish(&mut table);
        table.reset(3);
        assert!(table.get(3).is_empty());
        table.reset(99);
    }

    #[test]
    fn an_element_no_run_has_recorded_remembers_nothing() {
        let mut table = CommittedBoxTable::default();
        assert_eq!(table.get(9), CommittedBox::default());
        assert!(table.get(9).is_empty());

        // Recording nothing for an unreached key leaves the table empty, which
        // is what a page with no `auto` keyword does on every commit.
        table.note(id(9), CommittedBox::default());
        assert!(publish(&mut table).is_empty());
        assert_eq!(table.get(9), CommittedBox::default());
    }

    #[test]
    fn remembering_is_per_axis_and_removal_is_recording_an_empty_one() {
        let mut table = CommittedBoxTable::default();
        table.note(id(2), remembered(None, Some(40.0)));
        assert_eq!(
            publish(&mut table),
            Vec::new(),
            "a remembered size moving is no container resize",
        );
        assert_eq!(table.get(2).remembered.height, Some(40.0));
        assert_eq!(
            table.get(2).remembered.width,
            None,
            "an axis without auto keeps none"
        );
        assert_eq!(
            table.get(1),
            CommittedBox::default(),
            "and its neighbours are untouched"
        );

        table.note(id(2), remembered(Some(10.0), Some(50.0)));
        publish(&mut table);
        assert_eq!(table.get(2).remembered.width, Some(10.0));

        table.note(id(2), CommittedBox::default());
        publish(&mut table);
        assert!(table.get(2).is_empty(), "losing `auto` removes it");

        table.note(id(2), remembered(Some(3.0), None));
        publish(&mut table);
        table.reset(2);
        assert!(table.get(2).is_empty(), "and a freed slot starts clean");
        table.reset(77);
    }

    /// The two halves are recorded together and published together, and
    /// neither overwrites the other.
    #[test]
    fn a_record_is_staged_until_apply_and_then_carries_both_halves() {
        let mut table = CommittedBoxTable::default();
        let id = id(5);
        let both = CommittedBox {
            remembered: RememberedSize {
                width: Some(30.0),
                height: Some(40.0),
            },
            container: ContainerSize {
                width: Some(30.0),
                height: None,
            },
        };

        table.note(id, both);
        assert_eq!(
            table.get(5),
            CommittedBox::default(),
            "a staged record is invisible to both readers until it is published",
        );
        assert_eq!(publish(&mut table), vec![id]);
        assert_eq!(table.get(5), both);

        // Publishing one half never clobbers the other: a box that stops
        // being a query container goes on remembering what it rendered at.
        let remembered_only = CommittedBox {
            remembered: both.remembered,
            container: ContainerSize::default(),
        };
        table.note(id, remembered_only);
        assert_eq!(
            publish(&mut table),
            vec![id],
            "and dropping the container half is a move",
        );
        assert_eq!(table.get(5), remembered_only);
    }

    #[test]
    fn content_visibility_auto_adds_the_auto_keyword() {
        assert_eq!(
            effective(length(10.0), ContentVisibility::Auto),
            auto_length(10.0),
            "csswg-drafts#8407",
        );
        assert_eq!(
            effective(ContainIntrinsicSize::None, ContentVisibility::Auto),
            ContainIntrinsicSize::AutoNone,
        );
        assert_eq!(
            effective(auto_length(10.0), ContentVisibility::Auto),
            auto_length(10.0),
            "already auto, unchanged",
        );
        for content_visibility in [ContentVisibility::Visible, ContentVisibility::Hidden] {
            assert_eq!(
                effective(length(10.0), content_visibility),
                length(10.0),
                "#8407 is about `auto` alone",
            );
        }

        assert!(has_auto(&auto_length(1.0)));
        assert!(has_auto(&ContainIntrinsicSize::AutoNone));
        assert!(!has_auto(&length(1.0)));
        assert!(!has_auto(&ContainIntrinsicSize::None));
    }

    #[test]
    fn the_recording_side_asks_the_same_question_without_building_the_value() {
        for value in [
            ContainIntrinsicSize::None,
            ContainIntrinsicSize::AutoNone,
            length(10.0),
            auto_length(10.0),
        ] {
            for content_visibility in [
                ContentVisibility::Visible,
                ContentVisibility::Auto,
                ContentVisibility::Hidden,
            ] {
                assert_eq!(
                    has_effective_auto(&value, content_visibility),
                    has_auto(&effective(value.clone(), content_visibility)),
                    "{value:?} under {content_visibility:?}",
                );
            }
        }
    }
}
