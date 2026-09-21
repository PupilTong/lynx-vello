//! The **last remembered size**
//! ([css-sizing-4 §5.2.1](https://drafts.csswg.org/css-sizing-4/#last-remembered)):
//! what an `auto` `contain-intrinsic-size` answers with once the box starts
//! skipping its contents.
//!
//! The `auto` keyword's whole clause is one sentence:
//!
//! > If `auto` is specified and the element has a last remembered size and is
//! > currently skipping its contents, its explicit intrinsic inner size in the
//! > corresponding axis is the last remembered size in that axis.
//!
//! and the two recording rules are two more:
//!
//! > At the time that ResizeObserver events are determined and delivered, if
//! > an element has a `auto` keyword in `contain-intrinsic-size` property, is
//! > capable of being a ResizeObserver target, but does not have size
//! > containment, record the current inner dimensions of its principal box as
//! > its last remembered size.
//! >
//! > At the time that ResizeObserver events are determined and delivered, if
//! > an element has a last remembered size but does not have `auto` keyword in
//! > `contain-intrinsic-size` property, remove its last remembered size.
//!
//! **This engine has no `ResizeObserver`, so the recording moment is the
//! commit's own layout run** — [`record`], called from the layout host after
//! an algorithm's committing run. That is the same box and the same numbers a
//! `ResizeObserver` would have been handed, read where they are produced rather
//! than replayed to an observer afterwards. It is also *earlier* than a
//! browser's: a browser delivers observations after the layout that produced
//! them, so a style change that starts the box skipping in the same frame uses
//! the previous frame's remembered size, while here the commit that laid the
//! box out is the one that remembers it. Both answer the spec's question —
//! what was this box's inner size the last time it was rendered — with the
//! last rendered size.
//!
//! The sizes are **unrounded CSS px**: the content box of the algorithm's own
//! output (`size - padding - border`), which is what "the current inner
//! dimensions of its principal box" names. Device rounding happens afterwards,
//! in the rounding tail, and is a property of the frame rather than of the
//! element.
//!
//! [csswg-drafts#8407](https://github.com/w3c/csswg-drafts/issues/8407) is
//! folded in here too: an element with `content-visibility: auto` behaves as
//! if its `contain-intrinsic-*` values carried `auto`. The fork owns the
//! mapping (`ContainIntrinsicSize::add_auto_if_needed`) but applies it in
//! `StyleAdjuster::adjust_for_contain_intrinsic_size`, which is
//! `#[cfg(feature = "gecko")]` and so never runs in this build — the fold is
//! therefore this module's, on the computed value, on the way into layout.

use std::cell::RefCell;

use hughie::compute::{used_border, used_padding};
use hughie::geometry::Size;
use hughie::style::{Contain, ContainIntrinsicSize, ContentVisibility, CoreStyle};
use hughie::tree::LayoutInput;
use stylo::properties::ComputedValues;
use stylo::values::computed::Length;
use stylo::values::generics::NonNegative;

use super::style::{StyleView, skips_contents};
use crate::tree::document::{NodeSlot, TreeArenas};
use crate::tree::node::Node;

/// One element's last remembered size, per physical axis, in unrounded CSS px.
///
/// An axis reads `None` when nothing is remembered for it — the state the
/// spec's "remove its last remembered size" leaves behind, and the state every
/// element starts in.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct RememberedSize {
    pub(crate) width: Option<f32>,
    pub(crate) height: Option<f32>,
}

impl RememberedSize {
    #[inline]
    pub(crate) const fn is_empty(self) -> bool {
        self.width.is_none() && self.height.is_none()
    }
}

/// The slot-keyed last-remembered-size table.
///
/// It lives on [`TreeArenas`] beside the `content-visibility: auto` relevance
/// table and for the same structural reason: the substitution is read through
/// a [`StyleView`], which is built from the tree arenas alone. It is *not* in
/// [`LayoutSlot`](hughie::tree::LayoutSlot) and not in `NodeLayoutState`,
/// because neither is reachable from a style view.
///
/// Lazily sized, like the relevance table and `DocumentLayoutState`: an absent
/// entry reads as "nothing remembered", so a page that never uses the `auto`
/// keyword allocates nothing here.
///
/// The `RefCell` is what the write side costs. Recording happens inside
/// [`LayoutTree::compute_layout`](hughie::tree::LayoutTree::compute_layout),
/// which takes the tree **shared** — the mutable half of that split is the
/// layout state, which a style view cannot reach — so the table owns its own
/// interior mutability. Every borrow is one statement long and no engine code
/// runs under one, so the flag can never be observed held.
#[derive(Debug, Default)]
pub(crate) struct RememberedSizeTable {
    entries: RefCell<Vec<RememberedSize>>,
}

impl RememberedSizeTable {
    #[inline]
    pub(crate) fn get(&self, key: usize) -> RememberedSize {
        self.entries.borrow().get(key).copied().unwrap_or_default()
    }

    /// Writes `size` for `key`, an empty `size` being the spec's removal.
    ///
    /// Allocates only when there is something to keep: recording nothing for a
    /// key the table has never reached leaves the vector untouched, which is
    /// what keeps a page with no `auto` keyword free of this table entirely.
    pub(crate) fn record(&self, key: usize, size: RememberedSize) {
        let mut entries = self.entries.borrow_mut();
        if entries.len() <= key {
            if size.is_empty() {
                return;
            }
            entries.resize(key + 1, RememberedSize::default());
        }
        entries[key] = size;
    }

    /// Resets a freed slot, so the key's next occupant remembers nothing.
    pub(crate) fn reset(&mut self, key: usize) {
        if let Some(entry) = self.entries.get_mut().get_mut(key) {
            *entry = RememberedSize::default();
        }
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
///
/// `hughie` needs none of this vocabulary: its `contain_intrinsic_length`
/// already reads `AutoLength(l)` as `l` and `AutoNone` as no explicit size, so
/// answering `Length(remembered)` is the whole substitution.
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
    match axis(node.arenas().remembered_size(node.id())) {
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

/// The recording moment: one committing layout run of one box.
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
    let style = view.values();
    let content_visibility = style.clone_content_visibility();
    let position = style.get_position();
    let auto = Size::new(
        has_effective_auto(&position.contain_intrinsic_width, content_visibility),
        has_effective_auto(&position.contain_intrinsic_height, content_visibility),
    );
    let id = tree.at(node).id();

    if !auto.width && !auto.height {
        // "if an element has a last remembered size but does not have auto
        // keyword in contain-intrinsic-size property, remove its last
        // remembered size." Free for the overwhelming majority of elements,
        // which never reached the table at all.
        tree.record_remembered_size(id, RememberedSize::default());
        return;
    }

    // "but does not have size containment". A size-contained box was laid out
    // as if it had no contents, so this run's inner dimensions are the
    // substituted estimate rather than anything its contents produced —
    // recording it would overwrite the measurement with the guess. Every
    // *skipping* box is in here too (skipping turns `SIZE` on), which is
    // exactly what lets a remembered size survive for as long as the box goes
    // on skipping.
    //
    // Containment is per axis, so this is too: under `contain: inline-size`
    // (and under `container-type: inline-size`, which implies it) the height
    // this run produced *is* the contents' own, and only the width is the
    // estimate. The spec's sentence names whole-box size containment, but
    // applying it per axis is what keeps its reason intact — an axis records
    // what its contents made, or keeps what it last remembered.
    let containment = view.containment();
    let contained = Size::new(
        containment.contains(Contain::INLINE_SIZE),
        containment.contains(Contain::BLOCK_SIZE),
    );
    if contained.width && contained.height {
        return;
    }

    // "the current inner dimensions of its principal box": the content box,
    // resolved the way the algorithms themselves resolve it — percentages
    // against the containing block's inline size, `none`/`hidden` border sides
    // reading zero.
    let padding = used_padding(view, input.parent_size.width);
    let border = used_border(view);
    let inner = Size::new(
        (size.width - padding.horizontal_sum() - border.horizontal_sum()).max(0.0),
        (size.height - padding.vertical_sum() - border.vertical_sum()).max(0.0),
    );
    // Only a contained axis has anything to look up — it keeps what it last
    // remembered — so the ordinary box never reads the table to write it.
    let previous = if contained.width || contained.height {
        tree.remembered_size(id)
    } else {
        RememberedSize::default()
    };
    let axis = |auto: bool, contained: bool, previous: Option<f32>, inner: f32| {
        if !auto {
            return None;
        }
        if contained { previous } else { Some(inner) }
    };
    tree.record_remembered_size(
        id,
        RememberedSize {
            width: axis(auto.width, contained.width, previous.width, inner.width),
            height: axis(auto.height, contained.height, previous.height, inner.height),
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
        ContainIntrinsicSize, ContentVisibility, RememberedSize, RememberedSizeTable, effective,
        has_auto, has_effective_auto,
    };

    fn length(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::Length(NonNegative(Length::new(value)))
    }

    fn auto_length(value: f32) -> ContainIntrinsicSize {
        ContainIntrinsicSize::AutoLength(NonNegative(Length::new(value)))
    }

    #[test]
    fn an_element_no_run_has_recorded_remembers_nothing() {
        let table = RememberedSizeTable::default();
        assert_eq!(table.get(9), RememberedSize::default());
        assert!(table.get(9).is_empty());

        // Recording nothing for an unreached key leaves the table empty, which
        // is what a page with no `auto` keyword does on every commit.
        table.record(9, RememberedSize::default());
        assert_eq!(table.get(9), RememberedSize::default());
    }

    #[test]
    fn recording_is_per_axis_and_removal_is_writing_an_empty_one() {
        let mut table = RememberedSizeTable::default();
        table.record(
            2,
            RememberedSize {
                width: None,
                height: Some(40.0),
            },
        );
        assert_eq!(table.get(2).height, Some(40.0));
        assert_eq!(table.get(2).width, None, "an axis without auto keeps none");
        assert_eq!(
            table.get(1),
            RememberedSize::default(),
            "and its neighbours are untouched"
        );

        table.record(
            2,
            RememberedSize {
                width: Some(10.0),
                height: Some(50.0),
            },
        );
        assert_eq!(table.get(2).width, Some(10.0));

        table.record(2, RememberedSize::default());
        assert!(table.get(2).is_empty(), "losing `auto` removes it");

        table.record(
            2,
            RememberedSize {
                width: Some(3.0),
                height: None,
            },
        );
        table.reset(2);
        assert!(table.get(2).is_empty(), "and a freed slot starts clean");
        table.reset(77);
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
