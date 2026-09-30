//! The absolute pass of an anchor-positioned box: one layout per position
//! option it tries (css-anchor-position-1 §6.5), and the [`AnchorOutcome`]
//! the host keeps.
//!
//! Only boxes that use anchor positioning or have position options come
//! here; `absolute_layout` sends every other box straight to
//! [`place_absolute`] with its style's own values, so they pay one predicate
//! and one host call (`position_option_count`) and nothing else.

use smallvec::SmallVec;
use stylo::values::computed::PositionTryOrder;

use super::anchor::{AnchoredGeometry, GeometryValues};
use super::{AbsoluteContainingBlock, AbsolutePlacement, Placed, place_absolute};
use crate::geometry::{Edges, Point, Rect, Size};
use crate::style::CoreStyle;
use crate::tree::{AnchorOutcome, Layout, LayoutGoal, LayoutTree, RequestedAxis};

/// One option laid out, with the facts the outcome reports.
struct Tried {
    placed: Placed,
    references_default_anchor: bool,
    has_default_anchor: bool,
    compensates: Size<bool>,
}

/// Lays `node` out with the geometry values of `style`, position option
/// `option` (`base` is the box's own style).
#[allow(
    clippy::too_many_arguments,
    reason = "absolute_layout's inputs plus the option"
)]
fn lay_out_with<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    base: &impl CoreStyle,
    style: &impl CoreStyle,
    option: usize,
    containing_block: &AbsoluteContainingBlock,
    static_position: &StaticPosition,
    goal: LayoutGoal,
) -> Tried
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    let geometry =
        AnchoredGeometry::resolve(tree, state, node, base, style, option, containing_block);
    let Some(geometry) = geometry else {
        let values = GeometryValues::of(style);
        let placement = AbsolutePlacement::plain(style, &values, containing_block);
        let placed = place_absolute(
            tree,
            state,
            node,
            base,
            &values,
            &placement,
            containing_block,
            static_position,
            goal,
        );
        return Tried {
            placed,
            references_default_anchor: false,
            has_default_anchor: false,
            compensates: Size::new(false, false),
        };
    };
    let placed = place_absolute(
        tree,
        state,
        node,
        base,
        &geometry.values(),
        &geometry.placement,
        containing_block,
        static_position,
        goal,
    );
    Tried {
        placed,
        references_default_anchor: geometry.references_default_anchor,
        has_default_anchor: geometry.has_default_anchor,
        compensates: geometry.compensates,
    }
}

/// Lays `node` out with position option `option`: the host's cascaded
/// option style when the box has options (§6.5.2 "apply a position
/// option"), its own style otherwise.
#[allow(
    clippy::too_many_arguments,
    reason = "absolute_layout's inputs plus the option"
)]
fn lay_out_option<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    base: &impl CoreStyle,
    option: usize,
    has_options: bool,
    containing_block: &AbsoluteContainingBlock,
    static_position: &StaticPosition,
    goal: LayoutGoal,
) -> Tried
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    if has_options {
        let style = tree.position_option_style(node, option);
        lay_out_with(
            tree,
            state,
            node,
            base,
            &style,
            option,
            containing_block,
            static_position,
            goal,
        )
    } else {
        lay_out_with(
            tree,
            state,
            node,
            base,
            base,
            option,
            containing_block,
            static_position,
            goal,
        )
    }
}

/// The inset-modified containing block size option `option` yields for
/// §6.2's sort, without laying the box out.
fn option_sort_size<T: LayoutTree>(
    tree: &T,
    state: &T::State,
    node: T::NodeId,
    base: &impl CoreStyle,
    option: usize,
    containing_block: &AbsoluteContainingBlock,
) -> Size<f32> {
    let style = tree.position_option_style(node, option);
    AnchoredGeometry::resolve(tree, state, node, base, &style, option, containing_block)
        .map_or_else(
            || {
                // No anchor positioning in this option: its containing block
                // is the handed-over one and its insets are its own.
                let inset = style.inset();
                let size = containing_block.size;
                let used =
                    |inset, basis| super::util::resolve_inset(inset, Some(basis)).unwrap_or(0.0);
                Size::new(
                    (size.width - used(inset.left, size.width) - used(inset.right, size.width))
                        .max(0.0),
                    (size.height - used(inset.top, size.height) - used(inset.bottom, size.height))
                        .max(0.0),
                )
            },
            |geometry| geometry.sort_size(),
        )
}

/// The absolute pass of a box that uses anchor positioning or has position
/// options. Out of line and cold: the absolute pass of every other box never
/// reaches it.
#[cold]
#[inline(never)]
pub(super) fn anchored_absolute_layout<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: &AbsoluteContainingBlock,
    static_position: &StaticPosition,
    goal: LayoutGoal,
) -> Layout
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    let base = tree.style(node);
    let count = tree.position_option_count(node);
    let has_options = count > 1;
    // §6.5: "Let current styles be the current used styles of abspos, as
    // derived from the computed base style (which might be the result of
    // earlier fallback)" — the last successful option, else the base style.
    let current = if has_options {
        tree.last_successful_option(state, node)
            .filter(|&option| option < count)
            .unwrap_or(0)
    } else {
        0
    };
    let LayoutGoal::Commit {
        content_independent,
    } = goal
    else {
        // A measurement (a static position) decides nothing: it lays the
        // box out with the current option and reports no outcome.
        return lay_out_option(
            tree,
            state,
            node,
            &base,
            current,
            has_options,
            containing_block,
            static_position,
            goal,
        )
        .placed
        .layout;
    };
    // Which option a box with options is committed with depends on its own
    // size, so its input is not stable under a change of its content.
    let commit = LayoutGoal::Commit {
        content_independent: if has_options {
            Size::new(false, false)
        } else {
            content_independent
        },
    };
    let tried = lay_out_option(
        tree,
        state,
        node,
        &base,
        current,
        has_options,
        containing_block,
        static_position,
        commit,
    );
    // §6.5: "When a positioned box … overflows its inset-modified
    // containing block, and has more than one position option in its
    // position options list, it determines position fallback styles".
    if !has_options || tried.placed.fits() {
        return report(tree, state, node, containing_block, current, tried);
    }

    // §6.2: with `position-try-order` other than `normal`, "Stably sort the
    // position options list according to this size, with the largest coming
    // first."
    let mut order: SmallVec<[usize; 8]> = (0..count).collect();
    let try_order = base.position_try_order();
    if !try_order.is_normal() {
        let keys: SmallVec<[f32; 8]> = order
            .iter()
            .map(|&option| {
                let size = option_sort_size(tree, state, node, &base, option, containing_block);
                match try_order {
                    // No `writing-mode`: the block axis is vertical.
                    PositionTryOrder::MostHeight | PositionTryOrder::MostBlockSize => size.height,
                    PositionTryOrder::MostWidth
                    | PositionTryOrder::MostInlineSize
                    | PositionTryOrder::Normal => size.width,
                }
            })
            .collect();
        order.sort_by(|&a, &b| keys[b].total_cmp(&keys[a]));
    }

    // "For each option in the position options list: If option is currently
    // abspos's last successful position option, continue." The current
    // option — the last successful one, or the base style the loop would
    // otherwise try again — was just laid out and overflowed.
    for option in order {
        if option == current {
            continue;
        }
        let measure = lay_out_option(
            tree,
            state,
            node,
            &base,
            option,
            has_options,
            containing_block,
            static_position,
            LayoutGoal::Measure(RequestedAxis::Both),
        );
        // "If cb rect was negative-size in either axis and corrected into
        // zero-size, continue. … If el rect is not fully contained within cb
        // rect, continue. Return adjusted styles".
        if measure.placed.fits() {
            let chosen = lay_out_option(
                tree,
                state,
                node,
                &base,
                option,
                has_options,
                containing_block,
                static_position,
                commit,
            );
            return report(tree, state, node, containing_block, option, chosen);
        }
    }
    // "Assert: The previous step finished without finding a position option
    // that avoids overflow. Return current styles." Its commit stands.
    report(tree, state, node, containing_block, current, tried)
}

/// Hands the host the outcome of a committed option and returns its layout.
fn report<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: &AbsoluteContainingBlock,
    chosen: usize,
    tried: Tried,
) -> Layout {
    let to_host = |rect: Rect<f32>| rect.translate(containing_block.origin);
    tree.set_anchor_outcome(
        state,
        node,
        AnchorOutcome {
            chosen,
            // §6.6 `no-overflow`: after the determination, the margin box
            // still overflows (every option did, or the box has none).
            overflows: !tried.placed.fits(),
            references_default_anchor: tried.references_default_anchor,
            default_anchor_resolved: tried.has_default_anchor,
            compensates: tried.compensates,
            imcb: to_host(tried.placed.imcb),
            margin_box: to_host(tried.placed.margin_box),
        },
    );
    tried.placed.layout
}
