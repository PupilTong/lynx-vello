//! Protocol machinery entry points — free generic functions over
//! [`LayoutNode`] handles.
//!
//! There is deliberately no engine object: everything callable is a function
//! so that hosts compose them freely inside their
//! [`compute_child_layout`](crate::tree::LayoutNode::compute_child_layout)
//! dispatch, and so that unused entry points (and their monomorphizations)
//! never exist in the host's binary. Every function takes the node it
//! operates on as a `Copy` handle; mutation flows through the handle into
//! host-owned interior-mutable per-node slots, so borrowed style views stay
//! valid across recursive child layout.
//!
//! This module contains the generic machinery (root entry, cache wrapper,
//! hidden-subtree zeroing, leaf boxing, the positioned pass, rounding) and the
//! implemented [`compute_flexbox_layout`], [`compute_grid_layout`],
//! [`compute_linear_layout`], and [`compute_relative_layout`] entry points.
//!
//! # The canonical dispatch skeleton
//!
//! Every host implements the same shape once inside its
//! [`LayoutNode::compute_child_layout`]; this is the whole integration
//! surface of the engine (a host with additional layout modes adds arms that
//! call its own algorithms):
//!
//! ```
//! use neutron_star::compute::{
//!     compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_linear_layout,
//!     compute_relative_layout, compute_skipped_contents_layout, hide_subtree,
//! };
//! use neutron_star::style::{
//!     CoreStyle, FlexContainerStyle, FlexItemStyle, GridContainerStyle, GridItemStyle,
//!     LinearContainerStyle, LinearItemStyle, RelativeContainerStyle, RelativeItemStyle,
//! };
//! use neutron_star::tree::{LayoutInput, LayoutNode, LayoutOutput};
//!
//! # #[derive(Clone, Copy)]
//! enum Display {
//!     Flex,
//!     Grid,
//!     Linear,
//!     Relative,
//!     Hidden,
//! }
//!
//! fn dispatch<N>(node: N, input: LayoutInput) -> LayoutOutput
//! where
//!     N: LayoutNode,
//!     N::Style: FlexContainerStyle
//!         + FlexItemStyle
//!         + GridContainerStyle
//!         + GridItemStyle
//!         + LinearContainerStyle
//!         + LinearItemStyle
//!         + RelativeContainerStyle
//!         + RelativeItemStyle,
//! {
//!     let display = host_display_of(node);
//!     if let Display::Hidden = display {
//!         // Hidden mutation must precede the cache wrapper: caching HIDDEN as
//!         // a committed result would suppress geometry when the node reappears.
//!         hide_subtree(node);
//!         return LayoutOutput::HIDDEN;
//!     }
//!
//!     // content-visibility skipping routes here next, still outside the cache:
//!     // it sizes the box from contain-intrinsic-size and hides its contents.
//!     if node.style().skips_contents() {
//!         return compute_skipped_contents_layout(node, input);
//!     }
//!
//!     compute_cached_layout(node, input, |node, input| {
//!         match display {
//!             Display::Hidden => unreachable!(),
//!             Display::Flex => compute_flexbox_layout(node, input),
//!             Display::Grid => compute_grid_layout(node, input),
//!             Display::Linear => compute_linear_layout(node, input),
//!             Display::Relative => compute_relative_layout(node, input),
//!             // host: Display::Leaf => compute_leaf_layout(input, &style, natural_size),
//!         }
//!     })
//! }
//! # fn host_display_of<N>(_: N) -> Display { Display::Flex }
//! ```
//!
//! # Independent formatting contexts and layout containment
//!
//! Each display mode here is already its own formatting context (there is no
//! block flow and thus no margin collapsing yet), so `contain: layout`'s
//! "independent formatting context / no margin-collapse across the boundary"
//! requirement is satisfied structurally. When block layout and its margin
//! collapsing land, a `LAYOUT`-contained box must additionally suppress
//! collapsing through its boundary. Layout containment has two *active* v1
//! effects, both at each algorithm's output construction: it suppresses the
//! container baseline exported to the parent, and — per
//! [css-contain-2 §3.3](https://drafts.csswg.org/css-contain-2/#containment-layout)
//! — it collapses the box's own scrollable overflow to its border box when
//! `overflow: visible` (descendant overflow becomes ink overflow), via the
//! `own_scrollable_overflow` helper. Orthogonally, scrollable overflow is
//! *trapped* at every scroll container ([css-overflow-3
//! §3.3](https://drafts.csswg.org/css-overflow-3/#scrollable)): a
//! scroll-container child contributes only its border box to its container's
//! `content_size` (the `accumulate_scrollable_overflow` helper), regardless of
//! containment.
//!
//! The host's `LayoutNode::compute_child_layout` implementation simply calls
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
//!    ([`PositionProperty::Fixed`](crate::style::PositionProperty)) only get their static positions
//!    recorded here.
//! 2. [`compute_absolute_layout`] — the positioned pass: once per hoisted node, against its real
//!    containing block.
//! 3. [`round_layout`] — derives the device-pixel-snapped layouts. Optional but recommended for
//!    crisp rendering; kept separate so relayout always starts from unrounded values (re-rounding
//!    rounded values drifts).
mod flexbox;
mod grid;
mod leaf;
mod linear;
mod relative;
mod util;

pub use flexbox::compute_flexbox_layout;
pub use grid::compute_grid_layout;
pub(crate) use leaf::compute_leaf_layout_with_measurement;
#[cfg(feature = "layout-test-utils")]
#[doc(hidden)]
pub use leaf::compute_leaf_layout_with_measurement_for_testing;
pub use leaf::{LeafMeasureInput, LeafMetrics, NaturalSize, compute_leaf_layout};
pub use linear::compute_linear_layout;
pub use relative::compute_relative_layout;
use stylo::computed_values::direction;
use stylo::values::computed::{Margin, Size as StyleSize};

use self::util::{
    apply_box_sizing, auto_edges_to_zero, clamp, clamp_axis, resolve_border, resolve_container_box,
    resolve_insets, resolve_length_percentage, resolve_margins, resolve_max_sizes, resolve_padding,
    resolve_size, used_aspect_ratio,
};
use crate::geometry::{Edges, Point, Size};
use crate::invalidate::is_relayout_boundary;
use crate::style::CoreStyle;
use crate::style::containment::contain_intrinsic_length;
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis,
};

/// Lays out the tree under `root` into `available_space`.
///
/// The host's entry point for a layout flush. Builds the root
/// [`LayoutInput`] ([`LayoutGoal::Commit`],
/// no known dimensions, `parent_size` from
/// the definite parts of `available_space`), routes it through
/// [`compute_child_layout`](LayoutNode::compute_child_layout) — so the root
/// dispatches like any other node — resolves the root's own margins, and
/// stores the root's [`Layout`] (at location `(0, 0)`
/// plus resolved margins) via
/// [`set_unrounded_layout`](LayoutNode::set_unrounded_layout).
///
/// Incrementality: this walks — and pays for — only what caches miss. For a
/// clean subtree the recursion is answered from the host's cache slots at
/// its root.
pub fn compute_root_layout<N: LayoutNode>(root: N, available_space: Size<AvailableSpace>) {
    let parent_size = available_space.into_options();
    let output = root.compute_child_layout(LayoutInput::perform_layout(
        Size::NONE,
        parent_size,
        available_space,
    ));

    let style = root.style();
    let margin_value = style.margin();
    let optional_margin = resolve_margins(margin_value, parent_size.width);
    let hidden = style.display().is_none();
    let margin = resolve_root_margins(
        optional_margin,
        margin_value.map(Margin::is_auto),
        available_space.width,
        output.size.width,
    );
    let padding = resolve_padding(style.padding(), parent_size.width);
    let border = resolve_border(&style.border());

    if hidden {
        root.set_unrounded_layout(Layout::default());
        return;
    }

    let mut layout = Layout::with_order(0);
    layout.location = Point::new(margin.left, margin.top);
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    root.set_unrounded_layout(layout);
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

/// Re-runs a **relayout boundary** in place after an internal mutation, reusing
/// the exact [`LayoutInput`] it was last committed with.
///
/// A [`is_relayout_boundary`] node
/// (`contain: strict`, or a skipped `content-visibility` box) is safely
/// re-rootable: an internal descendant change can neither escape the box's
/// formatting context nor alter its own outer size. But a boundary's used size
/// is frequently **parent-imposed** — a stretched cross size
/// (`align-items: stretch`), a flex-grown main size, a resolved percentage.
/// [`compute_root_layout`] would discard that, synthesizing a fresh input from
/// `available_space` only, so the boundary would re-derive its *self-determined*
/// size and desync from its un-invalidated ancestors.
///
/// This entry instead re-runs `node` with the **verbatim** `input` — obtain it
/// from [`Cache::committed_input`](crate::cache::Cache::committed_input) *before*
/// [`invalidate_for_relayout`](crate::invalidate::invalidate_for_relayout)
/// clears the cache. Because identical input plus unchanged style
/// deterministically yields an identical outer size (size containment
/// guarantees interior changes cannot affect it), **only the interior
/// re-arranges** and every ancestor stays valid. `input`'s
/// [`goal`](LayoutGoal) is normalized to
/// [`Commit`](LayoutGoal::Commit): a committed slot always carries `Commit`
/// already, but a stray `Measure` input must not be replayed as a
/// geometry-storing pass.
///
/// Unlike [`compute_root_layout`], this does **not** store `node`'s own
/// [`Layout`]: at a boundary that record belongs to the still-valid parent and
/// is unchanged. The recursive commit refreshes the interior and re-populates
/// `node`'s cache; the returned [`LayoutOutput`] (its `size` equal to the
/// boundary's unchanged outer size) is handed back for the host to consume.
pub fn compute_boundary_relayout<N: LayoutNode>(node: N, input: LayoutInput) -> LayoutOutput {
    debug_assert!(
        is_relayout_boundary(&node.style()),
        "compute_boundary_relayout requires a relayout boundary \
         (contain: strict, or a skipped content-visibility box)"
    );
    let mut input = input;
    input.goal = LayoutGoal::Commit;
    node.compute_child_layout(input)
}

/// Wraps one node's layout computation in the shared caching policy.
///
/// After handling `display: none` with [`hide_subtree`], the host calls this
/// at the top of its visible-node dispatch (see the module docs);
/// `compute_uncached` is the actual routing closure. The **complete
/// `input` is the cache key** — it is passed through to the node's cache
/// slots unmodified, so no result-affecting
/// field (`goal`, `sizing_mode`, `parent_size`, …) can alias. On
/// a usable cached entry (matching per the [`cache`](crate::cache) module's
/// contract) the closure is skipped entirely; otherwise its result is
/// stored before being returned.
///
/// Hidden nodes must never enter this wrapper: [`hide_subtree`] invalidates
/// their cache before zeroing geometry, whereas storing
/// [`LayoutOutput::HIDDEN`] as a committed answer would undo that invariant.
pub fn compute_cached_layout<N, ComputeFn>(
    node: N,
    input: LayoutInput,
    compute_uncached: ComputeFn,
) -> LayoutOutput
where
    N: LayoutNode,
    ComputeFn: FnOnce(N, LayoutInput) -> LayoutOutput,
{
    if let Some(output) = node.cache_get(input) {
        return output;
    }

    let output = compute_uncached(node, input);
    node.cache_store(input, output);
    output
}

/// Zeroes the layout of a `display: none` node and its whole subtree.
///
/// Recurses directly through tree children, storing an all-zero [`Layout`] for
/// every visited node so previously-laid-out geometry cannot leak from a
/// subtree that just became hidden. Every node is first
/// [`cache_clear`](LayoutNode::cache_clear)ed, preventing a later cache hit
/// from restoring only a revealed subtree's root while its descendants stay
/// zeroed.
///
/// Host dispatch must call this command **before** [`compute_cached_layout`]
/// and then return [`LayoutOutput::HIDDEN`] itself.
pub fn hide_subtree<N: LayoutNode>(node: N) {
    node.cache_clear();
    node.set_unrounded_layout(Layout::with_order(0));

    for child in node.children() {
        hide_subtree(child);
    }
}

/// Lays out a box whose **contents are skipped** — `content-visibility:
/// hidden` (and `auto` while off-screen), reported by
/// [`CoreStyle::skips_contents`].
///
/// The node still generates its own principal box: it is sized purely from its
/// styles with `contain-intrinsic-{width,height}` substituted for the
/// content-derived size (both axes, since a skipped box is size-contained by
/// contract — see [`CoreStyle::skips_contents`]), and **none of its children
/// are laid out**. On a [`LayoutGoal::Commit`] each child subtree is passed to
/// [`hide_subtree`], mirroring the `display: none` discipline so stale
/// descendant geometry and caches from a previous non-skipped pass are cleaned.
/// A [`LayoutGoal::Measure`] probe stays side-effect free (no hiding).
///
/// Because the returned box has no laid-out contents, its `content_size`
/// equals its border box and it exports no baseline (matching layout
/// containment).
///
/// # Dispatch placement (host contract)
///
/// Route here **before** [`compute_cached_layout`], right after the
/// `display: none` check:
///
/// ```text
/// if style.display().is_none() { hide_subtree(node); return HIDDEN; }
/// if style.skips_contents()    { return compute_skipped_contents_layout(node, input); }
/// compute_cached_layout(node, input, <algorithm dispatch>)
/// ```
///
/// Like [`hide_subtree`], the child-hiding here deliberately **precedes and
/// bypasses the cache boundary**: caching a skipped result and later serving it
/// on a hit would leave a re-populated child subtree un-hidden. Sizing a
/// contentless box is cheap and `hide_subtree` is far cheaper than laying the
/// subtree out, so recomputing it per pass is acceptable; a normal→skipped
/// transition (the host [`cache_clear`](LayoutNode::cache_clear)s on the style
/// change) therefore always re-hides the freshly-orphaned descendants, and a
/// skipped→normal transition re-dispatches to the algorithm.
pub fn compute_skipped_contents_layout<N: LayoutNode>(node: N, input: LayoutInput) -> LayoutOutput {
    let style = node.style();
    let metrics = resolve_container_box(&style, input);
    let intrinsic = Size::new(
        contain_intrinsic_length(&style.contain_intrinsic_width()),
        contain_intrinsic_length(&style.contain_intrinsic_height()),
    );
    let outer_size = Size::new(
        metrics.outer.width.unwrap_or_else(|| {
            clamp_axis(
                intrinsic.width.unwrap_or(0.0) + metrics.box_inset.width,
                metrics.min.width,
                metrics.max.width,
                metrics.box_inset.width,
            )
        }),
        metrics.outer.height.unwrap_or_else(|| {
            clamp_axis(
                intrinsic.height.unwrap_or(0.0) + metrics.box_inset.height,
                metrics.min.height,
                metrics.max.height,
                metrics.box_inset.height,
            )
        }),
    );

    if input.goal == LayoutGoal::Commit {
        // Clean any descendant geometry/caches left by a prior non-skipped
        // pass. This precedes and bypasses the cache, mirroring hide_subtree.
        for child in node.children() {
            hide_subtree(child);
        }
    }

    LayoutOutput::new(outer_size, outer_size)
}

/// Sizes and positions one out-of-flow node against its containing block —
/// the host-driven **positioned pass** for
/// [`PositionProperty::Fixed`](crate::style::PositionProperty) nodes.
///
/// Runs after in-flow layout. The node's formatting parent computed and
/// recorded the node's static position
/// ([`set_static_position`](LayoutNode::set_static_position)) but did not
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
///   whose insets are both `auto` (CSS Position / Flexbox §4.1 / Grid §10.2 semantics).
///
/// The node's subtree is laid out normally through
/// [`compute_child_layout`](LayoutNode::compute_child_layout) (descendants
/// store parent-relative layouts as usual, with normal caching). The node's
/// **own** layout is *returned, not stored*: its `location` is relative to
/// the containing block's **padding box**, which is generally not the
/// node's tree parent — the host converts it into formatting-parent space
/// and stores it via
/// [`set_unrounded_layout`](LayoutNode::set_unrounded_layout), keeping
/// [`Layout::location`]'s parent-relative contract intact for rounding and
/// painting. The returned [`Layout::order`] is zero; the host's positioned
/// pass assigns the formatting parent's order-modified paint index when it
/// stores a hoisted node.
#[must_use = "the returned layout is in containing-block space; the host must convert and store it"]
pub fn compute_absolute_layout<N: LayoutNode>(
    node: N,
    containing_block: Size<f32>,
    static_position: Point<f32>,
) -> Layout {
    absolute_layout(
        node,
        containing_block,
        move |_, _| static_position,
        LayoutGoal::Commit,
    )
}

/// Commits one out-of-flow child while deriving its static position from the
/// resolved border-box size and used margins.
pub(super) fn compute_absolute_layout_with_static_position<N, StaticPosition>(
    node: N,
    containing_block: Size<f32>,
    static_position: StaticPosition,
) -> Layout
where
    N: LayoutNode,
    StaticPosition: FnOnce(Size<f32>, Edges<f32>) -> Point<f32>,
{
    absolute_layout(node, containing_block, static_position, LayoutGoal::Commit)
}

/// Measures an out-of-flow node with the same inset, automatic-size,
/// aspect-ratio, min/max, and margin rules as [`compute_absolute_layout`].
///
/// Formatting algorithms use this side-effect-free probe when a hoisted
/// out-of-flow node's static position depends on its margin-box size. The
/// requested axis can be narrowed to the inset pair that actually needs a
/// static fallback. The returned layout uses a zero static position; callers
/// consume its `size` and `margin` to record the formatting-context-specific
/// static position for the later positioned pass. No durable child geometry
/// is written by this measurement.
#[must_use]
pub(super) fn measure_absolute_layout<N: LayoutNode>(
    node: N,
    containing_block: Size<f32>,
    requested_axis: RequestedAxis,
) -> Layout {
    absolute_layout(
        node,
        containing_block,
        |_, _| Point::ZERO,
        LayoutGoal::Measure(requested_axis),
    )
}

fn absolute_layout<N, StaticPosition>(
    node: N,
    containing_block: Size<f32>,
    static_position: StaticPosition,
    goal: LayoutGoal,
) -> Layout
where
    N: LayoutNode,
    StaticPosition: FnOnce(Size<f32>, Edges<f32>) -> Point<f32>,
{
    debug_assert!(
        containing_block.width.is_finite()
            && containing_block.height.is_finite()
            && containing_block.width >= 0.0
            && containing_block.height >= 0.0,
        "containing-block sizes must be finite and non-negative"
    );
    let parent_size = Size::new(Some(containing_block.width), Some(containing_block.height));
    let resolved_style = resolve_absolute_style(node, parent_size);
    let ResolvedAbsoluteStyle {
        insets,
        optional_margin,
        padding,
        border,
        preferred_available,
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
    let available_space = Size::new(
        preferred_available
            .width
            .unwrap_or(AvailableSpace::Definite(inset_modified_size.width)),
        preferred_available
            .height
            .unwrap_or(AvailableSpace::Definite(inset_modified_size.height)),
    );
    let child_input = match goal {
        LayoutGoal::Commit => {
            LayoutInput::perform_layout(known_dimensions, parent_size, available_space)
        }
        LayoutGoal::Measure(requested_axis) => LayoutInput::compute_size(
            known_dimensions,
            parent_size,
            available_space,
            requested_axis,
        ),
    };
    let output = node.compute_child_layout(child_input);

    let margin = resolve_absolute_margins(
        optional_margin,
        insets,
        inset_modified_size,
        output.size,
        direction,
    );
    let static_position = static_position(output.size, margin);
    debug_assert!(
        static_position.x.is_finite() && static_position.y.is_finite(),
        "static positions must be finite"
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
            prefer_end: direction == direction::T::Rtl,
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
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    layout
}

/// Resolved box-model and positioning inputs retained across the recursive
/// child-layout call for one absolutely positioned node.
#[derive(Clone, Copy)]
struct ResolvedAbsoluteStyle {
    insets: Edges<Option<f32>>,
    optional_margin: Edges<Option<f32>>,
    padding: Edges<f32>,
    border: Edges<f32>,
    /// Intrinsic preferred sizes must be measured in their own sizing mode;
    /// the inset-modified containing block is only the fallback available
    /// space for `auto`. A definite fit-content limit is retained here even
    /// though `resolve_size` deliberately leaves intrinsic dimensions
    /// unresolved.
    preferred_available: Size<Option<AvailableSpace>>,
    auto_size: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
    direction: direction::T,
    padding_border_size: Size<f32>,
}

fn absolute_known_dimensions(
    style: &ResolvedAbsoluteStyle,
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

/// Whether a preferred-size value behaves as `auto` for stretch purposes.
/// The lynx-parseable keywords Starlight has no sizing behavior for (bare
/// `fit-content`, `stretch`, `-webkit-fill-available`) are treated as `auto`
/// (documented vocabulary-swap delta).
#[inline]
fn style_size_behaves_auto(value: &StyleSize) -> bool {
    match value {
        StyleSize::Auto
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => true,
        StyleSize::LengthPercentage(_)
        | StyleSize::MinContent
        | StyleSize::MaxContent
        | StyleSize::FitContentFunction(_) => false,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

fn resolve_absolute_style<N: LayoutNode>(
    node: N,
    parent_size: Size<Option<f32>>,
) -> ResolvedAbsoluteStyle {
    let style = node.style();
    let padding = resolve_padding(style.padding(), parent_size.width);
    let border = resolve_border(&style.border());
    let padding_border_size = Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    );
    let style_size = style.size();
    let preferred_available = Size::new(
        absolute_preferred_available(style_size.width, parent_size.width),
        absolute_preferred_available(style_size.height, parent_size.height),
    );
    let resolved_style_size = apply_box_sizing(
        resolve_size(style_size, parent_size),
        style.box_sizing(),
        padding_border_size,
    );
    let min_size = apply_box_sizing(
        resolve_size(style.min_size(), parent_size),
        style.box_sizing(),
        padding_border_size,
    );
    let max_size = apply_box_sizing(
        resolve_max_sizes(style.max_size(), parent_size),
        style.box_sizing(),
        padding_border_size,
    );

    ResolvedAbsoluteStyle {
        insets: resolve_insets(style.inset(), parent_size),
        optional_margin: resolve_margins(style.margin(), parent_size.width),
        padding,
        border,
        preferred_available,
        auto_size: Size::new(
            style_size_behaves_auto(style_size.width) && resolved_style_size.width.is_none(),
            style_size_behaves_auto(style_size.height) && resolved_style_size.height.is_none(),
        ),
        min_size,
        max_size,
        aspect_ratio: used_aspect_ratio(style.aspect_ratio()),
        direction: style.direction(),
        padding_border_size,
    }
}

/// The intrinsic available-space override selected by an intrinsic preferred
/// size on an absolutely positioned box.
#[inline]
fn absolute_preferred_available(value: &StyleSize, basis: Option<f32>) -> Option<AvailableSpace> {
    match value {
        StyleSize::MinContent => Some(AvailableSpace::MinContent),
        StyleSize::MaxContent => Some(AvailableSpace::MaxContent),
        StyleSize::FitContentFunction(limit) => resolve_length_percentage(&limit.0, basis)
            .map(|limit| AvailableSpace::Definite(limit.max(0.0))),
        StyleSize::LengthPercentage(_)
        | StyleSize::Auto
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => None,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

/// Derives device-pixel-snapped final layouts from the unrounded layouts
/// under `root`.
///
/// Walks the subtree rooted at `root` (including `root` itself), reading
/// each node's [`clone_unrounded_layout`](LayoutNode::clone_unrounded_layout) and
/// writing a rounded copy through
/// [`set_final_layout`](LayoutNode::set_final_layout).
///
/// `scale` is the device-pixel ratio — physical pixels per CSS pixel (e.g.
/// `2.0`/`3.0` on high-DPI displays; `1.0` snaps to whole CSS pixels). It
/// must be finite and `> 0` (debug-asserted). Snapping happens on the
/// **device-pixel grid**, since layout coordinates are CSS pixels but crisp
/// edges are physical: `snap(v) = css_round(v × scale) / scale`. CSS nearest-
/// integer rounding chooses the value toward positive infinity at an exact
/// half-way tie (`1.5 → 2`, `-1.5 → -1`).
///
/// Rounding contract (cumulative-error-free): positions are snapped in
/// *accumulated* (root-relative) space and sizes derived as
/// `snap(pos + size) - snap(pos)`, so adjacent edges land on the same
/// physical pixel and a box's snapped size never drifts more than one
/// device pixel from its unrounded size — at the cost that equal unrounded
/// sizes may snap to sizes differing by one device pixel (the standard
/// trade-off, also made by browsers). Idempotent given unchanged unrounded
/// inputs and scale.
pub fn round_layout<N: LayoutNode>(root: N, scale: f32) {
    debug_assert!(
        scale.is_finite() && scale > 0.0,
        "scale must be positive and finite"
    );
    round_layout_inner(root, scale, Point::ZERO);
}

/// CSS Values' nearest-integer rule: choose the upper integer on an exact
/// half-way tie. This intentionally differs from Rust's `f32::round` for
/// negative halves, where `-1.5` rounds away from zero to `-2`.
#[inline]
fn css_round_to_integer(value: f32) -> f32 {
    debug_assert!(value.is_finite(), "CSS pixel coordinates must be finite");
    let lower = value.floor();
    if value - lower < 0.5 {
        lower
    } else {
        lower + 1.0
    }
}

fn resolve_absolute_margins(
    optional: Edges<Option<f32>>,
    insets: Edges<Option<f32>>,
    available: Size<f32>,
    size: Size<f32>,
    direction: direction::T,
) -> Edges<f32> {
    let mut margin = auto_edges_to_zero(optional);

    if insets.left.is_some() && insets.right.is_some() {
        let remaining = available.width
            - size.width
            - optional.left.unwrap_or(0.0)
            - optional.right.unwrap_or(0.0);
        match (optional.left.is_none(), optional.right.is_none()) {
            (true, true) if remaining < 0.0 && direction == direction::T::Rtl => {
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

/// One physical-axis instance of the absolute-position equation used to turn
/// insets, used margins, and the static fallback into a border-box offset.
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

fn round_layout_inner<N: LayoutNode>(node: N, scale: f32, parent_position: Point<f32>) {
    // The rounded result is a second durable record by design. Keep this one
    // required whole-Layout duplication explicit in the protocol operation's
    // name rather than making a large by-value read look free.
    let unrounded = node.clone_unrounded_layout();
    let position = Point::new(
        parent_position.x + unrounded.location.x,
        parent_position.y + unrounded.location.y,
    );
    let source_size = unrounded.size;
    let source_content_size = unrounded.content_size;
    let source_border = unrounded.border;
    let source_padding = unrounded.padding;
    let source_margin = unrounded.margin;
    let snap = |value: f32| css_round_to_integer(value * scale) / scale;
    let mut rounded = unrounded;
    rounded.location = Point::new(
        snap(position.x) - snap(parent_position.x),
        snap(position.y) - snap(parent_position.y),
    );
    rounded.size = Size::new(
        snap(position.x + source_size.width) - snap(position.x),
        snap(position.y + source_size.height) - snap(position.y),
    );
    rounded.content_size = Size::new(
        snap(position.x + source_content_size.width) - snap(position.x),
        snap(position.y + source_content_size.height) - snap(position.y),
    );
    rounded.border.left = snap(position.x + source_border.left) - snap(position.x);
    rounded.border.right = snap(position.x + source_size.width)
        - snap(position.x + source_size.width - source_border.right);
    rounded.border.top = snap(position.y + source_border.top) - snap(position.y);
    rounded.border.bottom = snap(position.y + source_size.height)
        - snap(position.y + source_size.height - source_border.bottom);
    rounded.padding.left = snap(position.x + source_border.left + source_padding.left)
        - snap(position.x + source_border.left);
    rounded.padding.right = snap(position.x + source_size.width - source_border.right)
        - snap(position.x + source_size.width - source_border.right - source_padding.right);
    rounded.padding.top = snap(position.y + source_border.top + source_padding.top)
        - snap(position.y + source_border.top);
    rounded.padding.bottom = snap(position.y + source_size.height - source_border.bottom)
        - snap(position.y + source_size.height - source_border.bottom - source_padding.bottom);
    rounded.margin.left = snap(position.x) - snap(position.x - source_margin.left);
    rounded.margin.right = snap(position.x + source_size.width + source_margin.right)
        - snap(position.x + source_size.width);
    rounded.margin.top = snap(position.y) - snap(position.y - source_margin.top);
    rounded.margin.bottom = snap(position.y + source_size.height + source_margin.bottom)
        - snap(position.y + source_size.height);

    node.set_final_layout(rounded);

    for child in node.children() {
        round_layout_inner(child, scale, position);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn root_auto_margins_cover_indefinite_fixed_single_and_double_auto_cases() {
        let fixed = Edges {
            left: Some(3.0),
            right: Some(7.0),
            top: Some(2.0),
            bottom: Some(4.0),
        };
        assert_eq!(
            resolve_root_margins(
                fixed,
                Edges::uniform(false),
                AvailableSpace::MaxContent,
                40.0,
            ),
            Edges {
                left: 3.0,
                right: 7.0,
                top: 2.0,
                bottom: 4.0,
            }
        );
        assert_eq!(
            resolve_root_margins(
                fixed,
                Edges::uniform(false),
                AvailableSpace::Definite(100.0),
                40.0,
            )
            .left,
            3.0
        );

        let both = resolve_root_margins(
            Edges::uniform(None),
            Edges {
                left: true,
                right: true,
                top: false,
                bottom: false,
            },
            AvailableSpace::Definite(100.0),
            40.0,
        );
        assert_eq!((both.left, both.right), (30.0, 30.0));

        let one = resolve_root_margins(
            Edges {
                left: Some(5.0),
                right: None,
                top: None,
                bottom: None,
            },
            Edges {
                left: false,
                right: true,
                top: false,
                bottom: false,
            },
            AvailableSpace::Definite(100.0),
            40.0,
        );
        assert_eq!((one.left, one.right), (5.0, 55.0));
    }

    fn absolute_style() -> ResolvedAbsoluteStyle {
        ResolvedAbsoluteStyle {
            insets: Edges::uniform(Some(0.0)),
            optional_margin: Edges::uniform(None),
            padding: Edges::ZERO,
            border: Edges::ZERO,
            preferred_available: Size::new(None, None),
            auto_size: Size::new(true, true),
            min_size: Size::new(Some(20.0), Some(10.0)),
            max_size: Size::new(Some(90.0), Some(60.0)),
            aspect_ratio: None,
            direction: direction::T::Ltr,
            padding_border_size: Size::new(8.0, 6.0),
        }
    }

    #[test]
    fn absolute_known_dimensions_clamp_stretch_and_defer_ratio_height() {
        let style = absolute_style();
        assert_eq!(
            absolute_known_dimensions(&style, Size::new(100.0, 80.0), Edges::uniform(5.0),),
            Size::new(Some(90.0), Some(60.0))
        );

        let mut ratio = style;
        ratio.aspect_ratio = Some(2.0);
        assert_eq!(
            absolute_known_dimensions(&ratio, Size::new(100.0, 80.0), Edges::uniform(5.0),),
            Size::new(Some(90.0), None)
        );

        let mut vertical_only = style;
        vertical_only.auto_size.width = false;
        assert_eq!(
            absolute_known_dimensions(&vertical_only, Size::new(100.0, 30.0), Edges::uniform(20.0),),
            Size::new(None, Some(10.0))
        );
    }

    #[test]
    fn absolute_auto_margins_cover_positive_negative_and_one_sided_equations() {
        let insets = Edges::uniform(Some(0.0));
        let centered = resolve_absolute_margins(
            Edges::uniform(None),
            insets,
            Size::new(100.0, 80.0),
            Size::new(60.0, 40.0),
            direction::T::Ltr,
        );
        assert_eq!(centered, Edges::uniform(20.0));

        let ltr_overflow = resolve_absolute_margins(
            Edges::uniform(None),
            insets,
            Size::new(40.0, 80.0),
            Size::new(60.0, 40.0),
            direction::T::Ltr,
        );
        assert_eq!((ltr_overflow.left, ltr_overflow.right), (0.0, -20.0));
        let rtl_overflow = resolve_absolute_margins(
            Edges::uniform(None),
            insets,
            Size::new(40.0, 80.0),
            Size::new(60.0, 40.0),
            direction::T::Rtl,
        );
        assert_eq!((rtl_overflow.left, rtl_overflow.right), (-20.0, 0.0));

        let start_auto = resolve_absolute_margins(
            Edges {
                left: None,
                right: Some(3.0),
                top: None,
                bottom: Some(4.0),
            },
            insets,
            Size::new(100.0, 80.0),
            Size::new(60.0, 40.0),
            direction::T::Ltr,
        );
        assert_eq!((start_auto.left, start_auto.top), (37.0, 36.0));
        let end_auto = resolve_absolute_margins(
            Edges {
                left: Some(2.0),
                right: None,
                top: Some(5.0),
                bottom: None,
            },
            insets,
            Size::new(100.0, 80.0),
            Size::new(60.0, 40.0),
            direction::T::Ltr,
        );
        assert_eq!((end_auto.right, end_auto.bottom), (38.0, 35.0));
    }

    #[test]
    fn absolute_axis_location_covers_static_start_end_and_rtl_preference() {
        let base = AbsoluteAxis {
            containing_size: 100.0,
            box_size: 20.0,
            start_inset: None,
            end_inset: None,
            start_margin: 3.0,
            end_margin: 4.0,
            static_position: 11.0,
            prefer_end: false,
        };
        assert_eq!(absolute_axis_location(base), 14.0);
        assert_eq!(
            absolute_axis_location(AbsoluteAxis {
                start_inset: Some(7.0),
                ..base
            }),
            10.0
        );
        assert_eq!(
            absolute_axis_location(AbsoluteAxis {
                end_inset: Some(9.0),
                ..base
            }),
            67.0
        );
        assert_eq!(
            absolute_axis_location(AbsoluteAxis {
                start_inset: Some(7.0),
                end_inset: Some(9.0),
                prefer_end: true,
                ..base
            }),
            67.0
        );
    }
}
