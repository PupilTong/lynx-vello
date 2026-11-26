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
//! [`contain_intrinsic_height`], which a [`StyleView`] hands to `seldon` in
//! place of the computed value. `seldon` needs none of this vocabulary: its
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
//! [`LayoutSlot`](seldon::tree::LayoutSlot) or `NodeLayoutState`, neither of
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
//! **The recording moment is the commit's own layout run** — [`record`],
//! called from the layout host after an algorithm's committing run, on a
//! cache miss. That is the same box and the same numbers a resize
//! observation reads ([`crate::visual::resize`] gathers after that layout and
//! delivers from a posted entry after the frame), one step earlier than a
//! browser delivers them: a browser observes after the layout
//! that produced the size, so a same-frame change that starts the box skipping
//! still uses the previous frame's value, while here the commit that laid the
//! box out is the one that remembers it. Gecko likewise re-resolves container
//! query styles after layout, in `UpdateContainerQueryStyles`; the loop in
//! [`Document::layout`](crate::tree::document::Document::layout) is this engine's version of
//! that, driven by the ids [`CommittedBoxTable::apply`] collects.
//!
//! # The interleave, and why the container half rarely waits for that loop
//!
//! A `container-type: size` box is the one recording that does not have to
//! wait for its own contents, because css-contain-3 §2.1 gives it `contain:
//! size`: *both* of its axes are laid out as if it had no contents, so its
//! content box is a function of its own style and its layout input alone.
//! [`container_estimate`] is that function, and the layout host calls it at
//! the **entry** of such a box's committing run, before one child style has
//! been read. When it disagrees with what is published, the run records the
//! new size and lays no contents out
//! ([`DeferredContainer`](crate::tree::document::DeferredContainer)):
//! `layout::host::run_layout` publishes it, restyles the `cqw`/`cqh` readers
//! under the box, and relays the subtree once, at the size it will keep. What
//! the post-layout loop existed to fix — a whole document laid out at a
//! container size that was about to change — therefore does not happen for
//! that shape at all.
//!
//! The loop is still there, and still owns everything the estimate cannot
//! predict: a `container-type: inline-size` box, whose block axis really is
//! its contents' (there is nothing to publish before them), and a Lynx text
//! element, whose paragraph algorithm implements no size containment.
//! A prediction that disagreed with its algorithm would land there too, which
//! is what makes it a `debug_assert` rather than a correctness question — see
//! [`debug_assert_estimate_agrees`].
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
//! [`Document::layout`](crate::tree::document::Document::layout), which has the exclusive
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
//! The interleave adds one publication *inside* a run, which neither argument
//! covers and neither needs to: the records it publishes are its own deferrals
//! — whose remembered half is carried across unchanged, by
//! [`note_container_estimate`] — plus whatever the boxes laid out before them
//! staged. Both of those are records a later read in the same run could
//! already have observed under the rules above, and both of the reads above
//! are reads a *skipping* box makes of a frozen record, which no branch in a
//! run can change.
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

use rustc_hash::FxHashMap;
use seldon::compute::{compute_skipped_contents_size, used_border, used_padding};
use seldon::geometry::Size;
use seldon::style::{Contain, ContainIntrinsicSize, ContainerType, ContentVisibility, CoreStyle};
use seldon::tree::{LayoutInput, LayoutOutput};
use stylo::properties::ComputedValues;
use stylo::values::computed::Length;
use stylo::values::generics::NonNegative;

use super::style::{StyleView, skips_contents};
use crate::tree::document::{NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

pub(crate) const CONTAINER_PASSES: u32 = 4;

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

pub(crate) type RememberedSize = ContentBox;

pub(crate) type ContainerSize = ContentBox;

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

#[derive(Debug, Default)]
pub(crate) struct CommittedBoxTable {
    entries: FxHashMap<usize, CommittedBox>,
    pending: RefCell<Vec<(NodeId, CommittedBox)>>,
    uses_container_units: AtomicBool,
}

impl CommittedBoxTable {
    #[inline]
    pub(crate) fn get(&self, key: usize) -> CommittedBox {
        self.entries.get(&key).copied().unwrap_or_default()
    }

    pub(crate) fn note(&self, id: NodeId, committed: CommittedBox) {
        if committed.is_empty() && self.get(id.arena_key()).is_empty() {
            return;
        }
        self.pending.borrow_mut().push((id, committed));
    }

    pub(crate) fn apply(&mut self, resized: &mut Vec<NodeId>) {
        let mut pending = std::mem::take(self.pending.get_mut());
        for (id, committed) in pending.drain(..) {
            let key = id.arena_key();
            if committed.is_empty() {
                if self
                    .entries
                    .remove(&key)
                    .is_some_and(|old| !old.container.is_empty())
                {
                    resized.push(id);
                }
                continue;
            }
            let entry = self.entries.entry(key).or_default();
            if entry.container != committed.container {
                entry.container = committed.container;
                resized.push(id);
            }
            entry.remembered = committed.remembered;
        }
        *self.pending.get_mut() = pending;
    }

    pub(crate) fn reset(&mut self, key: usize) {
        if !self.entries.is_empty() {
            self.entries.remove(&key);
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

const fn has_auto(value: &ContainIntrinsicSize) -> bool {
    matches!(
        value,
        ContainIntrinsicSize::AutoNone | ContainIntrinsicSize::AutoLength(..)
    )
}

const fn has_effective_auto(
    value: &ContainIntrinsicSize,
    content_visibility: ContentVisibility,
) -> bool {
    matches!(content_visibility, ContentVisibility::Auto) || has_auto(value)
}

fn axis_value<T>(
    node: &Node<T>,
    style: &ComputedValues,
    computed: ContainIntrinsicSize,
    axis: impl FnOnce(RememberedSize) -> Option<f32>,
) -> ContainIntrinsicSize {
    let value = effective(computed, *style.get_content_visibility());
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
    axis_value(
        node,
        style,
        style.slow_clone_contain_intrinsic_width(),
        |size| size.width,
    )
}

pub(crate) fn contain_intrinsic_height<T>(
    node: &Node<T>,
    style: &ComputedValues,
) -> ContainIntrinsicSize {
    axis_value(
        node,
        style,
        style.slow_clone_contain_intrinsic_height(),
        |size| size.height,
    )
}

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
    let content_visibility = *style.get_content_visibility();
    let position = style.get_position();
    let auto = Size::new(
        has_effective_auto(&position.contain_intrinsic_width, content_visibility),
        has_effective_auto(&position.contain_intrinsic_height, content_visibility),
    );

    if !is_container && !auto.width && !auto.height {
        tree.note_committed_box(id, CommittedBox::default());
        return;
    }

    let containment = view.containment();
    let contained = Size::new(
        containment.contains(Contain::INLINE_SIZE),
        containment.contains(Contain::BLOCK_SIZE),
    );
    let published = tree.committed_box(id);
    if contained.width && contained.height && !is_container && published.container.is_empty() {
        return;
    }

    let inner = content_box(view, input, size);
    debug_assert_estimate_agrees(view, input, is_container, inner);

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
                container_size(container_type, inner)
            } else {
                ContainerSize::default()
            },
        },
    );
}

fn content_box<T>(view: &StyleView<'_, T>, input: LayoutInput, size: Size<f32>) -> Size<f32> {
    let padding = used_padding(view, input.parent_size.width);
    let border = used_border(view);
    Size::new(
        (size.width - padding.horizontal_sum() - border.horizontal_sum()).max(0.0),
        (size.height - padding.vertical_sum() - border.vertical_sum()).max(0.0),
    )
}

fn container_size(container_type: ContainerType, inner: Size<f32>) -> ContainerSize {
    ContainerSize {
        width: Some(inner.width),
        height: container_type
            .intersects(ContainerType::SIZE)
            .then_some(inner.height),
    }
}

pub(crate) struct ContainerEstimate {
    pub(crate) output: LayoutOutput,
    pub(crate) container: ContainerSize,
}

pub(crate) fn container_estimate<T>(
    view: &StyleView<'_, T>,
    input: LayoutInput,
) -> Option<ContainerEstimate> {
    let container_type = view.container_type();
    if !container_type.intersects(ContainerType::SIZE)
        || !view.containment().contains(Contain::SIZE)
    {
        return None;
    }
    let output = compute_skipped_contents_size(view, input);
    Some(ContainerEstimate {
        container: container_size(container_type, content_box(view, input, output.size)),
        output,
    })
}

pub(crate) fn note_container_estimate<T>(
    tree: &TreeArenas<T>,
    id: NodeId,
    container: ContainerSize,
) {
    tree.note_committed_box(
        id,
        CommittedBox {
            remembered: tree.committed_box(id).remembered,
            container,
        },
    );
}

fn debug_assert_estimate_agrees<T>(
    view: &StyleView<'_, T>,
    input: LayoutInput,
    is_container: bool,
    inner: Size<f32>,
) {
    if !cfg!(debug_assertions)
        || !is_container
        || crate::layout::paragraph::establishes_paragraph(view.display_mode())
    {
        return;
    }
    let Some(estimate) = container_estimate(view, input) else {
        return;
    };
    let recorded = container_size(view.container_type(), inner);
    let agrees = |estimated: Option<f32>, recorded: Option<f32>| match (estimated, recorded) {
        (Some(estimated), Some(recorded)) => (estimated - recorded).abs() < 1e-3,
        (left, right) => left == right,
    };
    debug_assert!(
        agrees(estimate.container.width, recorded.width)
            && agrees(estimate.container.height, recorded.height),
        "a size query container came out at {recorded:?}, not at the {:?} its own style and \
         layout input predicted before its contents were read",
        estimate.container,
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
        assert_eq!(publish(&mut table), []);
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

        table.note(id, container(200.0, Some(100.0)));
        assert_eq!(publish(&mut table), []);

        table.note(id, container(50.0, Some(100.0)));
        table.note(id, container(200.0, Some(100.0)));
        assert_eq!(publish(&mut table), vec![id, id]);
        assert_eq!(table.get(3), container(200.0, Some(100.0)));

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

        table.note(id(9), CommittedBox::default());
        assert_eq!(publish(&mut table), []);
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
