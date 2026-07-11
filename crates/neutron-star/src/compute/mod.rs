//! Protocol machinery entry points — free generic functions over the tree
//! traits.
//!
//! There is deliberately no engine object: everything callable is a function
//! so that hosts compose them freely inside their
//! [`compute_child_layout`](crate::tree::LayoutTree::compute_child_layout)
//! dispatch, and so that unused entry points (and their monomorphizations)
//! never exist in the host's binary.
//!
//! This module contains the generic machinery (root entry, cache wrapper,
//! hidden zeroing, leaf boxing, the positioned pass, rounding) and the
//! implemented [`compute_flexbox_layout`] entry point. Grid layout remains
//! the L2 algorithm milestone.
//!
//! # The canonical dispatch skeleton
//!
//! Every host implements the same shape once; this is the whole integration
//! surface of the engine (a host with custom layout modes — e.g. lynx-vello's
//! `display: linear`/`relative` — adds arms that call its own algorithms):
//!
//! ```
//! use neutron_star::compute::{
//!     compute_cached_layout, compute_flexbox_layout, compute_hidden_layout,
//! };
//! use neutron_star::tree::{CacheTree, FlexTree, GridTree, LayoutInput, LayoutOutput, NodeId};
//!
//! # #[derive(Clone, Copy)]
//! enum Display {
//!     Flex,
//!     Grid,
//!     Hidden,
//! }
//!
//! fn dispatch<Tree>(tree: &mut Tree, node: NodeId, input: LayoutInput) -> LayoutOutput
//! where
//!     Tree: FlexTree + GridTree + CacheTree,
//! {
//!     compute_cached_layout(tree, node, input, |tree, node, input| {
//!         match host_display_of(tree, node) {
//!             Display::Hidden => compute_hidden_layout(tree, node),
//!             Display::Flex => compute_flexbox_layout(tree, node, input),
//!             Display::Grid => unimplemented!("grid layout lands in L2"),
//!             // host: Display::Linear => host_linear_layout(tree, node, input),
//!             // host: Display::Leaf => compute_leaf_layout(input, &style, resolve, measure),
//!         }
//!     })
//! }
//! # fn host_display_of<T>(_: &T, _: NodeId) -> Display { Display::Flex }
//! ```
//!
//! The host's `LayoutTree::compute_child_layout` implementation simply calls
//! its `dispatch`. Algorithms call back into `compute_child_layout` for each
//! child, so the same routing (and the same cache) applies at every level of
//! the tree.
//!
//! # Pass structure
//!
//! A full layout run is host-initiated passes in this order:
//!
//! 1. [`compute_root_layout`] — in-flow layout of the whole (dirty part of the) tree in unrounded
//!    CSS pixels. Out-of-flow nodes whose containing block is not their formatting parent
//!    ([`Position::AbsoluteHoisted`](crate::style::Position)) only get their static positions
//!    recorded here.
//! 2. [`compute_absolute_layout`] — the positioned pass: once per hoisted node, against its real
//!    containing block.
//! 3. [`round_layout`] — derives the device-pixel-snapped layouts. Optional but recommended for
//!    crisp rendering; kept separate so relayout always starts from unrounded values (re-rounding
//!    rounded values drifts).
mod flexbox;
mod leaf;
mod util;

pub use flexbox::compute_flexbox_layout;
pub use leaf::{LeafMeasurement, compute_leaf_layout};

use self::util::{
    apply_box_sizing, auto_edges_to_zero, clamp, resolve_edges, resolve_insets,
    resolve_optional_edges, resolve_size, scrollbar_size,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::{BoxGenerationMode, CoreStyle, Direction};
use crate::tree::{
    AvailableSpace, CacheTree, Layout, LayoutInput, LayoutOutput, LayoutTree, NodeId, RoundTree,
    RunMode,
};

/// Lays out the tree under `root` into `available_space`.
///
/// The host's entry point for a layout flush. Builds the root
/// [`LayoutInput`] (`PerformLayout`, no known dimensions, `parent_size` from
/// the definite parts of `available_space`), routes it through
/// [`compute_child_layout`](LayoutTree::compute_child_layout) — so the root
/// dispatches like any other node — resolves the root's own margins, and
/// stores the root's [`Layout`] (at location `(0, 0)`
/// plus resolved margins) via
/// [`set_unrounded_layout`](LayoutTree::set_unrounded_layout).
///
/// Incrementality: this walks — and pays for — only what caches miss. For a
/// clean subtree the recursion is answered from [`CacheTree`] storage at its
/// root.
pub fn compute_root_layout<Tree: LayoutTree>(
    tree: &mut Tree,
    root: NodeId,
    available_space: Size<AvailableSpace>,
) {
    let parent_size = available_space.into_options();
    let output = tree.compute_child_layout(
        root,
        LayoutInput::perform_layout(Size::NONE, parent_size, available_space),
    );

    let (hidden, margin, padding, border, scrollbar_size) = {
        let style = tree.core_style(root);
        let resolve_calc = |handle, basis| tree.resolve_calc(handle, basis);
        let margin_value = style.margin();
        let optional_margin =
            resolve_optional_edges(margin_value, parent_size.width, &resolve_calc);
        (
            style.box_generation_mode() == BoxGenerationMode::None,
            resolve_root_margins(
                optional_margin,
                margin_value.map(crate::style::LengthPercentageAuto::is_auto),
                available_space.width,
                output.size.width,
            ),
            resolve_edges(style.padding(), parent_size.width, &resolve_calc),
            resolve_edges(style.border(), parent_size.width, &resolve_calc),
            scrollbar_size(&style),
        )
    };

    if hidden {
        tree.set_unrounded_layout(root, &Layout::default());
        return;
    }

    let mut layout = Layout::with_order(0);
    layout.location = Point::new(margin.left, margin.top);
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.scrollbar_size = scrollbar_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    tree.set_unrounded_layout(root, &layout);
}

fn resolve_root_margins(
    optional: Edges<Option<f32>>,
    auto: Edges<bool>,
    available_width: AvailableSpace,
    box_width: f32,
) -> Edges<f32> {
    let mut margin = auto_edges_to_zero(optional);
    let AvailableSpace::Definite(available_width) = available_width else {
        return margin;
    };
    let auto_count = usize::from(auto.left) + usize::from(auto.right);
    if auto_count == 0 {
        return margin;
    }
    let remaining = (available_width
        - box_width
        - optional.left.unwrap_or(0.0)
        - optional.right.unwrap_or(0.0))
    .max(0.0);
    let share = if auto_count == 2 {
        remaining / 2.0
    } else {
        remaining
    };
    if auto.left {
        margin.left = share;
    }
    if auto.right {
        margin.right = share;
    }
    margin
}

/// Wraps one node's layout computation in the shared caching policy.
///
/// The host calls this at the top of its dispatch (see the module docs);
/// `compute_uncached` is the actual routing closure. The **complete
/// `input` is the cache key** — it is passed through to
/// [`CacheTree`] unmodified, so no result-affecting
/// field (`sizing_mode`, `parent_size`, `requested_axis`, …) can alias. On
/// a usable cached entry (matching per the [`cache`](crate::cache) module's
/// contract) the closure is skipped entirely; otherwise its result is
/// stored before being returned.
/// [`RunMode::PerformHiddenLayout`](crate::tree::RunMode) requests bypass
/// the cache in both directions.
pub fn compute_cached_layout<Tree, ComputeFn>(
    tree: &mut Tree,
    node: NodeId,
    input: LayoutInput,
    compute_uncached: ComputeFn,
) -> LayoutOutput
where
    Tree: CacheTree,
    ComputeFn: FnOnce(&mut Tree, NodeId, LayoutInput) -> LayoutOutput,
{
    if input.run_mode == RunMode::PerformHiddenLayout {
        return compute_uncached(tree, node, input);
    }

    if let Some(output) = tree.cache_get(node, input) {
        return output;
    }

    let output = compute_uncached(tree, node, input);
    tree.cache_store(node, input, output);
    output
}

/// Zeroes the layout of a `display: none` node and its whole subtree.
///
/// Stores an all-zero [`Layout`] for `node`'s children
/// and recurses through
/// [`compute_child_layout`](LayoutTree::compute_child_layout) with
/// [`RunMode::PerformHiddenLayout`](crate::tree::RunMode), so previously
/// laid-out geometry can't leak out of a subtree that just became hidden.
/// Every visited node is also passed to
/// [`LayoutTree::invalidate_layout_cache`], preventing a later cache hit from
/// restoring only a revealed subtree's root while its descendants stay
/// zeroed. Returns [`LayoutOutput::HIDDEN`].
pub fn compute_hidden_layout<Tree: LayoutTree>(tree: &mut Tree, node: NodeId) -> LayoutOutput {
    tree.invalidate_layout_cache(node);
    let hidden_layout = Layout::with_order(0);
    tree.set_unrounded_layout(node, &hidden_layout);

    let hidden_input = LayoutInput {
        run_mode: RunMode::PerformHiddenLayout,
        ..LayoutInput::default()
    };
    let child_count = tree.child_count(node);
    for index in 0..child_count {
        let child = tree.child_id(node, index);
        tree.set_unrounded_layout(child, &hidden_layout);
        let _ = tree.compute_child_layout(child, hidden_input);
    }

    LayoutOutput::HIDDEN
}

/// Sizes and positions one out-of-flow node against its containing block —
/// the host-driven **positioned pass** for
/// [`Position::AbsoluteHoisted`](crate::style::Position) nodes.
///
/// Runs after in-flow layout. The node's formatting parent computed and
/// recorded the node's static position
/// ([`set_static_position`](LayoutTree::set_static_position)) but did not
/// size or place it. The host resolves which node is the containing block
/// (for Lynx `fixed`: the viewport root, or the nearest
/// transformed/filtered ancestor per the W3C rule), converts the recorded
/// static position into that block's space once all required in-flow and
/// ancestor layouts are available, and calls this once per hoisted node
/// with:
///
/// - `containing_block`: the containing block's **padding-box size**, the basis for the node's
///   inset and percentage resolution;
/// - `static_position`: the converted static position (padding-box space), the anchor for any axis
///   whose insets are both `auto` (CSS Position / Flexbox §4.1 / Grid §10.1 semantics).
///
/// The node's subtree is laid out normally through
/// [`compute_child_layout`](LayoutTree::compute_child_layout) (descendants
/// store parent-relative layouts as usual, with normal caching). The node's
/// **own** layout is *returned, not stored*: its `location` is relative to
/// the containing block's **padding box**, which is generally not the
/// node's tree parent — the host converts it into formatting-parent space
/// and stores it via
/// [`set_unrounded_layout`](LayoutTree::set_unrounded_layout), keeping
/// [`Layout::location`]'s parent-relative contract intact for rounding and
/// painting.
#[must_use = "the returned layout is in containing-block space; the host must convert and store it"]
pub fn compute_absolute_layout<Tree: LayoutTree>(
    tree: &mut Tree,
    node: NodeId,
    containing_block: Size<f32>,
    static_position: Point<f32>,
) -> Layout {
    debug_assert!(
        containing_block.width.is_finite()
            && containing_block.height.is_finite()
            && containing_block.width >= 0.0
            && containing_block.height >= 0.0,
        "containing-block sizes must be finite and non-negative"
    );
    debug_assert!(
        static_position.x.is_finite() && static_position.y.is_finite(),
        "static positions must be finite"
    );

    let parent_size = Size::new(Some(containing_block.width), Some(containing_block.height));
    let resolved_style = resolve_absolute_style(tree, node, parent_size);
    let AbsoluteStyle {
        insets,
        optional_margin,
        padding,
        border,
        scrollbar_size,
        direction,
        ..
    } = resolved_style;

    let fixed_margin = auto_edges_to_zero(optional_margin);
    let inset_modified_size = Size::new(
        (containing_block.width - insets.left.unwrap_or(0.0) - insets.right.unwrap_or(0.0))
            .max(0.0),
        (containing_block.height - insets.top.unwrap_or(0.0) - insets.bottom.unwrap_or(0.0))
            .max(0.0),
    );

    let known_dimensions =
        absolute_known_dimensions(&resolved_style, inset_modified_size, fixed_margin);
    let output = tree.compute_child_layout(
        node,
        LayoutInput::perform_layout(
            known_dimensions,
            parent_size,
            Size::new(
                AvailableSpace::Definite(inset_modified_size.width),
                AvailableSpace::Definite(inset_modified_size.height),
            ),
        ),
    );

    let margin = resolve_absolute_margins(
        optional_margin,
        insets,
        inset_modified_size,
        output.size,
        direction,
    );
    let location = Point::new(
        absolute_axis_location(AbsoluteAxis {
            containing_size: containing_block.width,
            box_size: output.size.width,
            start_inset: insets.left,
            end_inset: insets.right,
            start_margin: margin.left,
            end_margin: margin.right,
            static_position: static_position.x,
            prefer_end: direction == Direction::Rtl,
        }),
        absolute_axis_location(AbsoluteAxis {
            containing_size: containing_block.height,
            box_size: output.size.height,
            start_inset: insets.top,
            end_inset: insets.bottom,
            start_margin: margin.top,
            end_margin: margin.bottom,
            static_position: static_position.y,
            prefer_end: false,
        }),
    );

    let mut layout = Layout::with_order(0);
    layout.location = location;
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.scrollbar_size = scrollbar_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    layout
}

#[derive(Clone, Copy)]
struct AbsoluteStyle {
    insets: Edges<Option<f32>>,
    optional_margin: Edges<Option<f32>>,
    padding: Edges<f32>,
    border: Edges<f32>,
    scrollbar_size: Size<f32>,
    auto_size: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    direction: Direction,
    padding_border_size: Size<f32>,
}

fn absolute_known_dimensions(
    style: &AbsoluteStyle,
    inset_modified_size: Size<f32>,
    fixed_margin: Edges<f32>,
) -> Size<Option<f32>> {
    // `auto` stretches only when both opposing insets are definite. These
    // are caller-decided border-box dimensions, clamped before they become
    // known dimensions. When horizontal stretch first establishes the width
    // of a two-auto-axis ratio box, height remains ratio-derived; otherwise
    // definite vertical insets may stretch height independently.
    let horizontal_stretch =
        style.auto_size.width && style.insets.left.is_some() && style.insets.right.is_some();
    let ratio_dependent_height = style.aspect_ratio.is_some()
        && horizontal_stretch
        && style.auto_size.width
        && style.auto_size.height;
    Size::new(
        horizontal_stretch.then_some(
            clamp(
                inset_modified_size.width - fixed_margin.horizontal_sum(),
                style.min_size.width,
                style.max_size.width,
            )
            .max(style.padding_border_size.width),
        ),
        (style.auto_size.height
            && !ratio_dependent_height
            && style.insets.top.is_some()
            && style.insets.bottom.is_some())
        .then_some(
            clamp(
                inset_modified_size.height - fixed_margin.vertical_sum(),
                style.min_size.height,
                style.max_size.height,
            )
            .max(style.padding_border_size.height),
        ),
    )
}

fn resolve_absolute_style<Tree: LayoutTree>(
    tree: &Tree,
    node: NodeId,
    parent_size: Size<Option<f32>>,
) -> AbsoluteStyle {
    let style = tree.core_style(node);
    let resolve_calc = |handle, basis| tree.resolve_calc(handle, basis);
    let padding = resolve_edges(style.padding(), parent_size.width, &resolve_calc);
    let border = resolve_edges(style.border(), parent_size.width, &resolve_calc);
    let padding_border_size = Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    );
    let style_size = style.size();
    let resolved_style_size = apply_box_sizing(
        resolve_size(style_size, parent_size, &resolve_calc),
        style.box_sizing(),
        padding_border_size,
    );
    let min_size = apply_box_sizing(
        resolve_size(style.min_size(), parent_size, &resolve_calc),
        style.box_sizing(),
        padding_border_size,
    );
    let max_size = apply_box_sizing(
        resolve_size(style.max_size(), parent_size, &resolve_calc),
        style.box_sizing(),
        padding_border_size,
    );

    AbsoluteStyle {
        insets: resolve_insets(style.inset(), parent_size, &resolve_calc),
        optional_margin: resolve_optional_edges(style.margin(), parent_size.width, &resolve_calc),
        padding,
        border,
        scrollbar_size: scrollbar_size(&style),
        auto_size: Size::new(
            style_size.width.is_auto() && resolved_style_size.width.is_none(),
            style_size.height.is_auto() && resolved_style_size.height.is_none(),
        ),
        min_size,
        max_size,
        aspect_ratio: style.aspect_ratio(),
        direction: style.direction(),
        padding_border_size,
    }
}

/// Derives device-pixel-snapped final layouts from the unrounded layouts
/// under `root`.
///
/// Walks the tree via [`RoundTree`], reading each
/// [`unrounded_layout`](RoundTree::unrounded_layout) and writing a rounded
/// copy through [`set_final_layout`](RoundTree::set_final_layout).
///
/// `scale` is the device-pixel ratio — physical pixels per CSS pixel (e.g.
/// `2.0`/`3.0` on high-DPI displays; `1.0` snaps to whole CSS pixels). It
/// must be finite and `> 0` (debug-asserted). Snapping happens on the
/// **device-pixel grid**, since layout coordinates are CSS pixels but crisp
/// edges are physical: `snap(v) = round(v × scale) / scale`.
///
/// Rounding contract (cumulative-error-free): positions are snapped in
/// *accumulated* (root-relative) space and sizes derived as
/// `snap(pos + size) - snap(pos)`, so adjacent edges land on the same
/// physical pixel and a box's snapped size never drifts more than one
/// device pixel from its unrounded size — at the cost that equal unrounded
/// sizes may snap to sizes differing by one device pixel (the standard
/// trade-off, also made by browsers). Idempotent given unchanged unrounded
/// inputs and scale.
pub fn round_layout<Tree: RoundTree>(tree: &mut Tree, root: NodeId, scale: f32) {
    debug_assert!(
        scale.is_finite() && scale > 0.0,
        "scale must be positive and finite"
    );
    round_layout_inner(tree, root, scale, Point::ZERO);
}

fn resolve_absolute_margins(
    optional: Edges<Option<f32>>,
    insets: Edges<Option<f32>>,
    available: Size<f32>,
    size: Size<f32>,
    direction: Direction,
) -> Edges<f32> {
    let mut margin = auto_edges_to_zero(optional);

    if insets.left.is_some() && insets.right.is_some() {
        let remaining = available.width
            - size.width
            - optional.left.unwrap_or(0.0)
            - optional.right.unwrap_or(0.0);
        match (optional.left.is_none(), optional.right.is_none()) {
            (true, true) if remaining < 0.0 && direction == Direction::Rtl => {
                margin.left = remaining;
            }
            (true, true) if remaining < 0.0 => margin.right = remaining,
            (true, true) => {
                margin.left = remaining / 2.0;
                margin.right = remaining / 2.0;
            }
            (true, false) => margin.left = remaining,
            (false, true) => margin.right = remaining,
            (false, false) => {}
        }
    }

    if insets.top.is_some() && insets.bottom.is_some() {
        let remaining = available.height
            - size.height
            - optional.top.unwrap_or(0.0)
            - optional.bottom.unwrap_or(0.0);
        match (optional.top.is_none(), optional.bottom.is_none()) {
            (true, true) => {
                margin.top = remaining / 2.0;
                margin.bottom = remaining / 2.0;
            }
            (true, false) => margin.top = remaining,
            (false, true) => margin.bottom = remaining,
            (false, false) => {}
        }
    }

    margin
}

#[inline]
fn absolute_axis_location(axis: AbsoluteAxis) -> f32 {
    let AbsoluteAxis {
        containing_size,
        box_size,
        start_inset,
        end_inset,
        start_margin,
        end_margin,
        static_position,
        prefer_end,
    } = axis;
    match (start_inset, end_inset) {
        (None, None) => static_position + start_margin,
        (Some(_), Some(end)) if prefer_end => containing_size - end - box_size - end_margin,
        (Some(start), _) => start + start_margin,
        (None, Some(end)) => containing_size - end - box_size - end_margin,
    }
}

#[derive(Clone, Copy)]
struct AbsoluteAxis {
    containing_size: f32,
    box_size: f32,
    start_inset: Option<f32>,
    end_inset: Option<f32>,
    start_margin: f32,
    end_margin: f32,
    static_position: f32,
    prefer_end: bool,
}

fn round_layout_inner<Tree: RoundTree>(
    tree: &mut Tree,
    node: NodeId,
    scale: f32,
    parent_position: Point<f32>,
) {
    let unrounded = tree.unrounded_layout(node);
    let position = Point::new(
        parent_position.x + unrounded.location.x,
        parent_position.y + unrounded.location.y,
    );
    let snap = |value: f32| (value * scale).round() / scale;

    let mut rounded = unrounded;
    rounded.location = Point::new(
        snap(position.x) - snap(parent_position.x),
        snap(position.y) - snap(parent_position.y),
    );
    rounded.size = Size::new(
        snap(position.x + unrounded.size.width) - snap(position.x),
        snap(position.y + unrounded.size.height) - snap(position.y),
    );
    rounded.content_size = Size::new(
        snap(position.x + unrounded.content_size.width) - snap(position.x),
        snap(position.y + unrounded.content_size.height) - snap(position.y),
    );
    rounded.scrollbar_size = Size::new(
        snap(position.x + unrounded.size.width)
            - snap(position.x + unrounded.size.width - unrounded.scrollbar_size.width),
        snap(position.y + unrounded.size.height)
            - snap(position.y + unrounded.size.height - unrounded.scrollbar_size.height),
    );
    rounded.border.left = snap(position.x + unrounded.border.left) - snap(position.x);
    rounded.border.right = snap(position.x + unrounded.size.width)
        - snap(position.x + unrounded.size.width - unrounded.border.right);
    rounded.border.top = snap(position.y + unrounded.border.top) - snap(position.y);
    rounded.border.bottom = snap(position.y + unrounded.size.height)
        - snap(position.y + unrounded.size.height - unrounded.border.bottom);
    rounded.padding.left = snap(position.x + unrounded.border.left + unrounded.padding.left)
        - snap(position.x + unrounded.border.left);
    rounded.padding.right = snap(position.x + unrounded.size.width - unrounded.border.right)
        - snap(
            position.x + unrounded.size.width - unrounded.border.right - unrounded.padding.right,
        );
    rounded.padding.top = snap(position.y + unrounded.border.top + unrounded.padding.top)
        - snap(position.y + unrounded.border.top);
    rounded.padding.bottom = snap(position.y + unrounded.size.height - unrounded.border.bottom)
        - snap(
            position.y + unrounded.size.height - unrounded.border.bottom - unrounded.padding.bottom,
        );
    rounded.margin.left = snap(position.x) - snap(position.x - unrounded.margin.left);
    rounded.margin.right = snap(position.x + unrounded.size.width + unrounded.margin.right)
        - snap(position.x + unrounded.size.width);
    rounded.margin.top = snap(position.y) - snap(position.y - unrounded.margin.top);
    rounded.margin.bottom = snap(position.y + unrounded.size.height + unrounded.margin.bottom)
        - snap(position.y + unrounded.size.height);

    tree.set_final_layout(node, &rounded);

    let child_count = tree.child_count(node);
    for index in 0..child_count {
        let child = tree.child_id(node, index);
        round_layout_inner(tree, child, scale, position);
    }
}
