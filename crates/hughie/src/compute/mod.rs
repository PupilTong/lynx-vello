//! Protocol machinery entry points over a statically split tree and state.
mod anchor;
mod anchor_area;
mod anchor_fallback;
mod flexbox;
mod grid;
mod leaf;
mod linear;
mod relative;
mod single_axis;
mod util;

pub use anchor::{anchor_size_axis, uses_anchor_positioning};
pub use flexbox::compute_flexbox_layout;
pub use grid::{compute_grid_lanes_layout, compute_grid_layout};
#[cfg(feature = "layout-test-utils")]
#[doc(hidden)]
pub use leaf::compute_leaf_layout_with_measurement_for_testing;
pub use leaf::{LeafMeasureInput, LeafMetrics, NaturalSize, compute_leaf_layout};

/// The box-model wrapper a `display: -lynx-text` block lays out through.
///
/// Not a general host-measurer door: leaf content stays closed, and this exists
/// because a text block's content is a paragraph the host owns rather than a
/// natural size it can report. `requires_known_measurement` is `true` and
/// chosen here, where the reason lives — a paragraph must be laid out even when
/// both dimensions are known, because its line breaks are what paint reads.
pub fn compute_text_block_layout<Style, Measure>(
    input: crate::tree::LayoutInput,
    style: &Style,
    measure: Measure,
) -> crate::tree::LayoutOutput
where
    Style: crate::style::CoreStyle,
    Measure: FnMut(LeafMeasureInput) -> LeafMetrics,
{
    leaf::compute_leaf_layout_with_measurement(input, style, None, true, measure)
}

/// The used margins of a box whose placement its host performs itself.
///
/// `auto` is zero here, unlike the root and absolute paths above: a host that
/// places a box against a line rather than against a containing block has no
/// free space to distribute. Negative margins survive — they legitimately
/// shrink the advance a box occupies.
#[must_use]
pub fn used_margins<Style: CoreStyle>(style: &Style, inline_basis: Option<f32>) -> Edges<f32> {
    auto_edges_to_zero(resolve_margins(style.margin(), inline_basis))
}

/// The used padding of a box, percentages resolved against the inline basis —
/// which for every edge, vertical ones included, is the containing block's
/// *width*.
#[must_use]
pub fn used_padding<Style: CoreStyle>(style: &Style, inline_basis: Option<f32>) -> Edges<f32> {
    resolve_padding(style.padding(), inline_basis)
}

/// The used value of an inset on a box its host offsets itself — a sticky
/// box's constraint rectangle, a relative nudge — which is never absolutely
/// positioned: css-anchor-position-1 §5.1.1 resolves no `anchor-size()` for
/// it, so the function takes its fallback, and one without a fallback leaves
/// the inset `auto` (`None`), the initial value an invalid-at-computed-value
/// time declaration computes to.
#[must_use]
pub fn used_inset(value: &Inset, basis: Option<f32>) -> Option<f32> {
    self::util::resolve_inset(value, basis)
}

/// The used border widths, with a `none`/`hidden` side reading zero.
#[must_use]
pub fn used_border<Style: CoreStyle>(style: &Style) -> Edges<f32> {
    resolve_border(&style.border())
}

pub use linear::compute_linear_layout;
pub use relative::compute_relative_layout;
use stylo::computed_values::direction;
use stylo::values::computed::{Inset, Margin, Size as StyleSize};
use stylo::values::specified::align::AlignFlags;

use self::anchor::GeometryValues;
use self::util::{
    apply_box_sizing, auto_edges_to_zero, box_inset_size, clamp, clamp_axis, resolve_border,
    resolve_container_box, resolve_insets, resolve_length_percentage, resolve_margins,
    resolve_max_sizes, resolve_padding, resolve_quantitative_max_sizes, resolve_quantitative_sizes,
    resolve_size, style_size_behaves_auto, used_aspect_ratio,
};
use crate::geometry::{Edges, Point, Rect, Size};
use crate::invalidate::is_relayout_boundary;
use crate::style::CoreStyle;
use crate::style::containment::contain_intrinsic_length;
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
    SizingMode,
};

pub fn compute_root_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    root: T::NodeId,
    available_space: Size<AvailableSpace>,
) {
    let parent_size = available_space.definite_values();
    // The viewport imposes the root's constraints; no document content can
    // move them, which is what lets style-imposed sizing chain down from here.
    let root_input = LayoutInput::commit(
        Size::NONE,
        parent_size,
        available_space,
        Size::new(true, true),
    );
    commit_independent_box(tree, state, root, root_input);
}

/// Commits an atomic inline subtree at the same constraints used to measure it.
/// The paragraph places its outer box afterwards; this writes its own box model
/// and descendant geometry, which a measurement alone cannot restore after hiding.
pub fn compute_inline_box_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> LayoutOutput {
    commit_independent_box(
        tree,
        state,
        node,
        LayoutInput::commit(
            Size::NONE,
            parent_size,
            available_space,
            Size::new(false, false),
        ),
    )
}

fn commit_independent_box<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    root: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput {
    let parent_size = input.parent_size;
    let available_space = input.available_space;
    let output = tree.compute_layout(state, root, input);

    let style = tree.style(root);
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
        tree.set_unrounded_layout(state, root, Layout::default());
        return output;
    }

    let mut layout = Layout::with_order(0);
    layout.location = Point::new(margin.left, margin.top);
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    tree.set_unrounded_layout(state, root, layout);
    output
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

pub fn compute_boundary_relayout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput {
    debug_assert!(
        is_relayout_boundary(&tree.style(node)),
        "compute_boundary_relayout requires a relayout boundary \
         (contain: strict, or a skipped content-visibility box)"
    );
    tree.compute_layout(state, node, input)
}

pub fn compute_cached_layout<T, ComputeFn>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
    compute_uncached: ComputeFn,
) -> LayoutOutput
where
    T: LayoutTree,
    ComputeFn: FnOnce(&T, &mut T::State, T::NodeId, LayoutInput) -> LayoutOutput,
{
    if let Some(output) = tree.layout(state, node).cached_layout(input) {
        return output;
    }

    let output = compute_uncached(tree, state, node, input);
    let slot = tree.layout_mut(state, node);
    slot.store_cached_layout(input, output);
    if input.goal.commits() {
        // A commit that ran is the one thing that rewrites descendant boxes;
        // a measurement pass reads them and writes none. Marking here is what
        // gives the rounding tail its spine: a commit only reaches a node
        // through an unbroken chain of committing ancestors.
        slot.mark_subtree_dirty();
    }
    output
}

pub fn hide_subtree<T: LayoutTree>(tree: &T, state: &mut T::State, node: T::NodeId) {
    hide_subtree_at_order(tree, state, node, 0);
}

/// Hides a `display: none` child's subtree while keeping its paint-order slot.
pub(super) fn hide_child_at_order<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    order: u32,
) {
    hide_subtree_at_order(tree, state, node, order);
}

fn hide_subtree_at_order<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    order: u32,
) {
    if tree.layout(state, node).is_hidden() {
        // The subtree below is already zeroed and stays that way; only the
        // paint-order slot this box keeps among its siblings can still move.
        tree.layout_mut(state, node).set_hidden_order(order);
        return;
    }
    tree.clear_layout_cache(state, node);
    tree.set_unrounded_layout(state, node, Layout::with_order(order));
    tree.layout_mut(state, node).mark_hidden();

    for child in tree.children(node) {
        hide_subtree_at_order(tree, state, child, 0);
    }
}

/// Cleans the stale geometry under a box that skips its contents.
///
/// The **uncacheable half** of a skipped box. It answers to the box tree, not
/// to the layout input: a child inserted, moved or re-styled under the box
/// changes what has to be hidden while changing nothing the box's own size
/// depends on. A host must therefore run it on **every committing call** —
/// outside whatever cache serves [`compute_skipped_contents_size`] — or a
/// subtree re-populated under a box whose size came back from a cache hit
/// would keep geometry no algorithm ever laid out.
///
/// A measurement writes no durable geometry and so hides nothing, which is
/// why the goal is read here rather than left to the caller.
pub fn hide_skipped_contents<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) {
    if !input.goal.commits() {
        return;
    }
    for child in tree.children(node) {
        hide_subtree(tree, state, child);
    }
}

/// The size a box that skips its contents takes: its own styles with
/// `contain-intrinsic-size` substituted for the content it does not lay out
/// ([css-contain-2 §3.5](https://drafts.csswg.org/css-contain-2/#content-visibility)).
///
/// The **cacheable half**, and a pure function of this style and this input —
/// no child is read, so nothing a subtree mutation can change is in it. That
/// is what lets a host serve it through [`compute_cached_layout`] like any
/// algorithm's output, and what makes the box a relayout boundary
/// ([`crate::invalidate::is_relayout_boundary`]). Its counterpart is
/// [`hide_skipped_contents`], which the same host call must run outside that
/// cache.
#[must_use]
pub fn compute_skipped_contents_size<S: CoreStyle>(style: &S, input: LayoutInput) -> LayoutOutput {
    let metrics = resolve_container_box(style, input);
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

    LayoutOutput::new(outer_size, outer_size)
}

#[must_use = "the returned layout is in containing-block space; the host must convert and store it"]
pub fn compute_absolute_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: Size<f32>,
    static_position: Point<f32>,
) -> Layout {
    // A host places this box against a containing block it found itself,
    // whose direction the engine does not see; the box's own `direction`
    // stands in for it (they differ only when the box sets its own).
    let rtl = tree.style(node).direction() == direction::T::Rtl;
    compute_absolute_layout_in(
        tree,
        state,
        node,
        AbsoluteContainingBlock::padding_box(containing_block, rtl),
        move |_, _| static_position,
    )
}

/// Lays out `node`, a box [`LayoutTree::hoisted_children`] reports for
/// `containing_block`, against that box's padding box as its last committed
/// layout has it.
///
/// What a container's own absolute pass does for the hoisted boxes it
/// reaches, for a host that has to place one whose containing block did not
/// run: an in-place relayout of a subtree the box sits in, which its
/// containing block above it never sees. Both read the same padding box, so
/// they agree on the result.
pub fn compute_hoisted_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    containing_block: T::NodeId,
    node: T::NodeId,
) {
    let block = &tree.layout(state, containing_block).unrounded;
    let border = block.border;
    let size = Size::new(
        (block.size.width - border.horizontal_sum()).max(0.0),
        (block.size.height - border.vertical_sum()).max(0.0),
    );
    let rtl = tree.style(containing_block).direction() == direction::T::Rtl;
    lay_out_hoisted(
        tree,
        state,
        containing_block,
        node,
        &AbsoluteContainingBlock::padding_box(size, rtl),
        border,
    );
}

/// Lays out every box [`LayoutTree::hoisted_children`] reports for `node`,
/// whose padding box is `padding_box_size` inside `border`: the absolute
/// pass of a host algorithm (a paragraph) that has no own out-of-flow
/// children to interleave them with.
pub fn compute_hoisted_children<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    padding_box_size: Size<f32>,
    border: Edges<f32>,
    rtl: bool,
) {
    HoistedPass::new(padding_box_size, border, rtl).rest(tree, state, node);
}

/// One hoisted box: its static position moved from its box parent's
/// coordinates into the containing block's, laid out there, and its layout
/// moved back.
fn lay_out_hoisted<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    containing_block_node: T::NodeId,
    node: T::NodeId,
    containing_block: &AbsoluteContainingBlock,
    border: Edges<f32>,
) {
    let offset = tree.hoisted_parent_offset(state, containing_block_node, node);
    let static_position = tree.layout(state, node).static_position;
    let static_in_padding = Point::new(
        offset.x + static_position.x - border.left,
        offset.y + static_position.y - border.top,
    );
    let mut layout =
        compute_absolute_layout_in(tree, state, node, *containing_block, move |_, _| {
            static_in_padding
        });
    layout.location = Point::new(
        layout.location.x + border.left - offset.x,
        layout.location.y + border.top - offset.y,
    );
    tree.set_hoisted_layout(state, containing_block_node, node, layout);
}

/// The hoisted half of a container's absolute pass: the boxes
/// [`LayoutTree::hoisted_children`] reports, laid out in flat tree order
/// among the container's own out-of-flow children.
///
/// Every hoisted box is placed against the container's padding box —
/// css-position-3's containing block, with the container's direction and the
/// author's self-alignment — whatever the container's algorithm does for its
/// own children (a grid area, Lynx's inset-only `linear` and `relative`):
/// those are rules about a container's children, and a hoisted box is not
/// one.
pub(super) struct HoistedPass {
    /// Every hoisted box whose `via` is below this has been laid out.
    next: usize,
    containing_block: AbsoluteContainingBlock,
    border: Edges<f32>,
}

impl HoistedPass {
    #[inline]
    pub(super) const fn new(padding_box_size: Size<f32>, border: Edges<f32>, rtl: bool) -> Self {
        Self {
            next: 0,
            containing_block: AbsoluteContainingBlock::padding_box(padding_box_size, rtl),
            border,
        }
    }

    /// Lays out the hoisted boxes that come before the container's own
    /// out-of-flow child at flattened index `index` in tree order.
    #[inline]
    pub(super) fn before<T: LayoutTree>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        node: T::NodeId,
        index: usize,
    ) {
        if index > self.next {
            self.lay_out(tree, state, node, self.next, index);
            self.next = index;
        }
    }

    /// Lays out the hoisted boxes inside the container's own out-of-flow
    /// child at flattened index `index`, which was just laid out: they come
    /// after it in tree order, and its own layout may be what registered
    /// them.
    #[inline]
    pub(super) fn inside<T: LayoutTree>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        node: T::NodeId,
        index: usize,
    ) {
        self.lay_out(tree, state, node, index, index + 1);
        self.next = index + 1;
    }

    /// Lays out every hoisted box left, after the container's last own
    /// out-of-flow child.
    #[inline]
    pub(super) fn rest<T: LayoutTree>(&mut self, tree: &T, state: &mut T::State, node: T::NodeId) {
        self.lay_out(tree, state, node, self.next, usize::MAX);
        self.next = usize::MAX;
    }

    /// The hoisted boxes whose `via` is in `from..to`, in order. The common
    /// answer — none — costs one host call and stays inline; the rest is
    /// out of line.
    #[inline]
    fn lay_out<T: LayoutTree>(
        &self,
        tree: &T,
        state: &mut T::State,
        node: T::NodeId,
        from: usize,
        to: usize,
    ) {
        if tree.hoisted_children(state, node).is_empty() {
            return;
        }
        self.lay_out_listed(tree, state, node, from, to);
    }

    /// The host is asked again after every box: laying one out can register
    /// another inside it, which comes right after it in tree order.
    #[cold]
    #[inline(never)]
    fn lay_out_listed<T: LayoutTree>(
        &self,
        tree: &T,
        state: &mut T::State,
        node: T::NodeId,
        from: usize,
        to: usize,
    ) {
        let mut done = 0;
        loop {
            let hoisted = tree.hoisted_children(state, node);
            let Some(child) = hoisted
                .iter()
                .filter(|child| (from..to).contains(&child.via))
                .nth(done)
            else {
                return;
            };
            lay_out_hoisted(
                tree,
                state,
                node,
                child.node,
                &self.containing_block,
                self.border,
            );
            done += 1;
        }
    }
}

/// The containing block of an absolutely positioned box, as the absolute pass
/// of the box that generates it knows it.
#[derive(Debug, Clone, Copy)]
pub(super) struct AbsoluteContainingBlock {
    /// Its origin in its generator's padding-box coordinates: a grid area's
    /// offset, zero for every other containing block.
    pub(super) origin: Point<f32>,
    pub(super) size: Size<f32>,
    /// Its generator's padding-box size. css-position-3 §2.1.1: "The
    /// element's original containing block is its containing block before
    /// applying any of these effects" — grid placement included.
    pub(super) padding_box_size: Size<f32>,
    /// Whether it is its generator's whole padding box, the one
    /// css-position-4's scrollable containing block replaces.
    pub(super) is_padding_box: bool,
    /// Its `direction`: what `start`/`end` mean in the horizontal axis.
    pub(super) rtl: bool,
    /// Whether the author's `justify-self`/`align-self` place the box
    /// (css-position-3 §4.3). Lynx's own `linear` and `relative` containers
    /// place an absolutely positioned child by its insets alone, and those
    /// algorithms are not extended; `anchor-center` and `position-area`'s
    /// defaults, which are opt-in anchor positioning, still apply there.
    pub(super) honors_self_alignment: bool,
}

impl AbsoluteContainingBlock {
    #[inline]
    pub(super) const fn padding_box(size: Size<f32>, rtl: bool) -> Self {
        Self {
            origin: Point::ZERO,
            size,
            padding_box_size: size,
            is_padding_box: true,
            rtl,
            honors_self_alignment: true,
        }
    }

    /// The original containing block in this one's coordinates.
    #[inline]
    pub(super) fn original(&self) -> Rect<f32> {
        Rect::new(
            Point::new(-self.origin.x, -self.origin.y),
            self.padding_box_size,
        )
    }

    /// The padding box of a Lynx `linear` or `relative` container.
    #[inline]
    pub(super) const fn lynx_padding_box(size: Size<f32>, rtl: bool) -> Self {
        Self {
            honors_self_alignment: false,
            ..Self::padding_box(size, rtl)
        }
    }
}

pub(super) fn compute_absolute_layout_in<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: AbsoluteContainingBlock,
    static_position: StaticPosition,
) -> Layout
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    absolute_layout(
        tree,
        state,
        node,
        &containing_block,
        &static_position,
        // Every field of the input `absolute_layout` builds for this box is a
        // function of the containing block, the box's own style and, when
        // that style uses anchor positioning, its anchors' geometry — none of
        // which the box's own subtree can move. The exceptions are an
        // anchored axis sized from content, which is measured to build the
        // input (`settle_anchored_axes` withdraws the claim on that axis),
        // and a box with position options, whose choice among them depends on
        // its own size (`anchor_fallback` withdraws both). And the box's
        // content cannot move the containing block back: it contributes to no
        // ancestor's used size, only to their scrollable overflow, which is an
        // output the in-place path compares before it trusts anything.
        LayoutGoal::Commit {
            content_independent: Size::new(true, true),
        },
    )
}

#[must_use]
pub(super) fn measure_absolute_layout<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    containing_block: AbsoluteContainingBlock,
    requested_axis: RequestedAxis,
) -> Layout {
    absolute_layout(
        tree,
        state,
        node,
        &containing_block,
        &|_, _| Point::ZERO,
        LayoutGoal::Measure(requested_axis),
    )
}

fn absolute_layout<T, StaticPosition>(
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
    let size = containing_block.size;
    debug_assert!(
        size.width.is_finite()
            && size.height.is_finite()
            && size.width >= 0.0
            && size.height >= 0.0,
        "containing-block sizes must be finite and non-negative"
    );
    let style = tree.style(node);
    if tree.position_option_count(node) > 1 || anchor::uses_anchor_positioning(&style) {
        return anchor_fallback::anchored_absolute_layout(
            tree,
            state,
            node,
            containing_block,
            static_position,
            goal,
        );
    }
    let values = GeometryValues::of(&style);
    let placement = AbsolutePlacement::plain(&style, &values, containing_block);
    place_absolute(
        tree,
        state,
        node,
        &style,
        &values,
        &placement,
        containing_block,
        static_position,
        goal,
    )
    .layout
}

/// The used self-alignment of an absolutely positioned box on one axis.
#[derive(Debug, Clone, Copy)]
pub(super) struct AxisAlignment {
    /// The value with its overflow-position bits. `auto` is stored as
    /// `normal`: css-align-3 §6.1 — "Behaves as normal … when determining the
    /// actual position of an absolutely positioned box."
    pub(super) flags: AlignFlags,
    /// `anchor-center` with a default anchor: where the margin box's center
    /// goes, in the coordinates of the containing block it is laid out in.
    pub(super) anchor_center: Option<f32>,
}

impl AxisAlignment {
    pub(super) const NORMAL: Self = Self {
        flags: AlignFlags::NORMAL,
        anchor_center: None,
    };

    #[inline]
    pub(super) fn of(flags: AlignFlags) -> Self {
        Self {
            flags: if flags.value() == AlignFlags::AUTO {
                AlignFlags::NORMAL
            } else {
                flags
            },
            anchor_center: None,
        }
    }

    #[inline]
    pub(super) fn is_normal(self) -> bool {
        self.flags.value() == AlignFlags::NORMAL
    }

    /// css-position-3 §4.1: an automatic size is the stretch-fit size only
    /// under `stretch`, or `normal`, and both insets non-`auto`; css-align-3
    /// §6.1.2: "Values other than stretch or normal cause non-replaced
    /// absolutely-positioned boxes to use fit-content sizing".
    #[inline]
    fn stretches(self) -> bool {
        matches!(self.flags.value(), AlignFlags::NORMAL | AlignFlags::STRETCH)
    }
}

/// Where and against what an absolutely positioned box is laid out.
#[derive(Debug, Clone, Copy)]
pub(super) struct AbsolutePlacement {
    /// The containing block the box is sized and positioned in, in the
    /// coordinates of the one its generator's absolute pass handed over: that
    /// one, or a `position-area` region.
    pub(super) area: Rect<f32>,
    /// The original containing block (css-position-3 §2.1.1: the generator's
    /// padding box, or its scrollable containing block), for css-align-3
    /// §4.4.1.2's overflow limit rect.
    pub(super) original: Rect<f32>,
    /// `justify-self` (horizontal) and `align-self` (vertical), used values.
    pub(super) align: Size<AxisAlignment>,
    /// The insets whose computed value is `auto`, even where `position-area`
    /// or `anchor-center` made their used value 0.
    pub(super) auto_inset: Edges<bool>,
    /// Axes the box's own run would read differently from the values it is
    /// laid out with, which the pass hands it as known dimensions.
    pub(super) sensitive: Size<bool>,
}

impl AbsolutePlacement {
    /// The placement of a box that uses no anchor positioning.
    #[inline]
    fn plain(
        style: &impl CoreStyle,
        values: &GeometryValues<'_>,
        containing_block: &AbsoluteContainingBlock,
    ) -> Self {
        Self {
            area: Rect::new(Point::ZERO, containing_block.size),
            original: containing_block.original(),
            align: if containing_block.honors_self_alignment {
                Size::new(
                    AxisAlignment::of(style.justify_self().0),
                    AxisAlignment::of(style.align_self().0),
                )
            } else {
                Size::new(AxisAlignment::NORMAL, AxisAlignment::NORMAL)
            },
            auto_inset: values.inset.map(|inset| matches!(inset, Inset::Auto)),
            sensitive: Size::new(false, false),
        }
    }
}

/// One absolute layout of a box, with what the position fallback test reads.
pub(super) struct Placed {
    pub(super) layout: Layout,
    /// The inset-modified containing block, in the handed-over containing
    /// block's coordinates.
    pub(super) imcb: Rect<f32>,
    /// The margin box, in the same coordinates.
    pub(super) margin_box: Rect<f32>,
    /// css-anchor-position-1 §6.5: "If cb rect was negative-size in either
    /// axis and corrected into zero-size".
    pub(super) negative_corrected: bool,
}

impl Placed {
    /// §6.5: the margin box is "fully contained within" the inset-modified
    /// containing block, which was not negative-size.
    #[inline]
    pub(super) fn fits(&self) -> bool {
        // A layout unit of slack (1/64 px) keeps an exact fit a fit.
        !self.negative_corrected && self.imcb.contains_rect(&self.margin_box, 1.0 / 64.0)
    }
}

/// css-position-3 §4's absolute positioning layout model for one box:
/// inset-modified containing block, size, auto margins, then alignment of the
/// margin box — in `placement.area`, with `values` in place of the box's own
/// geometry values.
#[allow(
    clippy::too_many_arguments,
    reason = "the model's inputs; bundling them would only rename them"
)]
#[allow(
    clippy::too_many_lines,
    reason = "the four steps of the model in their order"
)]
#[inline]
fn place_absolute<T, StaticPosition>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    style: &impl CoreStyle,
    values: &GeometryValues<'_>,
    placement: &AbsolutePlacement,
    containing_block: &AbsoluteContainingBlock,
    static_position: &StaticPosition,
    goal: LayoutGoal,
) -> Placed
where
    T: LayoutTree,
    StaticPosition: Fn(Size<f32>, Edges<f32>) -> Point<f32>,
{
    let area = placement.area;
    let containing = area.size;
    let parent_size = Size::new(Some(containing.width), Some(containing.height));
    let resolved_style = resolve_absolute_style(style, values, parent_size);
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
        (containing.width - insets.left.unwrap_or(0.0) - insets.right.unwrap_or(0.0)).max(0.0),
        (containing.height - insets.top.unwrap_or(0.0) - insets.bottom.unwrap_or(0.0)).max(0.0),
    );

    let mut known_dimensions = absolute_known_dimensions(
        &resolved_style,
        inset_modified_size,
        fixed_margin,
        Size::new(
            placement.align.width.stretches(),
            placement.align.height.stretches(),
        ),
    );
    let available_space = Size::new(
        preferred_available
            .width
            .unwrap_or(AvailableSpace::Definite(inset_modified_size.width)),
        preferred_available
            .height
            .unwrap_or(AvailableSpace::Definite(inset_modified_size.height)),
    );
    let goal = settle_anchored_axes(
        tree,
        state,
        node,
        style,
        values,
        placement.sensitive,
        AbsoluteSpace {
            parent_size,
            inset_modified_size,
            fixed_margin,
        },
        &mut known_dimensions,
        goal,
    );
    let child_input = LayoutInput::new(goal, known_dimensions, parent_size, available_space);
    let output = tree.compute_layout(state, node, child_input);

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
    let static_position = Point::new(
        static_position.x - area.origin.x,
        static_position.y - area.origin.y,
    );
    let self_rtl = direction == direction::T::Rtl;
    let horizontal_imcb = inset_modified_axis(
        containing.width,
        (insets.left, insets.right),
        weaker_is_start(
            (placement.auto_inset.left, placement.auto_inset.right),
            containing_block.rtl,
        ),
        static_position.x,
    );
    let vertical_imcb = inset_modified_axis(
        containing.height,
        (insets.top, insets.bottom),
        weaker_is_start(
            (placement.auto_inset.top, placement.auto_inset.bottom),
            false,
        ),
        static_position.y,
    );
    let original = placement.original;
    let aligned = |alignment: AxisAlignment,
                   start_margin: Option<f32>,
                   end_margin: Option<f32>,
                   imcb: InsetModifiedAxis,
                   limit: (f32, f32),
                   horizontal: bool| {
        (!alignment.is_normal() && start_margin.is_some() && end_margin.is_some()).then_some(
            AlignedAxis {
                imcb_start: imcb.start,
                imcb_size: imcb.size,
                alignment,
                limit,
                containing_rtl: horizontal && containing_block.rtl,
                self_rtl: horizontal && self_rtl,
                horizontal,
            },
        )
    };
    let location = Point::new(
        absolute_axis_location(AbsoluteAxis {
            containing_size: containing.width,
            box_size: output.size.width,
            start_inset: insets.left,
            end_inset: insets.right,
            start_margin: margin.left,
            end_margin: margin.right,
            static_position: static_position.x,
            prefer_end: self_rtl,
            aligned: aligned(
                placement.align.width,
                optional_margin.left,
                optional_margin.right,
                horizontal_imcb,
                (
                    original.origin.x - area.origin.x,
                    original.origin.x + original.size.width - area.origin.x,
                ),
                true,
            ),
        }),
        absolute_axis_location(AbsoluteAxis {
            containing_size: containing.height,
            box_size: output.size.height,
            start_inset: insets.top,
            end_inset: insets.bottom,
            start_margin: margin.top,
            end_margin: margin.bottom,
            static_position: static_position.y,
            prefer_end: false,
            aligned: aligned(
                placement.align.height,
                optional_margin.top,
                optional_margin.bottom,
                vertical_imcb,
                (
                    original.origin.y - area.origin.y,
                    original.origin.y + original.size.height - area.origin.y,
                ),
                false,
            ),
        }),
    );

    let mut layout = Layout::with_order(0);
    layout.location = Point::new(area.origin.x + location.x, area.origin.y + location.y);
    layout.size = output.size;
    layout.content_size = output.content_size;
    layout.border = border;
    layout.padding = padding;
    layout.margin = margin;
    Placed {
        imcb: Rect::new(
            Point::new(
                area.origin.x + horizontal_imcb.start,
                area.origin.y + vertical_imcb.start,
            ),
            Size::new(horizontal_imcb.size, vertical_imcb.size),
        ),
        margin_box: Rect::new(
            Point::new(
                layout.location.x - margin.left,
                layout.location.y - margin.top,
            ),
            Size::new(
                output.size.width + margin.horizontal_sum(),
                output.size.height + margin.vertical_sum(),
            ),
        ),
        negative_corrected: horizontal_imcb.negative_corrected || vertical_imcb.negative_corrected,
        layout,
    }
}

/// One axis of the inset-modified containing block, in the coordinates of
/// the containing block it is cut from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct InsetModifiedAxis {
    start: f32,
    size: f32,
    negative_corrected: bool,
}

/// css-position-3 §3.5.2: "the weaker inset in the affected axis is reduced
/// … In the case that only one inset is auto, that is the weaker inset …;
/// otherwise the weaker inset is the inset of the end edge (where end is
/// interpreted relative to the writing mode of the containing block)."
/// Whether that is the start (left or top) inset, from the computed-`auto`
/// flags of the start and end insets.
#[inline]
fn weaker_is_start(auto_insets: (bool, bool), rtl: bool) -> bool {
    match auto_insets {
        (true, false) => true,
        (false, true) => false,
        _ => rtl,
    }
}

/// The inset-modified containing block on one axis (css-position-3 §3.5):
/// the containing block reduced by the insets, an `auto` one being 0 when
/// the other is not (§3.5.1: "If only one inset property in a given axis is
/// auto, it is set to zero") and both `auto` standing at the static
/// position (§3.5.1: "Set its start-edge inset property to the static
/// position, and its end-edge inset property to zero"). A negative size is
/// brought up to zero by moving the weaker edge (§3.5.2).
#[inline]
fn inset_modified_axis(
    containing: f32,
    (start, end): (Option<f32>, Option<f32>),
    weaker_is_start: bool,
    static_position: f32,
) -> InsetModifiedAxis {
    let (start, end) = match (start, end) {
        (None, None) => (static_position, 0.0),
        (start, end) => (start.unwrap_or(0.0), end.unwrap_or(0.0)),
    };
    let size = containing - start - end;
    if size >= 0.0 {
        InsetModifiedAxis {
            start,
            size,
            negative_corrected: false,
        }
    } else {
        InsetModifiedAxis {
            start: if weaker_is_start {
                containing - end
            } else {
                start
            },
            size: 0.0,
            negative_corrected: true,
        }
    }
}

/// Resolved box-model and positioning inputs retained across the recursive
/// child-layout call for one absolutely positioned node.
#[derive(Clone, Copy)]
struct ResolvedAbsoluteStyle {
    insets: Edges<Option<f32>>,
    optional_margin: Edges<Option<f32>>,
    padding: Edges<f32>,
    border: Edges<f32>,
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
    stretch_alignment: Size<bool>,
) -> Size<Option<f32>> {
    let horizontal_stretch = style.auto_size.width
        && stretch_alignment.width
        && style.insets.left.is_some()
        && style.insets.right.is_some();
    let ratio_dependent_height =
        style.aspect_ratio.is_some() && horizontal_stretch && style.auto_size.height;
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
            && stretch_alignment.height
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

/// Settles, for an absolutely positioned box laid out with values its own
/// run would read differently — substituted anchor functions, a fallback
/// option's values — the used border-box size on every such `sensitive`
/// axis, and records it as that axis's known dimension (see
/// [`anchor::AnchoredGeometry`] for why that is the whole job). Any other box
/// passes through.
///
/// An axis with a definite preferred size takes it clamped by the min/max
/// sizes, exactly the preferred size the box's own run derives from these
/// values (`resolve_container_box`). An axis sized from content — `auto`, an
/// intrinsic sizing keyword, or a size the substitution made `auto` — with an
/// anchor-bearing min/max size or margin is measured with the box's size
/// styles ignored, so none of its unresolvable reads enter the result, in the
/// space its keyword asks for ([`content_available_space`]), and then clamped
/// here; a height is measured at the used width. Returns the goal with content
/// independence withdrawn on the axes it measured.
#[allow(
    clippy::too_many_arguments,
    reason = "one step of absolute_layout, consuming its locals"
)]
fn settle_anchored_axes<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    style: &impl CoreStyle,
    values: &GeometryValues<'_>,
    sensitive: Size<bool>,
    space: AbsoluteSpace,
    known: &mut Size<Option<f32>>,
    mut goal: LayoutGoal,
) -> LayoutGoal {
    if !sensitive.width && !sensitive.height {
        return goal;
    }
    let parent_size = space.parent_size;
    let aspect_ratio = used_aspect_ratio(style.aspect_ratio());
    let box_sizing = style.box_sizing();
    let box_inset = box_inset_size(
        resolve_padding(style.padding(), parent_size.width),
        resolve_border(&style.border()),
    );
    let quantitative =
        |value| resolve_quantitative_sizes(value, parent_size, aspect_ratio, box_sizing, box_inset);
    let preferred = quantitative(values.size);
    let min = quantitative(values.min_size);
    let max = resolve_quantitative_max_sizes(
        values.max_size,
        parent_size,
        aspect_ratio,
        box_sizing,
        box_inset,
    );
    let clamp_width = |width| clamp_axis(width, min.width, max.width, box_inset.width);
    let clamp_height = |height| clamp_axis(height, min.height, max.height, box_inset.height);
    if sensitive.width && known.width.is_none() {
        known.width = preferred.width.map(clamp_width);
    }
    if sensitive.height && known.height.is_none() {
        known.height = preferred.height.map(clamp_height);
    }

    let mut measured = Size::new(false, false);
    let content_space = content_available_space(style, values, space);
    if sensitive.width && known.width.is_none() {
        let input = content_measure(
            Size::new(None, known.height),
            parent_size,
            content_space,
            RequestedAxis::Horizontal,
        );
        let width = tree.compute_layout(state, node, input).size.width;
        known.width = Some(clamp_width(width));
        measured.width = true;
    }
    if sensitive.height && known.height.is_none() {
        // The height of content depends on the width it is laid out at, which
        // the measurement below would otherwise take from the very size
        // styles it ignores.
        let width = known.width.unwrap_or_else(|| {
            let input = LayoutInput::measure(
                Size::NONE,
                parent_size,
                Size::new(
                    AvailableSpace::Definite(space.inset_modified_size.width),
                    AvailableSpace::MaxContent,
                ),
                RequestedAxis::Horizontal,
            );
            tree.compute_layout(state, node, input).size.width
        });
        let input = content_measure(
            Size::new(Some(width), None),
            parent_size,
            content_space,
            RequestedAxis::Vertical,
        );
        let height = tree.compute_layout(state, node, input).size.height;
        known.height = Some(clamp_height(height));
        measured.height = true;
    }
    if let LayoutGoal::Commit {
        content_independent,
    } = &mut goal
    {
        // A size measured from the box's own content moves with it.
        content_independent.width &= !measured.width;
        content_independent.height &= !measured.height;
    }
    goal
}

/// The containing-block geometry `absolute_layout` has resolved for one box.
#[derive(Clone, Copy)]
struct AbsoluteSpace {
    parent_size: Size<Option<f32>>,
    inset_modified_size: Size<f32>,
    fixed_margin: Edges<f32>,
}

/// A measurement of the box's content alone: its own size, min and max sizes
/// ignored.
fn content_measure(
    known: Size<Option<f32>>,
    parent_size: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    axis: RequestedAxis,
) -> LayoutInput {
    let mut input = LayoutInput::measure(known, parent_size, available, axis);
    input.sizing_mode = SizingMode::IgnoreSizeStyles;
    input
}

/// The space a content-sized anchored axis is measured in: the same space the
/// box's own run gets when nothing is anchored — its intrinsic sizing keyword
/// (`min-content`, `max-content`, `fit-content(<limit>)`), else the
/// inset-modified containing block — so the keyword, not a formula of its
/// own, decides the size.
///
/// The measurement ignores size styles but not margins: the run still
/// subtracts the margins it reads from a definite space, and on an anchored
/// axis it reads their unresolvable form. The space is widened by exactly
/// that and narrowed by the substituted margins, so the run ends up
/// subtracting the margins that apply.
fn content_available_space(
    style: &impl CoreStyle,
    values: &GeometryValues<'_>,
    space: AbsoluteSpace,
) -> Size<AvailableSpace> {
    let unresolved = auto_edges_to_zero(resolve_margins(style.margin(), space.parent_size.width));
    let axis =
        |size: &StyleSize, basis: Option<f32>, inset_modified: f32, unresolved: f32, used: f32| {
            match absolute_preferred_available(size, basis)
                .unwrap_or(AvailableSpace::Definite(inset_modified))
            {
                AvailableSpace::Definite(space) => {
                    AvailableSpace::Definite((space + unresolved - used).max(0.0))
                }
                intrinsic => intrinsic,
            }
        };
    Size::new(
        axis(
            values.size.width,
            space.parent_size.width,
            space.inset_modified_size.width,
            unresolved.horizontal_sum(),
            space.fixed_margin.horizontal_sum(),
        ),
        axis(
            values.size.height,
            space.parent_size.height,
            space.inset_modified_size.height,
            unresolved.vertical_sum(),
            space.fixed_margin.vertical_sum(),
        ),
    )
}

fn resolve_absolute_style(
    style: &impl CoreStyle,
    values: &GeometryValues<'_>,
    parent_size: Size<Option<f32>>,
) -> ResolvedAbsoluteStyle {
    let padding = resolve_padding(style.padding(), parent_size.width);
    let border = resolve_border(&style.border());
    let padding_border_size = Size::new(
        padding.horizontal_sum() + border.horizontal_sum(),
        padding.vertical_sum() + border.vertical_sum(),
    );
    let style_size = values.size;
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
        resolve_size(values.min_size, parent_size),
        style.box_sizing(),
        padding_border_size,
    );
    let max_size = apply_box_sizing(
        resolve_max_sizes(values.max_size, parent_size),
        style.box_sizing(),
        padding_border_size,
    );

    ResolvedAbsoluteStyle {
        insets: resolve_insets(values.inset, parent_size),
        optional_margin: resolve_margins(values.margin, parent_size.width),
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
        // `absolute_layout` hands over substituted values whenever the style
        // references `anchor-size()`; a function still here resolves as
        // §5.1.1's unresolvable form, which is never an intrinsic keyword.
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unresolvable_preferred_available(value, basis)
        }
    }
}

/// The anchor arm of [`absolute_preferred_available`], out of line so the hot
/// function does not call itself.
#[cold]
#[inline(never)]
fn unresolvable_preferred_available(
    value: &StyleSize,
    basis: Option<f32>,
) -> Option<AvailableSpace> {
    absolute_preferred_available(&self::anchor::unresolvable_style_size(value), basis)
}

pub fn round_layout<T: LayoutTree>(tree: &T, state: &mut T::State, root: T::NodeId, scale: f32) {
    round_layout_subtree(tree, state, root, scale, Point::ZERO);
}

pub fn round_layout_subtree<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    scale: f32,
    parent_position: Point<f32>,
) {
    round_layout_subtree_with(
        tree,
        state,
        node,
        scale,
        parent_position,
        true,
        |_, _, _| false,
    );
}

/// Rounds a subtree after a statically dispatched preorder hook.
/// Returning `false` prunes only later hook calls; rounding still visits descendants.
/// The hook runs before the current unrounded layout is cloned.
///
/// `rescale` says the rounding function itself changed under the caller — a new
/// device scale, or a viewport that moves boxes no layout write touched — and
/// forces the whole subtree. Otherwise the walk descends only where a box was
/// written since the last rounding: a changed node carries its whole subtree
/// with it, because everything under it shifts with its origin and anything
/// positioned against it resolves against its new size.
#[doc(hidden)]
pub fn round_layout_subtree_with<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    scale: f32,
    parent_position: Point<f32>,
    rescale: bool,
    mut pre_node: impl FnMut(&T, &mut T::State, T::NodeId) -> bool,
) {
    debug_assert!(
        scale.is_finite() && scale > 0.0,
        "scale must be positive and finite"
    );
    debug_assert!(
        parent_position.x.is_finite() && parent_position.y.is_finite(),
        "accumulated parent position must be finite"
    );
    round_layout_inner(
        tree,
        state,
        node,
        scale,
        parent_position,
        &mut pre_node,
        true,
        rescale,
    );
}

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
        aligned,
    } = axis;
    match (start_inset, end_inset, aligned) {
        (None, None, _) => static_position + start_margin,
        (Some(_), Some(_), Some(aligned)) => {
            aligned_margin_box_start(aligned, box_size + start_margin + end_margin) + start_margin
        }
        (Some(_), Some(end), None) if prefer_end => containing_size - end - box_size - end_margin,
        (Some(start), _, _) => start + start_margin,
        (None, Some(end), _) => containing_size - end - box_size - end_margin,
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
    /// Self-alignment other than `normal` with both insets and both margins
    /// non-`auto`: css-position-3 §4.3's last case. `None` keeps the
    /// equation above — the stronger inset, or the margins' own solution.
    aligned: Option<AlignedAxis>,
}

/// css-position-3 §4.3: "the box is aligned as specified by its
/// self-alignment property in the relevant axis (as defined by the writing
/// mode of the containing block), using its margin box as the alignment
/// subject and the inset-modified containing block as the alignment
/// container."
#[derive(Debug, Clone, Copy)]
struct AlignedAxis {
    imcb_start: f32,
    imcb_size: f32,
    alignment: AxisAlignment,
    /// css-align-3 §4.4.1.2's overflow limit rect on this axis: "the
    /// bounding rectangle of the alignment subject's inset-modified
    /// containing block and its original containing block", before the
    /// bounding with the inset-modified containing block.
    limit: (f32, f32),
    /// The containing block's inline direction runs right to left on this
    /// axis.
    containing_rtl: bool,
    /// The box's own inline direction runs right to left on this axis.
    self_rtl: bool,
    horizontal: bool,
}

/// Where the margin box of size `subject` starts (its left or top edge) under
/// `axis`'s alignment.
fn aligned_margin_box_start(axis: AlignedAxis, subject: f32) -> f32 {
    let AlignedAxis {
        imcb_start,
        imcb_size,
        alignment,
        limit,
        containing_rtl,
        self_rtl,
        horizontal,
    } = axis;
    let free = imcb_size - subject;
    let toward = |at_end: bool| imcb_start + if at_end { free } else { 0.0 };
    let desired = match alignment.anchor_center {
        // css-anchor-position-1 §4.2: "it is centered (insofar as possible)
        // over the default anchor box in the relevant axis".
        Some(center) => center - subject / 2.0,
        None => match alignment.flags.value() {
            AlignFlags::CENTER | AlignFlags::ANCHOR_CENTER => imcb_start + free / 2.0,
            AlignFlags::END | AlignFlags::FLEX_END => toward(!containing_rtl),
            AlignFlags::LEFT if horizontal => toward(false),
            AlignFlags::RIGHT if horizontal => toward(true),
            // css-align-3 §4.2: a baseline's fallback alignment is "safe
            // self-start" (first) or "safe self-end" (last); there is no
            // baseline to share with an absolutely positioned box.
            AlignFlags::SELF_START | AlignFlags::BASELINE => toward(self_rtl),
            AlignFlags::SELF_END | AlignFlags::LAST_BASELINE => toward(!self_rtl),
            // `start`, `flex-start`, a `stretch` that could not stretch
            // (css-align-3 §6.1: it "falls back to flex-start"), and `left`
            // or `right` in `align-self`, where they are not values.
            _ => toward(containing_rtl),
        },
    };
    if alignment.flags.flags().contains(AlignFlags::UNSAFE) {
        // css-align-3 §4.4: "Regardless of the relative sizes of the
        // alignment subject and alignment container, the given alignment
        // value is honored."
        return desired;
    }
    // css-align-3 §4.4.1.2, which both `safe` and the default follow for an
    // absolutely positioned box whose alignment is not `normal`.
    let imcb_end = imcb_start + imcb_size;
    // "If the alignment subject fits within the inset-modified containing
    // block, align as specified to the extent possible without overflowing
    // the inset-modified containing block."
    if subject <= imcb_size {
        return desired.clamp(imcb_start, imcb_end - subject);
    }
    let (limit_start, limit_end) = (limit.0.min(imcb_start), limit.1.max(imcb_end));
    // "Otherwise, if the alignment subject fits within the overflow limit
    // rect, align the alignment subject such that it fully covers the
    // inset-modified containing block and is otherwise aligned as specified
    // to the extent possible without overflowing the overflow limit rect."
    if subject <= limit_end - limit_start {
        return desired
            .clamp(imcb_end - subject, imcb_start)
            .clamp(limit_start, limit_end - subject);
    }
    // "Otherwise, start-align the alignment subject within the overflow limit
    // rect."
    if containing_rtl {
        limit_end - subject
    } else {
        limit_start
    }
}

fn rounded_layout(
    source: &Layout,
    scale: f32,
    parent_position: Point<f32>,
) -> (Layout, Point<f32>) {
    let position = Point::new(
        parent_position.x + source.location.x,
        parent_position.y + source.location.y,
    );
    let mut snap = |value: f32| {
        #[cfg(test)]
        tests::ROUND_SNAP_CALLS.with(|calls| calls.set(calls.get() + 1));
        css_round_to_integer(value * scale) / scale
    };
    macro_rules! snap_point {
        ($x:expr, $y:expr) => {
            Point::new($x, $y).map(&mut snap)
        };
    }
    let snapped_parent_position = parent_position.map(&mut snap);
    let snapped_position = position.map(&mut snap);
    let snapped_box_end = snap_point!(
        position.x + source.size.width,
        position.y + source.size.height
    );
    let snapped_content_end = snap_point!(
        position.x + source.content_size.width,
        position.y + source.content_size.height
    );
    let snapped_border_start = snap_point!(
        position.x + source.border.left,
        position.y + source.border.top
    );
    let snapped_border_end = snap_point!(
        position.x + source.size.width - source.border.right,
        position.y + source.size.height - source.border.bottom
    );
    let snapped_padding_start = snap_point!(
        position.x + source.border.left + source.padding.left,
        position.y + source.border.top + source.padding.top
    );
    let snapped_padding_end = snap_point!(
        position.x + source.size.width - source.border.right - source.padding.right,
        position.y + source.size.height - source.border.bottom - source.padding.bottom
    );
    let snapped_margin_start = snap_point!(
        position.x - source.margin.left,
        position.y - source.margin.top
    );
    let snapped_margin_end = snap_point!(
        position.x + source.size.width + source.margin.right,
        position.y + source.size.height + source.margin.bottom
    );
    let mut rounded = Layout::with_order(source.order);
    rounded.location = Point::new(
        snapped_position.x - snapped_parent_position.x,
        snapped_position.y - snapped_parent_position.y,
    );
    rounded.size = Size::new(
        snapped_box_end.x - snapped_position.x,
        snapped_box_end.y - snapped_position.y,
    );
    rounded.content_size = Size::new(
        snapped_content_end.x - snapped_position.x,
        snapped_content_end.y - snapped_position.y,
    );
    rounded.border.left = snapped_border_start.x - snapped_position.x;
    rounded.border.right = snapped_box_end.x - snapped_border_end.x;
    rounded.border.top = snapped_border_start.y - snapped_position.y;
    rounded.border.bottom = snapped_box_end.y - snapped_border_end.y;
    rounded.padding.left = snapped_padding_start.x - snapped_border_start.x;
    rounded.padding.right = snapped_border_end.x - snapped_padding_end.x;
    rounded.padding.top = snapped_padding_start.y - snapped_border_start.y;
    rounded.padding.bottom = snapped_border_end.y - snapped_padding_end.y;
    rounded.margin.left = snapped_position.x - snapped_margin_start.x;
    rounded.margin.right = snapped_margin_end.x - snapped_box_end.x;
    rounded.margin.top = snapped_position.y - snapped_margin_start.y;
    rounded.margin.bottom = snapped_margin_end.y - snapped_box_end.y;

    (rounded, position)
}

/// Snaps a sticky item's recorded containing-block edges the way
/// [`rounded_layout`] snaps the box they bound: in the box parent's
/// coordinates, through the parent's own snapped position.
fn rounded_containing_block(
    bounds: Edges<f32>,
    scale: f32,
    parent_position: Point<f32>,
) -> Edges<f32> {
    let snap = |value: f32| css_round_to_integer(value * scale) / scale;
    let snapped_parent = parent_position.map(snap);
    Edges {
        left: snap(parent_position.x + bounds.left) - snapped_parent.x,
        right: snap(parent_position.x + bounds.right) - snapped_parent.x,
        top: snap(parent_position.y + bounds.top) - snapped_parent.y,
        bottom: snap(parent_position.y + bounds.bottom) - snapped_parent.y,
    }
}

#[allow(
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    reason = "the extra parameters are propagation state of one recursion, not an API surface"
)]
fn round_layout_inner<T: LayoutTree>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    scale: f32,
    parent_position: Point<f32>,
    pre_node: &mut impl FnMut(&T, &mut T::State, T::NodeId) -> bool,
    visit_pre_node: bool,
    ancestor_moved: bool,
) {
    if !ancestor_moved && !tree.layout(state, node).needs_rounding() {
        // Nothing under here was written since the last rounding, and it
        // rounds from the same accumulated position, so every `rounded` box in
        // this subtree already holds the value this walk would recompute.
        return;
    }
    let visit_pre_node = visit_pre_node && pre_node(tree, state, node);
    let (rounded, position) =
        rounded_layout(&tree.layout(state, node).unrounded, scale, parent_position);
    if let Some(bounds) = tree.sticky_containing_block(state, node) {
        tree.set_rounded_sticky_containing_block(
            state,
            node,
            rounded_containing_block(bounds, scale, parent_position),
        );
    }
    let slot = tree.layout_mut(state, node);
    // The hook writes the boxes of out-of-flow children here rather than
    // during layout, so read the mark after it has run.
    let moved = ancestor_moved || slot.box_changed();
    slot.rounded = rounded;
    slot.clear_rounding_marks();

    for child in tree.children(node) {
        round_layout_inner(
            tree,
            state,
            child,
            scale,
            position,
            pre_node,
            visit_pre_node,
            moved,
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use core::cell::Cell;

    use stylo::values::computed::Display;

    use super::*;

    std::thread_local! {
        pub(super) static ROUND_SNAP_CALLS: Cell<usize> = const { Cell::new(0) };
    }

    fn edges<T>(left: T, right: T, top: T, bottom: T) -> Edges<T> {
        Edges {
            left,
            right,
            top,
            bottom,
        }
    }

    macro_rules! assert_cases {
        ($function:ident; $(
            $name:literal: ($($argument:expr),+ $(,)?) => $expected:expr;
        )+) => {
            $(assert_eq!($function($($argument),+), $expected, "{}", $name);)+
        };
    }

    #[derive(Debug)]
    struct RoundingStyle;

    impl CoreStyle for RoundingStyle {
        fn display(&self) -> Display {
            Display::Flex
        }
    }

    struct RoundingTree;

    impl LayoutTree for RoundingTree {
        type NodeId = ();
        type State = crate::tree::LayoutSlot;
        type Style<'tree> = &'static RoundingStyle;
        type ChildIter<'tree> = core::iter::Empty<()>;

        fn children(&self, (): ()) -> Self::ChildIter<'_> {
            core::iter::empty()
        }

        fn style(&self, (): ()) -> Self::Style<'_> {
            &RoundingStyle
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            (): (),
        ) -> &'state crate::tree::LayoutSlot {
            state
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            (): (),
        ) -> &'state mut crate::tree::LayoutSlot {
            state
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            (): (),
            _input: LayoutInput,
        ) -> LayoutOutput {
            unreachable!("rounding does not compute box layouts")
        }
    }

    /// Two branches under one root, each with a leaf. The "algorithm" is a
    /// commit that writes every child's box, so a pass over it leaves exactly
    /// the marks a real one would.
    struct BranchTree;

    impl BranchTree {
        const CHILDREN: [&'static [usize]; 5] = [&[1, 3], &[2], &[], &[4], &[]];

        fn box_of(node: usize) -> Layout {
            let mut layout = Layout::with_order(0);
            #[allow(clippy::cast_precision_loss)]
            let index = node as f32;
            layout.location = Point::new(0.0, index * 10.0);
            layout.size = Size::new(50.0, 20.0);
            layout
        }

        fn commit_input() -> LayoutInput {
            LayoutInput::commit(
                Size::new(Some(50.0), Some(20.0)),
                Size::NONE,
                Size::new(
                    AvailableSpace::Definite(50.0),
                    AvailableSpace::Definite(20.0),
                ),
                Size::new(false, false),
            )
        }
    }

    impl LayoutTree for BranchTree {
        type NodeId = usize;
        type State = Vec<crate::tree::LayoutSlot>;
        type Style<'tree> = &'static RoundingStyle;
        type ChildIter<'tree> = core::iter::Copied<core::slice::Iter<'tree, usize>>;

        fn children(&self, node: usize) -> Self::ChildIter<'_> {
            Self::CHILDREN[node].iter().copied()
        }

        fn style(&self, _node: usize) -> Self::Style<'_> {
            &RoundingStyle
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            node: usize,
        ) -> &'state crate::tree::LayoutSlot {
            &state[node]
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            node: usize,
        ) -> &'state mut crate::tree::LayoutSlot {
            &mut state[node]
        }

        fn compute_layout(
            &self,
            state: &mut Self::State,
            node: usize,
            input: LayoutInput,
        ) -> LayoutOutput {
            compute_cached_layout(self, state, node, input, |tree, state, node, _input| {
                for child in tree.children(node) {
                    tree.compute_layout(state, child, Self::commit_input());
                    tree.set_unrounded_layout(state, child, Self::box_of(child));
                }
                LayoutOutput::new(Size::new(50.0, 20.0), Size::new(50.0, 20.0))
            })
        }
    }

    fn run_branch_pass(tree: &BranchTree, state: &mut Vec<crate::tree::LayoutSlot>) {
        tree.compute_layout(state, 0, BranchTree::commit_input());
        tree.set_unrounded_layout(state, 0, BranchTree::box_of(0));
    }

    fn round_branches(tree: &BranchTree, state: &mut Vec<crate::tree::LayoutSlot>) -> Vec<usize> {
        let mut visited = Vec::new();
        round_layout_subtree_with(tree, state, 0, 1.0, Point::ZERO, false, |_, _, node| {
            visited.push(node);
            true
        });
        visited
    }

    #[test]
    fn the_rounding_tail_visits_only_the_dirty_spine_and_what_moved() {
        let tree = BranchTree;
        let mut state = (0..5).map(|_| crate::tree::LayoutSlot::default()).collect();

        run_branch_pass(&tree, &mut state);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 1, 2, 3, 4],
            "the first pass writes every box, so the tail owes every node",
        );

        run_branch_pass(&tree, &mut state);
        assert!(
            round_branches(&tree, &mut state).is_empty(),
            "a pass that rewrote nothing leaves the tail nothing to do",
        );

        // What a host's invalidation does: clear the mutated node and the
        // spine above it, leaving the other branch's caches alone.
        for node in [2, 1, 0] {
            tree.clear_layout_cache(&mut state, node);
        }
        run_branch_pass(&tree, &mut state);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 1, 2],
            "only the recomputed spine is walked; the untouched branch is skipped",
        );

        // A box that actually moved carries its whole subtree with it: every
        // descendant rounds from a new accumulated position. Both marks come
        // from the same place a pass would leave them — the parent ran, and
        // wrote one child a different box.
        let mut moved = BranchTree::box_of(3);
        moved.location.y += 4.0;
        tree.layout_mut(&mut state, 0).mark_subtree_dirty();
        tree.set_unrounded_layout(&mut state, 3, moved);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 3, 4],
            "the moved child pulls its subtree in; its unmoved sibling stays out",
        );
    }

    #[test]
    fn the_anchor_seam_defaults_mean_no_anchors() {
        use crate::tree::{AnchorOutcome, AnchorSpec};
        let tree = BranchTree;
        let mut state: Vec<crate::tree::LayoutSlot> =
            (0..5).map(|_| crate::tree::LayoutSlot::default()).collect();
        let name = crate::style::TreeScoped::with_default_level(crate::style::DashedIdent(
            stylo::Atom::from("--a"),
        ));
        assert_eq!(tree.anchor_rect(&state, 1, 0, AnchorSpec::Default), None);
        assert_eq!(
            tree.anchor_rect(&state, 1, 0, AnchorSpec::Named(&name)),
            None
        );
        assert_eq!(tree.default_anchor(&state, 1, 0), None);
        assert!(!tree.anchor_scrolls_with_default(
            &state,
            1,
            0,
            &name,
            crate::style::PhysicalAxis::Vertical,
        ));
        assert_eq!(tree.scrollable_containing_block(&state, 1), None);
        assert_eq!(tree.position_option_count(1), 0);
        assert!(core::ptr::eq(
            tree.position_option_style(1, 0),
            tree.style(1)
        ));
        assert_eq!(tree.last_successful_option(&state, 1), None);
        tree.set_anchor_outcome(
            &mut state,
            1,
            AnchorOutcome {
                chosen: 0,
                overflows: false,
                references_default_anchor: false,
                default_anchor_resolved: false,
                compensates: Size::new(false, false),
                imcb: Rect::ZERO,
                margin_box: Rect::ZERO,
            },
        );
        // Hoisting: a host that lowers no position reports nothing, and the
        // default commit is a plain store.
        assert!(tree.hoisted_children(&state, 0).is_empty());
        assert_eq!(tree.hoisted_parent_offset(&state, 0, 1), Point::ZERO);
        let mut hoisted = Layout::with_order(0);
        hoisted.size = Size::new(3.0, 4.0);
        tree.set_hoisted_layout(&mut state, 0, 1, hoisted);
        assert_eq!(state[1].unrounded.size, Size::new(3.0, 4.0));
        compute_hoisted_children(&tree, &mut state, 0, Size::ZERO, Edges::ZERO, false);
    }

    #[test]
    fn hiding_an_already_hidden_subtree_costs_nothing() {
        let tree = BranchTree;
        let mut state: Vec<crate::tree::LayoutSlot> =
            (0..5).map(|_| crate::tree::LayoutSlot::default()).collect();
        run_branch_pass(&tree, &mut state);
        let _ = round_branches(&tree, &mut state);

        // Hiding runs from the parent's own commit, which is what puts the
        // spine mark on the parent.
        tree.layout_mut(&mut state, 0).mark_subtree_dirty();
        hide_subtree(&tree, &mut state, 1);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0, 1, 2],
            "hiding zeroes the subtree's boxes, which the tail has to pick up",
        );

        tree.layout_mut(&mut state, 0).mark_subtree_dirty();
        hide_subtree(&tree, &mut state, 1);
        assert_eq!(
            round_branches(&tree, &mut state),
            vec![0],
            "hiding an already-hidden subtree writes nothing the tail must chase",
        );

        // Laying the node out again takes the subtree back out of hiding.
        tree.set_unrounded_layout(&mut state, 1, BranchTree::box_of(1));
        assert!(!tree.layout(&state, 1).is_hidden());
    }

    #[test]
    fn rounding_reuses_twenty_unique_snaps_without_changing_bits() {
        let unrounded = Layout {
            order: 17,
            location: Point::new(0.37, -0.42),
            size: Size::new(20.18, 13.73),
            content_size: Size::new(24.91, 15.09),
            border: edges(1.13, 2.27, 0.77, 1.91),
            padding: edges(3.08, 0.66, 2.42, 1.36),
            margin: edges(4.17, -0.83, 1.27, 3.44),
        };
        let scale = 1.25;
        let parent_position = Point::new(-7.31, 5.19);
        let expected = Layout {
            order: 17,
            location: Point::ZERO,
            size: Size::new(20.8, 13.599_999),
            content_size: Size::new(24.8, 15.2),
            border: edges(1.599_999_9, 2.400_000_6, 0.799_999_7, 1.600_000_4),
            padding: edges(3.199_999_8, 0.800_000_2, 2.4, 1.599_999_4),
            margin: edges(4.0, -0.800_000_2, 1.600_000_1, 3.200_000_8),
        };
        let tree = RoundingTree;
        let mut state = crate::tree::LayoutSlot::default();
        state.unrounded = unrounded;

        ROUND_SNAP_CALLS.set(0);
        round_layout_subtree(&tree, &mut state, (), scale, parent_position);
        let actual = &state.rounded;

        assert_eq!(ROUND_SNAP_CALLS.get(), 20);
        assert_eq!(actual.order, expected.order);
        macro_rules! assert_field_bits {
            ($($field:ident),+ $(,)?) => {
                $(assert_eq!(
                    actual.$field.map(f32::to_bits),
                    expected.$field.map(f32::to_bits),
                    stringify!($field),
                );)+
            };
        }
        assert_field_bits!(location, size, content_size, border, padding, margin);
    }

    #[test]
    fn grid_containing_block_edges_snap_in_the_box_parents_coordinates() {
        assert_eq!(
            rounded_containing_block(edges(1.1, 21.4, 2.1, 17.7), 2.0, Point::new(0.2, 0.3)),
            edges(1.5, 21.5, 2.0, 17.5)
        );
    }

    #[test]
    fn root_auto_margins_cover_indefinite_fixed_single_and_double_auto_cases() {
        let fixed = edges(Some(3.0), Some(7.0), Some(2.0), Some(4.0));
        let expected_fixed = edges(3.0, 7.0, 2.0, 4.0);
        let definite = AvailableSpace::Definite(100.0);
        assert_cases! { resolve_root_margins;
            "fixed indefinite":
                (fixed, Edges::uniform(false), AvailableSpace::MaxContent, 40.0) => expected_fixed;
            "fixed definite":
                (fixed, Edges::uniform(false), definite, 40.0) => expected_fixed;
            "both horizontal auto":
                (Edges::uniform(None), edges(true, true, false, false), definite, 40.0)
                => edges(30.0, 30.0, 0.0, 0.0);
            "right auto":
                (edges(Some(5.0), None, None, None), edges(false, true, false, false),
                 definite, 40.0) => edges(5.0, 55.0, 0.0, 0.0);
        }
    }

    /// A style carrying exactly what the host-placement accessors read.
    #[derive(Debug)]
    struct HostPlacedStyle {
        margin: Edges<stylo::values::computed::Margin>,
        padding: Edges<stylo::values::computed::NonNegativeLengthPercentage>,
        border: Edges<stylo::values::computed::BorderSideWidth>,
    }

    impl CoreStyle for HostPlacedStyle {
        fn display(&self) -> Display {
            Display::Flex
        }

        fn margin(&self) -> Edges<&stylo::values::computed::Margin> {
            self.margin.as_ref()
        }

        fn padding(&self) -> Edges<&stylo::values::computed::NonNegativeLengthPercentage> {
            self.padding.as_ref()
        }

        fn border(&self) -> Edges<stylo::values::computed::BorderSideWidth> {
            self.border.clone()
        }
    }

    /// The three accessors a host that places a box itself — the `<text>`
    /// paragraph is the one in this workspace — resolves its box model with.
    ///
    /// There is deliberately no inset accessor beside them: insets on a box a
    /// host places itself are ignored, following native Lynx
    /// (`docs/tracking/deviations.md`).
    #[test]
    fn host_placement_accessors_resolve_margins_padding_and_border() {
        use stylo::Zero;
        use stylo::values::computed::{
            Au, BorderSideWidth, Length, LengthPercentage, Margin, NonNegativeLengthPercentage,
            Percentage,
        };
        use stylo::values::generics::NonNegative;

        let px = |value: f32| LengthPercentage::new_length(Length::new(value));
        let style = HostPlacedStyle {
            margin: edges(
                Margin::LengthPercentage(px(10.0)),
                Margin::Auto,
                Margin::LengthPercentage(px(-4.0)),
                Margin::LengthPercentage(LengthPercentage::new_percent(Percentage(0.5))),
            ),
            padding: edges(
                NonNegative(px(6.0)),
                NonNegativeLengthPercentage::zero(),
                NonNegative(LengthPercentage::new_percent(Percentage(0.25))),
                NonNegativeLengthPercentage::zero(),
            ),
            border: edges(
                BorderSideWidth(Au::from_f32_px(4.0)),
                BorderSideWidth(Au::from_f32_px(1.0)),
                BorderSideWidth(Au::from_f32_px(2.0)),
                BorderSideWidth(Au::from_f32_px(3.0)),
            ),
        };

        assert_eq!(
            used_margins(&style, Some(40.0)),
            edges(10.0, 0.0, -4.0, 20.0),
            "auto is zero for a box placed against a line, a negative margin \
             survives, and every edge resolves against the inline basis",
        );
        assert_eq!(
            used_padding(&style, Some(40.0)),
            edges(6.0, 0.0, 10.0, 0.0),
            "a vertical padding percentage resolves against the width too",
        );
        assert_eq!(used_border(&style), edges(4.0, 1.0, 2.0, 3.0));
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
        let mut ratio = style;
        ratio.aspect_ratio = Some(2.0);
        let mut vertical_only = style;
        vertical_only.auto_size.width = false;
        let stretch = Size::new(true, true);
        assert_cases! { absolute_known_dimensions;
            "stretch clamp":
                (&style, Size::new(100.0, 80.0), Edges::uniform(5.0), stretch)
                => Size::new(Some(90.0), Some(60.0));
            "ratio defers height":
                (&ratio, Size::new(100.0, 80.0), Edges::uniform(5.0), stretch)
                => Size::new(Some(90.0), None);
            "vertical only clamps minimum":
                (&vertical_only, Size::new(100.0, 30.0), Edges::uniform(20.0), stretch)
                => Size::new(None, Some(10.0));
            "aligned axes fit their content":
                (&style, Size::new(100.0, 80.0), Edges::uniform(5.0), Size::new(false, true))
                => Size::new(None, Some(60.0));
        }
    }

    #[test]
    fn absolute_auto_margins_cover_positive_negative_and_one_sided_equations() {
        use direction::T::{Ltr, Rtl};

        let insets = Edges::uniform(Some(0.0));
        let all_auto = Edges::uniform(None);
        let normal = Size::new(100.0, 80.0);
        let overflow = Size::new(40.0, 80.0);
        let box_size = Size::new(60.0, 40.0);
        assert_cases! { resolve_absolute_margins;
            "centered":
                (all_auto, insets, normal, box_size, Ltr) => Edges::uniform(20.0);
            "ltr overflow":
                (all_auto, insets, overflow, box_size, Ltr)
                => edges(0.0, -20.0, 20.0, 20.0);
            "rtl overflow":
                (all_auto, insets, overflow, box_size, Rtl)
                => edges(-20.0, 0.0, 20.0, 20.0);
            "start edges auto":
                (edges(None, Some(3.0), None, Some(4.0)), insets, normal, box_size, Ltr)
                => edges(37.0, 3.0, 36.0, 4.0);
            "end edges auto":
                (edges(Some(2.0), None, Some(5.0), None), insets, normal, box_size, Ltr)
                => edges(2.0, 38.0, 5.0, 35.0);
        }
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
            aligned: None,
        };
        let mut start = base;
        start.start_inset = Some(7.0);
        let mut end = base;
        end.end_inset = Some(9.0);
        let mut prefer_end = start;
        prefer_end.end_inset = Some(9.0);
        prefer_end.prefer_end = true;
        assert_cases! { absolute_axis_location;
            "static position": (base) => 14.0;
            "start inset": (start) => 10.0;
            "end inset": (end) => 67.0;
            "prefer end with both insets": (prefer_end) => 67.0;
        }
    }
}
