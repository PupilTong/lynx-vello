//! The absolute pass of an anchor-positioned box: one layout per position
//! option it tries (css-anchor-position-1 §6.5), and the [`AnchorOutcome`]
//! the host keeps.
//!
//! Only boxes that use anchor positioning or have position options come
//! here; `absolute_layout` sends every other box straight to
//! [`place_absolute`] with its style's own values, so they pay two style
//! predicates and nothing else — a box whose style lists no
//! `position-try-fallbacks` is never asked about through a host call.

use smallvec::SmallVec;
use stylo::values::computed::PositionTryOrder;

use super::anchor::{AnchorMemo, AnchoredGeometry, imcb_size_auto_zero};
use super::{AbsoluteContainingBlock, Placed, place_absolute, place_without_anchors};
use crate::geometry::{Edges, Point, Rect, Size};
use crate::style::CoreStyle;
use crate::tree::{AnchorOutcome, Layout, LayoutGoal, LayoutTree, RequestedAxis};

struct Tried {
    placed: Placed,
    default_anchor_missing: bool,
    compensates: Size<bool>,
    carried_edges: Edges<bool>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "absolute_layout's inputs plus the option"
)]
fn lay_out_option<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    base: &impl CoreStyle,
    base_uses: bool,
    option: usize,
    containing_block: &AbsoluteContainingBlock,
    static_position: &StaticPosition,
    goal: LayoutGoal,
    memo: &mut AnchorMemo,
) -> Tried
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    let style = tree.position_option_style(node, option);
    let geometry = AnchoredGeometry::resolve(
        tree,
        state,
        node,
        base,
        base_uses,
        &style,
        option,
        containing_block,
        memo,
    );
    let Some(geometry) = geometry else {
        let placed = place_without_anchors(
            tree,
            state,
            node,
            base,
            &style,
            containing_block,
            static_position,
            goal,
        );
        return Tried {
            placed,
            default_anchor_missing: false,
            compensates: Size::new(false, false),
            carried_edges: Edges {
                left: false,
                right: false,
                top: false,
                bottom: false,
            },
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
        default_anchor_missing: geometry.default_anchor_missing,
        compensates: geometry.compensates,
        carried_edges: geometry.carried,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "absolute_layout's inputs plus the option and its memo"
)]
fn option_sort_size<T: LayoutTree>(
    tree: &T,
    state: &T::State,
    node: T::NodeId,
    base: &impl CoreStyle,
    base_uses: bool,
    option: usize,
    containing_block: &AbsoluteContainingBlock,
    memo: &mut AnchorMemo,
) -> Size<f32> {
    let style = tree.position_option_style(node, option);
    AnchoredGeometry::resolve(
        tree,
        state,
        node,
        base,
        base_uses,
        &style,
        option,
        containing_block,
        memo,
    )
    .map_or_else(
        || imcb_size_auto_zero(containing_block.size, style.inset()),
        |geometry| geometry.sort_size(),
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "§6.5's determination in its order, threading one memo"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "absolute_layout's inputs plus what it already read"
)]
#[cold]
#[inline(never)]
pub(super) fn anchored_absolute_layout<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    base: &impl CoreStyle,
    count: usize,
    base_uses: bool,
    containing_block: &AbsoluteContainingBlock,
    static_position: &StaticPosition,
    goal: LayoutGoal,
) -> Layout
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    let has_options = count > 1;
    let mut memo = AnchorMemo::default();
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
        return lay_out_option(
            tree,
            state,
            node,
            base,
            base_uses,
            current,
            containing_block,
            static_position,
            goal,
            &mut memo,
        )
        .placed
        .layout;
    };
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
        base,
        base_uses,
        current,
        containing_block,
        static_position,
        commit,
        &mut memo,
    );
    if !has_options || tried.placed.fits() {
        return report(tree, state, node, containing_block, current, tried);
    }

    let mut order: SmallVec<[usize; 8]> = (0..count).collect();
    let try_order = base.position_try_order();
    if !try_order.is_normal() {
        let keys: SmallVec<[f32; 8]> = order
            .iter()
            .map(|&option| {
                let size = option_sort_size(
                    tree,
                    state,
                    node,
                    base,
                    base_uses,
                    option,
                    containing_block,
                    &mut memo,
                );
                match try_order {
                    PositionTryOrder::MostHeight | PositionTryOrder::MostBlockSize => size.height,
                    PositionTryOrder::MostWidth
                    | PositionTryOrder::MostInlineSize
                    | PositionTryOrder::Normal => size.width,
                }
            })
            .collect();
        order.sort_by(|&a, &b| keys[b].total_cmp(&keys[a]));
    }

    for option in order {
        if option == current {
            continue;
        }
        let measure = lay_out_option(
            tree,
            state,
            node,
            base,
            base_uses,
            option,
            containing_block,
            static_position,
            LayoutGoal::Measure(RequestedAxis::Both),
            &mut memo,
        );
        if measure.placed.fits() {
            let chosen = lay_out_option(
                tree,
                state,
                node,
                base,
                base_uses,
                option,
                containing_block,
                static_position,
                commit,
                &mut memo,
            );
            return report(tree, state, node, containing_block, option, chosen);
        }
    }
    report(tree, state, node, containing_block, current, tried)
}

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
            overflows: !tried.placed.fits(),
            default_anchor_missing: tried.default_anchor_missing,
            compensates: tried.compensates,
            carried_edges: tried.carried_edges,
            imcb: to_host(tried.placed.imcb),
            margin_box: to_host(tried.placed.margin_box),
        },
    );
    tried.placed.layout
}
