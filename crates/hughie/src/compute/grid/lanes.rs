//! CSS Grid Layout Level 3 grid lanes (`display: grid-lanes`), the masonry
//! layout mode.
//!
//! One axis carries the tracks (the **grid axis**); items stack along the
//! other (the **stacking axis**), each landing at the running end of the
//! shortest lane its span can occupy. The grid axis reuses regular Grid's
//! template expansion, line resolution and §12 track sizing verbatim;
//! placement, stacking and the container's stacking-axis size are Level 3's
//! own.
//!
//! §3.4.2's virtual-item grouping is deliberately not implemented. A
//! contribution here depends on the tracks the item spans — both the automatic
//! minimum and the fixed-maximum clamp read them — so a group maximum would
//! have to be recomputed per candidate position anyway. The algorithm instead
//! clones one sizing item per candidate start position, and builds no clones
//! at all unless some track consults its items.
//!
//! Also not implemented: `inline-grid-lanes`; an orientation property and
//! `grid-auto-flow: normal`; `dense` backfilling (§4.4 step 4); §3.1.1's
//! intrinsic `repeat(auto-fill, auto)`; §6.4 stacking-axis self-alignment;
//! baseline alignment and baseline sharing in either axis; subgrid;
//! fragmentation.

#![allow(clippy::cast_precision_loss)]

use stylo::computed_values::direction;
use stylo::values::computed::GridTemplateComponent;
use stylo::values::computed::length::NonNegativeLengthPercentageOrNormal;
use stylo::values::specified::align::AlignFlags;

use super::alignment::{align_tracks, alignment_spacing_from_free_space, item_alignment_offset};
use super::placement::{AxisPlacement, GridArea, TrackSpan, resolve_axis_placement};
use super::sizing::{
    CrossAxisTracks, IntrinsicSizingScratch, initialize_tracks, resolve_item_intrinsic_dimensions,
    size_tracks, track_sizing_reads_items,
};
use super::tracks::{
    AxisTrackSpec, GRID_LINE_LIMIT, MAX_MATERIALIZED_TRACKS, build_axis_tracks, expand_template,
};
use super::types::{Axis, GridItem, Track, TrackSet, TrackSizingFunction};
use super::{
    ContainerPrologue, ItemDefaults, classify_item, container_prologue, final_outer_axis,
    item_area_geometry, item_commit_independence, layout_absolute_items, refresh_item_basis,
    relative_item_offset, resolve_grid_item, span_is_fixed,
};
use crate::compute::hide_subtree;
use crate::compute::single_axis::flow_to_physical;
use crate::compute::util::{
    accumulate_scrollable_overflow, container_content_independence, normalize_content_alignment,
    normalize_item_alignment, own_scrollable_overflow, resolve_gap_axis, resolve_length_percentage,
    sort_and_assign_layout_order,
};
use crate::geometry::{Point, Size};
use crate::style::containment::size_containment;
use crate::style::{Contain, CoreStyle, FlowTolerance, GridLanesStyle, GridStyle, Overflow};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
};

/// §4.2: the distance from the shortest lane within which two lanes count as
/// equally filled. `normal` is `1em`; a percentage resolves against the used
/// grid-axis content-box size, which placement always runs after and which the
/// threshold never feeds back into.
fn tie_threshold(tolerance: &FlowTolerance, font_size: f32, basis: f32) -> f32 {
    match tolerance {
        FlowTolerance::Normal => font_size.max(0.0),
        FlowTolerance::Infinite => f32::INFINITY,
        FlowTolerance::LengthPercentage(value) => resolve_length_percentage(&value.0, Some(basis))
            .unwrap_or(0.0)
            .max(0.0),
    }
}

/// §3.3.1: placement runs after sizing, so `auto-fit` occupancy is a
/// heuristic. Every track a definite-position item covers counts as occupied;
/// then, with `auto_span_total` the sum of the spans of all auto-placed items,
/// so do the first that many tracks still unoccupied. Every other `auto-fit`
/// track collapses.
fn auto_fit_occupancy(
    range: TrackSpan,
    placements: &[AxisPlacement],
    auto_span_total: usize,
) -> Vec<bool> {
    let len =
        usize::try_from(range.end - range.start).expect("the implicit grid range is well ordered");
    let mut occupied = vec![false; len];
    for placement in placements {
        if let AxisPlacement::Definite(span) = *placement {
            let start = usize::try_from(span.start - range.start)
                .expect("the implicit grid covers every definite span");
            let end = usize::try_from(span.end - range.start)
                .expect("the implicit grid covers every definite span");
            for slot in &mut occupied[start..end] {
                *slot = true;
            }
        }
    }
    let mut remaining = auto_span_total;
    for slot in &mut occupied {
        if remaining == 0 {
            break;
        }
        if !*slot {
            *slot = true;
            remaining -= 1;
        }
    }
    occupied
}

/// Whether a `span`-track window starting at `start` is free of collapsed
/// lanes. A collapsed lane is zero-sized, so nothing may be placed across it.
#[inline]
fn window_is_open(collapsed: &[bool], start: usize, span: usize) -> bool {
    !collapsed[start..start + span].iter().any(|&value| value)
}

/// §4.4 step 1: the lane whose spanned tracks have the smallest maximum
/// running position, with everything within `tolerance` of that minimum
/// counting as equally short, resolved in favour of the first such lane at or
/// after the auto-placement cursor.
fn choose_lane(
    running: &[f32],
    collapsed: &[bool],
    span: usize,
    cursor: usize,
    tolerance: f32,
) -> usize {
    debug_assert!(span >= 1 && span <= running.len());
    let last_start = running.len() - span;
    let max_pos = |start: usize| {
        running[start..start + span]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max)
    };
    let open = || (0..=last_start).filter(|&start| window_is_open(collapsed, start, span));
    let best = open()
        .map(max_pos)
        .reduce(f32::min)
        .expect("the auto-fit heuristic keeps at least one window of every item's span open");
    let mut before_cursor = None;
    for start in open().filter(|&start| max_pos(start) - best <= tolerance) {
        if start >= cursor {
            return start;
        }
        before_cursor.get_or_insert(start);
    }
    before_cursor.expect("the shortest lane is always within its own tolerance")
}

/// §6.3: the stacking range is the one alignment subject, so content
/// distribution collapses to a single offset and the distributed values take
/// their fallback alignment.
fn stacking_content_offset(free: f32, alignment: AlignFlags) -> f32 {
    alignment_spacing_from_free_space(free.max(0.0), 1, alignment).0
}

#[inline]
fn grid_axis_area(axis: Axis, span: TrackSpan) -> GridArea {
    match axis {
        Axis::Horizontal => GridArea {
            column: span,
            row: TrackSpan::default(),
        },
        Axis::Vertical => GridArea {
            column: TrackSpan::default(),
            row: span,
        },
    }
}

/// §3.4: a definite-position item contributes once, at its own span; an
/// auto-placed item is assumed to sit at every start position its span fits
/// and contributes once per position.
fn sizing_items<N: Copy>(
    items: &[GridItem<N>],
    placements: &[AxisPlacement],
    collapsed: &[bool],
    first_line: i32,
    axis: Axis,
) -> Vec<GridItem<N>> {
    let count = collapsed.len();
    let mut sizing = Vec::with_capacity(items.len());
    let mut push = |item: &GridItem<N>, span: TrackSpan| {
        let mut clone = item.clone();
        clone.area = grid_axis_area(axis, span);
        sizing.push(clone);
    };
    for (item, placement) in items.iter().zip(placements) {
        match *placement {
            AxisPlacement::Definite(span) => push(item, span),
            AxisPlacement::Indefinite { span } => {
                let span = span.min(count).max(1);
                for start in
                    (0..=count - span).filter(|&start| window_is_open(collapsed, start, span))
                {
                    let start_line = first_line + line_offset(start);
                    push(
                        item,
                        TrackSpan {
                            start: start_line,
                            end: start_line + line_offset(span),
                        },
                    );
                }
            }
        }
    }
    sizing
}

#[inline]
fn line_offset(tracks: usize) -> i32 {
    i32::try_from(tracks).expect("grid track counts are clamped to 20,000")
}

/// Sizes the grid axis once: §12 track sizing over the sizing item list, with
/// the stacking axis passed as indefinite.
#[allow(clippy::too_many_arguments)]
fn size_grid_axis<T>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    specs: &[AxisTrackSpec],
    collapsed: &[bool],
    items: &[GridItem<T::NodeId>],
    placements: &[AxisPlacement],
    basis: Size<Option<f32>>,
    gap: f32,
    available: AvailableSpace,
    alignment: AlignFlags,
    scratch: &mut IntrinsicSizingScratch,
) where
    T: LayoutTree,
{
    initialize_tracks(tracks, specs, axis.size(basis), gap);
    // Nothing reads the per-position clones unless some track consults the
    // items placed in it, so an all-fixed grid axis builds none of them. The
    // condition is `size_tracks`'s own, borrowed rather than restated.
    let mut sizing = if track_sizing_reads_items(tracks) {
        sizing_items(items, placements, collapsed, tracks.first_coordinate, axis)
    } else {
        Vec::new()
    };
    size_tracks(
        tree,
        state,
        axis,
        tracks,
        None,
        &mut sizing,
        basis,
        available,
        alignment,
        scratch,
    );
}

/// One in-flow item after placement, before the stacking-axis content
/// distribution offset exists.
struct PlacedItem<N> {
    node: N,
    layout: Layout,
    /// Margin-box start in the stacking axis, from the content edge.
    stacking_start: f32,
    /// Margin-box extent in the stacking axis.
    stacking_extent: f32,
    /// What sits between the container's border-box origin and the item's
    /// border box on the stacking axis regardless of the distribution offset:
    /// the content origin, the start margin, and any relative inset.
    stacking_bias: f32,
    /// Index of the first track the item covers in the grid axis.
    first_track: usize,
    /// Whether this item is the first one placed in at least one of its tracks.
    first_in_track: bool,
    baseline: Option<f32>,
    overflow: Point<Overflow>,
}

/// Everything §4.4's pass reads that is fixed for the whole pass.
struct LanesContext<'a> {
    grid_axis: Axis,
    tracks: &'a TrackSet,
    collapsed: &'a [bool],
    inner_grid: f32,
    content_origin: Point<f32>,
    stacking_gap: f32,
    tolerance: f32,
    rtl: bool,
    goal: LayoutGoal,
    /// Whether every live track of the grid axis is sized by its own function
    /// alone, which is what an auto-placed item's area independence needs.
    all_tracks_fixed: bool,
}

struct LanesPass<N> {
    items: Vec<PlacedItem<N>>,
    /// §5: the endmost outer edge any item reaches. Every lane starts at the
    /// content edge, so this is the whole stacking range.
    stacking_range: f32,
}

/// §4.4 steps 1–3, in order-modified document order. Lays each item out at its
/// final grid-axis geometry and its pre-distribution stacking position; step 4
/// (`dense` backfilling) is not implemented, so the pass is strictly sparse.
#[allow(clippy::too_many_lines)]
fn run_lanes<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [GridItem<T::NodeId>],
    placements: &[AxisPlacement],
    context: &LanesContext<'_>,
) -> LanesPass<T::NodeId>
where
    T: LayoutTree,
{
    let axis = context.grid_axis;
    let stacking = axis.other();
    let first_line = context.tracks.first_coordinate;
    let count = context.tracks.tracks.len();
    let mut running = vec![0.0_f32; count];
    let mut used = vec![false; count];
    let mut cursor = 0_usize;
    let mut placed = Vec::with_capacity(items.len());
    let mut stacking_range = 0.0_f32;

    for (item, placement) in items.iter_mut().zip(placements) {
        let (start, span, definite) = match *placement {
            // A definite position is used as-is and leaves the cursor alone.
            AxisPlacement::Definite(span) => (
                usize::try_from(span.start - first_line)
                    .expect("the implicit grid covers every definite span"),
                usize::try_from(span.end - span.start)
                    .expect("a resolved span covers at least one track"),
                true,
            ),
            AxisPlacement::Indefinite { span } => {
                let span = span.min(count).max(1);
                let start =
                    choose_lane(&running, context.collapsed, span, cursor, context.tolerance);
                cursor = start + span;
                (start, span, false)
            }
        };
        let lines = TrackSpan {
            start: first_line + line_offset(start),
            end: first_line + line_offset(start + span),
        };

        item.area = grid_axis_area(axis, lines);
        let track_area = context.tracks.area_size(lines.start, lines.end);
        // The containing block is the spanned tracks in the grid axis and
        // indefinite in the stacking axis: an item is placed at a running
        // position, not inside a sized area.
        let area = axis.pack(Some(track_area), None);
        refresh_item_basis(tree, item, area);
        item.clear_contribution_cache(Axis::Horizontal);
        item.clear_contribution_cache(Axis::Vertical);
        // A grid-axis intrinsic keyword measures across an indefinite stacking
        // size, but a stacking-axis one measures across the lane it landed in,
        // whose grid-axis size is already resolved.
        resolve_item_intrinsic_dimensions(tree, state, item, axis, None, area);
        resolve_item_intrinsic_dimensions(
            tree,
            state,
            item,
            stacking,
            Some(CrossAxisTracks::resolved(context.tracks)),
            area,
        );
        let (known, available) = item_area_geometry(item, area);
        let item_goal = match context.goal.independence() {
            None => context.goal,
            Some(container) => {
                // §3.4: an auto-placed item contributes to every lane it could
                // land in, so its area only stands still when every live track
                // does. Which lane it lands in follows the earlier siblings'
                // sizes, never its own content.
                let area_is_fixed = if definite {
                    span_is_fixed(context.tracks, lines)
                } else {
                    context.all_tracks_fixed
                };
                let mut independent = item_commit_independence(
                    tree,
                    item,
                    known,
                    axis.pack(area_is_fixed, false),
                    container,
                );
                // The stacking axis has no area to chain off at all.
                stacking.set_size(&mut independent, false);
                LayoutGoal::Commit {
                    content_independent: independent,
                }
            }
        };
        let output = tree.compute_layout(
            state,
            item.key.node,
            LayoutInput::new(item_goal, known, area, available),
        );

        // Auto margins take the grid-axis free space; in the stacking axis
        // there is none to take, so they stay at zero.
        let mut margin = item.margin;
        let auto_start = item.margin_auto.start(axis);
        let auto_end = item.margin_auto.end(axis);
        let auto_count = usize::from(auto_start) + usize::from(auto_end);
        if auto_count > 0 {
            let free = track_area - axis.size(output.size) - axis.sum(margin);
            let share = free.max(0.0) / auto_count as f32;
            if auto_start {
                axis.set_start(&mut margin, share);
            }
            if auto_end {
                axis.set_end(&mut margin, share);
            }
        }

        let grid_reverse = context.rtl && axis == Axis::Horizontal;
        let (container_reversed, self_reversed) = if axis == Axis::Horizontal {
            (context.rtl, item.direction == direction::T::Rtl)
        } else {
            (false, false)
        };
        let self_alignment = match axis {
            Axis::Horizontal => item.justify_self,
            Axis::Vertical => item.align_self,
        };
        let free = track_area - axis.size(output.size) - axis.sum(margin);
        let align_offset =
            item_alignment_offset(free, self_alignment, container_reversed, self_reversed);
        let logical_track_start = context.tracks.line_position(lines.start);
        let grid_start = if grid_reverse {
            context.inner_grid - logical_track_start - track_area
        } else {
            logical_track_start
        };

        // §4.4 step 2: the item starts where the last of its lanes ends.
        let position = running[start..start + span]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let stacking_extent = (stacking.size(output.size) + stacking.sum(margin)).max(0.0);
        // §6.1: the stacking gutter sits before every item but the first in a
        // track, which is what leaving each lane's running position at zero
        // until its first item does.
        for lane in &mut running[start..start + span] {
            *lane = position + stacking_extent + context.stacking_gap;
        }
        stacking_range = stacking_range.max(position + stacking_extent);
        let first_in_track = used[start..start + span].iter().any(|&value| !value);
        for lane in &mut used[start..start + span] {
            *lane = true;
        }

        let relative = relative_item_offset(item);
        let mut layout = Layout::with_order(item.key.layout_order);
        layout.size = output.size;
        layout.content_size = output.content_size;
        layout.border = item.border;
        layout.padding = item.padding;
        layout.margin = margin;
        axis.set_point(
            &mut layout.location,
            axis.point(context.content_origin)
                + grid_start
                + axis.start(margin)
                + align_offset
                + axis.point(relative),
        );
        placed.push(PlacedItem {
            node: item.key.node,
            layout,
            stacking_start: position,
            stacking_extent,
            stacking_bias: stacking.point(context.content_origin)
                + stacking.start(margin)
                + stacking.point(relative),
            first_track: start,
            first_in_track,
            baseline: output.first_baselines.y,
            overflow: item.overflow,
        });
    }

    LanesPass {
        items: placed,
        stacking_range,
    }
}

/// §6.5: with lanes running down the columns the container's first baseline is
/// the highest one among the items placed first in each track; with lanes
/// running across the rows it is the first item's, in order, in the first
/// track that received one. An item without a baseline contributes none — no
/// bottom edge is synthesised in its place.
fn lanes_first_baseline<N>(items: &[PlacedItem<N>], grid_axis: Axis) -> Option<f32> {
    let baseline = |item: &PlacedItem<N>| {
        item.baseline
            .map(|baseline| item.layout.location.y + baseline)
    };
    match grid_axis {
        Axis::Horizontal => items
            .iter()
            .filter(|item| item.first_in_track)
            .filter_map(baseline)
            .reduce(f32::min),
        Axis::Vertical => items
            .iter()
            .filter(|item| item.baseline.is_some())
            .min_by_key(|item| item.first_track)
            .and_then(baseline),
    }
}

#[allow(clippy::too_many_lines)]
pub fn compute_grid_lanes_layout<'tree, T>(
    tree: &'tree T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    T: LayoutTree + 'tree,
    T::Style<'tree>: GridLanesStyle,
{
    let style = tree.style(node);
    let size_containment = size_containment(&style);
    let layout_contained = style.containment().contains(Contain::LAYOUT);
    let gap_value = style.gap();
    let rtl = style.direction() == direction::T::Rtl;
    // §2.3: with the orientation property still unspecified, only its initial
    // `normal` behaviour exists — the block axis carries the tracks exactly
    // when `grid-template-rows` alone names any.
    let grid_axis = if matches!(style.grid_template_columns(), GridTemplateComponent::None)
        && !matches!(style.grid_template_rows(), GridTemplateComponent::None)
    {
        Axis::Vertical
    } else {
        Axis::Horizontal
    };
    let stacking_axis = grid_axis.other();

    let align_content = normalize_content_alignment(style.align_content().primary(), false, rtl)
        .unwrap_or(AlignFlags::STRETCH);
    let justify_content = normalize_content_alignment(style.justify_content().primary(), true, rtl)
        .unwrap_or(AlignFlags::STRETCH);
    let content_alignment = Size::new(justify_content, align_content);
    let align_items = normalize_item_alignment(style.align_items().0, false, rtl);
    let item_defaults = ItemDefaults {
        align_items: align_items.unwrap_or(AlignFlags::STRETCH),
        align_items_normal: align_items.is_none(),
        justify_items: normalize_item_alignment(style.justify_items().computed.0.0, true, rtl)
            .unwrap_or(AlignFlags::STRETCH),
        rtl,
    };

    let ContainerPrologue {
        metrics,
        style_definite,
        percentage_basis,
        gap: initial_gap,
        repeat_max_basis,
        repeat_min_basis,
        repeat_count_gap,
    } = container_prologue(&style, input);
    let explicit = expand_template(
        match grid_axis {
            Axis::Horizontal => style.grid_template_columns(),
            Axis::Vertical => style.grid_template_rows(),
        },
        grid_axis.size(repeat_max_basis),
        grid_axis.size(repeat_min_basis),
        grid_axis.size(repeat_count_gap),
    );
    let explicit_len = explicit.tracks.len();

    let commits_layout = input.goal.commits();
    let children = tree.flattened_children(node);
    let mut in_flow = Vec::with_capacity(children.capacity_hint());
    let mut absolute = commits_layout.then(Vec::new);
    let mut hidden = commits_layout.then(Vec::new);
    for (document_index, (child, child_style, display)) in children.enumerate() {
        let Some(pending) = classify_item(child, &child_style, display, document_index) else {
            if let Some(hidden) = &mut hidden {
                hidden.push((document_index, child));
            }
            continue;
        };
        if matches!(
            pending.position,
            stylo::values::computed::PositionProperty::Absolute
                | stylo::values::computed::PositionProperty::Fixed
        ) {
            if let Some(absolute) = &mut absolute {
                absolute.push(pending);
            }
        } else {
            in_flow.push(pending);
        }
    }
    if let Some(absolute) = &mut absolute {
        sort_and_assign_layout_order(&mut in_flow, absolute);
    } else if in_flow.iter().any(|item| item.ordered.css_order != 0) {
        in_flow.sort_unstable_by_key(|item| (item.ordered.css_order, item.ordered.document_index));
    }

    // The stacking axis has no lines of its own, so only the grid axis's
    // placement longhands are read.
    let placements = in_flow
        .iter()
        .map(|item| {
            resolve_axis_placement(
                match grid_axis {
                    Axis::Horizontal => item.column,
                    Axis::Vertical => item.row,
                },
                explicit_len,
            )
        })
        .collect::<Vec<_>>();
    let mut range = TrackSpan {
        start: 0,
        end: line_offset(explicit_len),
    };
    let mut auto_span_total = 0_usize;
    let mut largest_auto_span = 0_usize;
    for placement in &placements {
        match *placement {
            AxisPlacement::Definite(span) => {
                range.start = range.start.min(span.start);
                range.end = range.end.max(span.end);
            }
            AxisPlacement::Indefinite { span } => {
                auto_span_total = auto_span_total.saturating_add(span);
                largest_auto_span = largest_auto_span.max(span);
            }
        }
    }
    // The implicit grid grows until the largest auto-placed span fits, as the
    // regular grid's auto-placement does when a span overruns the explicit
    // tracks.
    range.end = range
        .end
        .max(range.start.saturating_add(line_offset(largest_auto_span)))
        .min(GRID_LINE_LIMIT);
    let occupied = auto_fit_occupancy(range, &placements, auto_span_total);
    let auto_tracks = if range.start < 0 || range.end > line_offset(explicit_len) {
        match grid_axis {
            Axis::Horizontal => style.grid_auto_columns(),
            Axis::Vertical => style.grid_auto_rows(),
        }
        .0
        .iter()
        .take(MAX_MATERIALIZED_TRACKS)
        .map(TrackSizingFunction::from_style)
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let specs = build_axis_tracks(&explicit, &auto_tracks, range, &occupied);
    let collapsed = specs.iter().map(|spec| spec.collapsed).collect::<Vec<_>>();

    let mut items = in_flow
        .iter()
        .map(|pending| {
            let key = pending.key();
            let mut item = resolve_grid_item(
                &tree.style(key.node),
                key,
                GridArea::default(),
                Size::NONE,
                item_defaults,
            );
            // §6.5 allows baseline self-alignment to fall back in the stacking
            // axis; grid-axis baseline sharing is not implemented either, so
            // both axes take the fallback before anything reads them.
            if item.align_self == AlignFlags::BASELINE {
                item.align_self = AlignFlags::START;
            }
            if item.justify_self == AlignFlags::BASELINE {
                item.justify_self = AlignFlags::START;
            }
            item
        })
        .collect::<Vec<_>>();

    let mut tracks = TrackSet::default();
    let mut scratch = IntrinsicSizingScratch::default();
    let grid_basis = grid_axis.size(percentage_basis);
    let grid_alignment = grid_axis.size(content_alignment);
    size_grid_axis(
        tree,
        state,
        grid_axis,
        &mut tracks,
        &specs,
        &collapsed,
        &items,
        &placements,
        percentage_basis,
        grid_axis.size(initial_gap),
        grid_basis.map_or(
            grid_axis.size(metrics.available_inner),
            AvailableSpace::Definite,
        ),
        grid_alignment,
        &mut scratch,
    );
    let grid_content = match size_containment {
        Some(intrinsic) => grid_axis.size(intrinsic).unwrap_or(0.0),
        None => tracks.used_size(),
    };
    let grid_outer = final_outer_axis(&metrics, grid_axis, grid_content);
    let grid_inner = (grid_outer - grid_axis.size(metrics.box_inset)).max(0.0);
    let grid_gap = resolve_gap_axis(grid_axis.size(gap_value), Some(grid_inner));
    // The grid axis reruns under its now-definite basis whenever that basis
    // was missing or the gutter it feeds came out at a different value.
    #[allow(
        clippy::float_cmp,
        reason = "the rerun is keyed on the used gutter being a different value, not a near one"
    )]
    let needs_definite_rerun = grid_basis.is_none() || grid_gap != grid_axis.size(initial_gap);
    if needs_definite_rerun {
        size_grid_axis(
            tree,
            state,
            grid_axis,
            &mut tracks,
            &specs,
            &collapsed,
            &items,
            &placements,
            grid_axis.pack(Some(grid_inner), stacking_axis.size(percentage_basis)),
            grid_gap,
            AvailableSpace::Definite(grid_inner),
            grid_alignment,
            &mut scratch,
        );
    }
    align_tracks(&mut tracks, grid_inner, grid_alignment);

    let all_tracks_fixed = !tracks.tracks.iter().any(|track| {
        !track.collapsed && (track.intrinsic_min || track.intrinsic_max || track.is_flexible())
    });
    let container_goal = container_content_independence(input, style_definite).map_or(
        input.goal,
        |content_independent| LayoutGoal::Commit {
            content_independent,
        },
    );
    let mut context = LanesContext {
        grid_axis,
        tracks: &tracks,
        collapsed: &collapsed,
        inner_grid: grid_inner,
        content_origin: Point::new(
            metrics.border.left + metrics.padding.left,
            metrics.border.top + metrics.padding.top,
        ),
        stacking_gap: stacking_axis.size(initial_gap),
        tolerance: tie_threshold(style.flow_tolerance(), style.font_size(), grid_inner),
        rtl,
        goal: container_goal,
        all_tracks_fixed,
    };
    let stacking_content = |range: f32| match size_containment {
        Some(intrinsic) => stacking_axis.size(intrinsic).unwrap_or(0.0),
        None => range,
    };
    // A percentage stacking gutter against an indefinite stacking size is
    // cyclic: it resolves to zero here, and the size that produces is what the
    // real gutter then resolves against. The first pass therefore only
    // measures — no child is ever committed twice.
    let cyclic_stacking_gap = stacking_axis.size(percentage_basis).is_none()
        && matches!(
            stacking_axis.size(gap_value),
            NonNegativeLengthPercentageOrNormal::LengthPercentage(value)
                if value.0.has_percentage()
        );
    if cyclic_stacking_gap {
        context.goal = LayoutGoal::Measure(RequestedAxis::Both);
    }
    let mut pass = run_lanes(tree, state, &mut items, &placements, &context);
    if cyclic_stacking_gap {
        let probe_outer = final_outer_axis(
            &metrics,
            stacking_axis,
            stacking_content(pass.stacking_range),
        );
        let probe_inner = (probe_outer - stacking_axis.size(metrics.box_inset)).max(0.0);
        context.stacking_gap = resolve_gap_axis(stacking_axis.size(gap_value), Some(probe_inner));
        context.goal = container_goal;
        pass = run_lanes(tree, state, &mut items, &placements, &context);
    }

    let stacking_outer = final_outer_axis(
        &metrics,
        stacking_axis,
        stacking_content(pass.stacking_range),
    );
    let stacking_inner = (stacking_outer - stacking_axis.size(metrics.box_inset)).max(0.0);
    let stacking_offset = stacking_content_offset(
        stacking_inner - pass.stacking_range,
        stacking_axis.size(content_alignment),
    );
    let outer_size = grid_axis.pack(grid_outer, stacking_outer);
    let final_inner = grid_axis.pack(grid_inner, stacking_inner);

    // A horizontal stacking axis under `direction: rtl` stacks from the
    // inline-start edge, which is the right one.
    let stacking_reverse = rtl && stacking_axis == Axis::Horizontal;
    let mut content_size = outer_size;
    for placed in &mut pass.items {
        let physical = flow_to_physical(
            stacking_offset + placed.stacking_start,
            placed.stacking_extent,
            stacking_inner,
            stacking_reverse,
        );
        stacking_axis.set_point(&mut placed.layout.location, placed.stacking_bias + physical);
        accumulate_scrollable_overflow(
            &mut content_size,
            placed.layout.location,
            placed.layout.size,
            placed.layout.content_size,
            placed.overflow,
        );
    }
    let baselines = if layout_contained {
        Point::NONE
    } else {
        Point::new(None, lanes_first_baseline(&pass.items, grid_axis))
    };
    if commits_layout {
        for placed in pass.items {
            tree.set_unrounded_layout(state, placed.node, placed.layout);
        }
        for (document_index, child) in hidden.expect("commit keeps hidden grid-lanes items") {
            hide_subtree(tree, state, child);
            tree.set_unrounded_layout(
                state,
                child,
                Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
            );
        }
        // §8: the stacking axis has exactly two lines, the range's own edges,
        // so grid-aligned absolute positioning resolves against a one-track
        // set spanning it.
        let stacking_tracks = TrackSet {
            tracks: vec![Track {
                base: pass.stacking_range,
                position: stacking_offset,
                ..Track::default()
            }],
            gap: 0.0,
            first_coordinate: 0,
            collapsed_line_positions: None,
        };
        let (columns, rows) = match grid_axis {
            Axis::Horizontal => (&tracks, &stacking_tracks),
            Axis::Vertical => (&stacking_tracks, &tracks),
        };
        // Negative line numbers count back from the explicit end, so each axis
        // needs its own count: the template's in the grid axis, one in the
        // stacking axis.
        let explicit_lines = grid_axis.pack(explicit_len, 1);
        let absolute_content_size = layout_absolute_items(
            tree,
            state,
            &absolute.expect("commit keeps out-of-flow grid-lanes items"),
            columns,
            rows,
            explicit_lines.width,
            explicit_lines.height,
            final_inner,
            outer_size,
            metrics.padding,
            metrics.border,
            rtl,
            item_defaults,
        );
        content_size = content_size.zip_map(absolute_content_size, f32::max);
    }
    let content_size = own_scrollable_overflow(&style, outer_size, content_size);
    LayoutOutput::new(outer_size, content_size).with_first_baselines(baselines)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use stylo::values::computed::{Length, LengthPercentage, Percentage};
    use stylo::values::generics::NonNegative;

    use super::*;

    fn definite(start: i32, end: i32) -> AxisPlacement {
        AxisPlacement::Definite(TrackSpan { start, end })
    }

    #[test]
    fn tie_threshold_reads_normal_as_one_em_and_percentages_off_the_grid_axis() {
        assert_eq!(tie_threshold(&FlowTolerance::Normal, 18.0, 200.0), 18.0);
        assert_eq!(
            tie_threshold(&FlowTolerance::Infinite, 18.0, 200.0),
            f32::INFINITY
        );
        let length = FlowTolerance::LengthPercentage(NonNegative(LengthPercentage::new_length(
            Length::new(7.0),
        )));
        assert_eq!(tie_threshold(&length, 18.0, 200.0), 7.0);
        let percentage = FlowTolerance::LengthPercentage(NonNegative(
            LengthPercentage::new_percent(Percentage(0.25)),
        ));
        assert_eq!(tie_threshold(&percentage, 18.0, 200.0), 50.0);
        assert_eq!(tie_threshold(&percentage, 18.0, 0.0), 0.0);
    }

    #[test]
    fn auto_fit_occupancy_keeps_definite_tracks_and_one_track_per_auto_span() {
        let range = TrackSpan { start: 0, end: 6 };
        // Two auto-placed items of span one keep the first two free tracks.
        assert_eq!(
            auto_fit_occupancy(
                range,
                &[
                    definite(3, 4),
                    AxisPlacement::Indefinite { span: 1 },
                    AxisPlacement::Indefinite { span: 1 },
                ],
                2,
            ),
            vec![true, true, false, true, false, false]
        );
        // With no auto-placed item only the definite span survives.
        assert_eq!(
            auto_fit_occupancy(range, &[definite(2, 4)], 0),
            vec![false, false, true, true, false, false]
        );
        // A negative implicit track is part of the range and takes its turn.
        assert_eq!(
            auto_fit_occupancy(
                TrackSpan { start: -1, end: 2 },
                &[definite(-1, 0), AxisPlacement::Indefinite { span: 2 }],
                2,
            ),
            vec![true, true, true]
        );
    }

    #[test]
    fn choose_lane_takes_the_shortest_lane_then_the_cursor_within_tolerance() {
        let open = [false, false, false];
        // Exact shortest lane with no ties.
        assert_eq!(choose_lane(&[30.0, 10.0, 20.0], &open, 1, 0, 0.0), 1);
        // A tie ahead of the cursor wins over the same tie behind it.
        assert_eq!(choose_lane(&[10.0, 10.0, 10.0], &open, 1, 2, 0.0), 2);
        // No possible lane at or after the cursor falls back to the first one.
        assert_eq!(choose_lane(&[10.0, 10.0, 30.0], &open, 1, 2, 0.0), 0);
        // The tolerance widens the tie: lane 2 is within 16px of lane 1.
        assert_eq!(choose_lane(&[10.0, 20.0, 10.0], &open, 1, 1, 16.0), 1);
        assert_eq!(choose_lane(&[10.0, 20.0, 10.0], &open, 1, 1, 0.0), 2);
        // A span takes the largest running position of the lanes it covers, so
        // the tall lane rules out both windows that reach it.
        assert_eq!(
            choose_lane(&[0.0, 50.0, 10.0, 0.0], &[false; 4], 2, 0, 0.0),
            2
        );
        // Collapsed lanes are never spanned.
        assert_eq!(
            choose_lane(&[0.0, 0.0, 10.0], &[false, true, false], 1, 0, 0.0),
            0
        );
        assert_eq!(
            choose_lane(&[30.0, 0.0, 10.0], &[false, true, false], 1, 1, 0.0),
            2
        );
        // `infinite` makes every open lane possible, so the cursor decides.
        assert_eq!(
            choose_lane(&[0.0, 90.0, 0.0], &open, 1, 1, f32::INFINITY),
            1
        );
    }

    #[test]
    fn stacking_content_offset_uses_the_single_subject_fallbacks() {
        use AlignFlags as A;

        assert_eq!(stacking_content_offset(40.0, A::START), 0.0);
        assert_eq!(stacking_content_offset(40.0, A::STRETCH), 0.0);
        assert_eq!(stacking_content_offset(40.0, A::CENTER), 20.0);
        assert_eq!(stacking_content_offset(40.0, A::END), 40.0);
        assert_eq!(stacking_content_offset(40.0, A::FLEX_END), 40.0);
        assert_eq!(stacking_content_offset(40.0, A::SPACE_BETWEEN), 0.0);
        assert_eq!(stacking_content_offset(40.0, A::SPACE_AROUND), 20.0);
        assert_eq!(stacking_content_offset(40.0, A::SPACE_EVENLY), 20.0);
        // An overflowing stacking range never pulls the items backwards.
        assert_eq!(stacking_content_offset(-40.0, A::END), 0.0);
    }
}
