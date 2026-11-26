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
//! Grid-axis sizing groups the span-1 items in the manner of §3.4.2's virtual
//! items: every span-1 item that may sit in a track — the definite-position
//! ones placed there and every auto-placed one — contributes through one
//! aggregate entry per track, whose contributions are the maxima of its
//! members' (`sizing::LaneSizingItems`). A contribution depends on the track
//! only through the automatic minimum and the fixed-maximum clamp, which the
//! aggregate keeps apart and applies per track, and every step that reads a
//! span-1 contribution takes a maximum through monotone operations, so the
//! tracks come out bit-for-bit as with one clone per candidate position — a
//! debug build sizes both ways and compares. An item spanning several tracks
//! is still cloned per candidate start position, since spreading a spanning
//! contribution over its tracks is not shown to commute with the maximum in
//! floating point. No entry is built at all unless some track consults its
//! items.
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
    CrossAxisTracks, GroupMaxima, IntrinsicSizingScratch, LaneEntry, LaneSizingItems,
    initialize_tracks, resolve_item_intrinsic_dimensions, size_tracks, track_sizing_reads_items,
};
use super::tracks::{
    AxisTrackSpec, GRID_LINE_LIMIT, MAX_MATERIALIZED_TRACKS, build_axis_tracks, expand_template,
};
use super::types::{Axis, GridItem, Track, TrackSet};
use super::{
    ContainerPrologue, ItemDefaults, clamped_inner_axis, classify_item, container_prologue,
    final_outer_axis, fit_content_inner_size, item_area_geometry, item_commit_independence,
    layout_absolute_items, refresh_item_basis, relative_item_offset, resolve_grid_item,
    span_is_fixed,
};
use crate::compute::hide_subtree;
use crate::compute::single_axis::flow_to_physical;
use crate::compute::util::{
    accumulate_scrollable_overflow, container_content_independence, is_scroll_container,
    item_end_margin, normalize_content_alignment, normalize_item_alignment,
    own_scrollable_overflow, padding_box_end, resolve_gap_axis, resolve_length_percentage,
    sort_and_assign_layout_order,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::containment::contained_axes;
use crate::style::{
    Contain, CoreStyle, FitTolerance, GridLanesStyle, GridStyle, Overflow, PositionProperty,
};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
};

fn tie_threshold(tolerance: &FitTolerance, font_size: f32, basis: f32) -> f32 {
    match tolerance {
        FitTolerance::Normal => font_size.max(0.0),
        FitTolerance::Infinite => f32::INFINITY,
        FitTolerance::LengthPercentage(value) => resolve_length_percentage(&value.0, Some(basis))
            .unwrap_or(0.0)
            .max(0.0),
    }
}

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

#[inline]
fn window_is_open(tracks: &[Track], start: usize, span: usize) -> bool {
    !tracks[start..start + span]
        .iter()
        .any(|track| track.collapsed)
}

fn choose_lane(
    running: &[f32],
    tracks: &[Track],
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
    let open = || (0..=last_start).filter(|&start| window_is_open(tracks, start, span));
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

fn candidate_positions<'a>(
    placements: &'a [AxisPlacement],
    tracks: &'a [Track],
    first_line: i32,
) -> impl Iterator<Item = (usize, TrackSpan)> + 'a {
    let count = tracks.len();
    placements
        .iter()
        .enumerate()
        .flat_map(move |(index, placement)| {
            let (definite, auto_span) = match *placement {
                AxisPlacement::Definite(span) => (Some(span), None),
                AxisPlacement::Indefinite { span } => (None, Some(span.min(count).max(1))),
            };
            let auto_positions = auto_span.into_iter().flat_map(move |span| {
                (0..=count - span)
                    .filter(move |&start| window_is_open(tracks, start, span))
                    .map(move |start| {
                        let start_line = first_line + line_offset(start);
                        TrackSpan {
                            start: start_line,
                            end: start_line + line_offset(span),
                        }
                    })
            });
            definite
                .into_iter()
                .chain(auto_positions)
                .map(move |span| (index, span))
        })
}

#[inline]
fn placement_span(placement: AxisPlacement, count: usize) -> usize {
    match placement {
        AxisPlacement::Definite(span) => usize::try_from(span.end - span.start).unwrap_or(1).max(1),
        AxisPlacement::Indefinite { span } => span.min(count).max(1),
    }
}

fn sizing_entries<'a, N: Copy>(
    items: &'a mut [GridItem<N>],
    placements: &[AxisPlacement],
    tracks: &TrackSet,
    axis: Axis,
) -> LaneSizingItems<'a, N> {
    let count = tracks.tracks.len();
    let first_line = tracks.first_coordinate;
    let mut sizing = LaneSizingItems::new(items);
    let mut definite = Vec::new();
    for (index, placement) in placements.iter().enumerate() {
        if placement_span(*placement, count) != 1 {
            continue;
        }
        match *placement {
            AxisPlacement::Definite(span) => definite.push((tracks.index_of(span.start), index)),
            AxisPlacement::Indefinite { .. } => sizing.pooled.push(index),
        }
    }
    for (index, span) in candidate_positions(placements, &tracks.tracks, first_line) {
        if placement_span(placements[index], count) == 1 {
            continue;
        }
        let mut clone = sizing.items[index].clone();
        clone.area = grid_axis_area(axis, span);
        sizing.entries.push(LaneEntry::Item(clone));
    }
    definite.sort_by_key(|&(track, _)| track);
    let mut next = 0;
    for track in 0..count {
        let first_member = sizing.members.len();
        while next < definite.len() && definite[next].0 == track {
            sizing.members.push(definite[next].1);
            next += 1;
        }
        let pooled = !sizing.pooled.is_empty() && window_is_open(&tracks.tracks, track, 1);
        if sizing.members.len() > first_member || pooled {
            let line = first_line + line_offset(track);
            sizing.entries.push(LaneEntry::Aggregate {
                span: TrackSpan {
                    start: line,
                    end: line + 1,
                },
                members: first_member..sizing.members.len(),
                pooled,
                maxima: GroupMaxima::default(),
            });
        }
    }
    sizing.spanned_flex_sum = candidate_positions(placements, &tracks.tracks, first_line)
        .flat_map(|(_, span)| {
            tracks.tracks[tracks.span_indices(span.start, span.end)]
                .iter()
                .filter(|track| track.is_flexible())
                .map(|track| track.flex_factor)
        })
        .sum::<f32>();
    sizing
}

#[cfg(debug_assertions)]
fn per_position_items<N: Copy>(
    sizing: &LaneSizingItems<'_, N>,
    placements: &[AxisPlacement],
    tracks: &[Track],
    first_line: i32,
    axis: Axis,
) -> Vec<GridItem<N>> {
    let count = tracks.len();
    let mut spanning = sizing.entries.iter().filter_map(|entry| match entry {
        LaneEntry::Item(item) => Some(item),
        LaneEntry::Aggregate { .. } => None,
    });
    candidate_positions(placements, tracks, first_line)
        .map(|(index, span)| {
            if placement_span(placements[index], count) == 1 {
                let mut clone = sizing.items[index].clone();
                clone.area = grid_axis_area(axis, span);
                clone
            } else {
                spanning
                    .next()
                    .expect("one spanning clone per candidate position")
                    .clone()
            }
        })
        .collect()
}

#[inline]
fn line_offset(tracks: usize) -> i32 {
    i32::try_from(tracks).expect("grid track counts are clamped to 20,000")
}

#[allow(clippy::too_many_arguments)]
fn size_grid_axis<T>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    specs: &[AxisTrackSpec<'_>],
    items: &mut [GridItem<T::NodeId>],
    items_fresh: bool,
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
    let reads_items = track_sizing_reads_items(tracks);
    if reads_items && !items_fresh {
        let item_basis = axis.pack(None, axis.other().size(basis));
        for item in items.iter_mut() {
            if item.intrinsic.has_intrinsic() {
                refresh_item_basis(tree, item, item_basis);
            }
            item.clear_contribution_cache(Axis::Horizontal);
            item.clear_contribution_cache(Axis::Vertical);
        }
    }
    if reads_items {
        let stacking = axis.other();
        for item in items.iter_mut() {
            resolve_item_intrinsic_dimensions(tree, state, item, stacking, None, basis);
        }
    }
    let mut sizing = if reads_items {
        sizing_entries(items, placements, tracks, axis)
    } else {
        LaneSizingItems::new(items)
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
    #[cfg(debug_assertions)]
    if reads_items {
        let mut per_position = per_position_items(
            &sizing,
            placements,
            &tracks.tracks,
            tracks.first_coordinate,
            axis,
        );
        let mut expected = TrackSet {
            first_coordinate: tracks.first_coordinate,
            ..TrackSet::default()
        };
        initialize_tracks(&mut expected, specs, axis.size(basis), gap);
        size_tracks(
            tree,
            state,
            axis,
            &mut expected,
            None,
            per_position.as_mut_slice(),
            basis,
            available,
            alignment,
            &mut IntrinsicSizingScratch::default(),
        );
        for (index, (track, expected)) in tracks.tracks.iter().zip(&expected.tracks).enumerate() {
            #[allow(clippy::float_cmp)]
            let same = track.base == expected.base
                && track.growth_limit == expected.growth_limit
                && track.position == expected.position;
            debug_assert!(
                same,
                "grid-lanes track {index}: aggregated {} / {} at {}, per position {} / {} at {}",
                track.base,
                track.growth_limit,
                track.position,
                expected.base,
                expected.growth_limit,
                expected.position,
            );
        }
    }
}

struct PlacedItem<N> {
    node: N,
    layout: Layout,
    stacking_start: f32,
    stacking_extent: f32,
    stacking_bias: f32,
    first_track: usize,
    last_track: usize,
    first_in_track: bool,
    last_in_track: bool,
    baseline: Option<f32>,
    last_baseline: Option<f32>,
    relative: Point<f32>,
    overflow: Point<Overflow>,
    sticky_area: Option<Edges<f32>>,
}

#[allow(clippy::struct_excessive_bools)]
struct LanesContext<'a> {
    grid_axis: Axis,
    tracks: &'a TrackSet,
    inner_grid: f32,
    content_origin: Point<f32>,
    stacking_gap: f32,
    stacking_basis: Option<f32>,
    tolerance: f32,
    rtl: bool,
    goal: LayoutGoal,
    all_tracks_fixed: bool,
    reads_last_baseline: bool,
    reads_first_baseline: bool,
}

struct LanesPass<N> {
    items: Vec<PlacedItem<N>>,
    stacking_range: f32,
}

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
            AxisPlacement::Definite(span) => (
                usize::try_from(span.start - first_line)
                    .expect("the implicit grid covers every definite span"),
                usize::try_from(span.end - span.start)
                    .expect("a resolved span covers at least one track"),
                true,
            ),
            AxisPlacement::Indefinite { span } => {
                let span = span.min(count).max(1);
                let start = choose_lane(
                    &running,
                    &context.tracks.tracks,
                    span,
                    cursor,
                    context.tolerance,
                );
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
        let containing_block = axis.pack(Some(track_area), context.stacking_basis);
        let area = axis.pack(Some(track_area), None);
        refresh_item_basis(tree, item, containing_block);
        item.clear_contribution_cache(Axis::Horizontal);
        item.clear_contribution_cache(Axis::Vertical);
        resolve_item_intrinsic_dimensions(tree, state, item, axis, None, containing_block);
        resolve_item_intrinsic_dimensions(
            tree,
            state,
            item,
            stacking,
            Some(CrossAxisTracks::resolved(context.tracks)),
            containing_block,
        );
        let (known, available) = item_area_geometry(item, area);
        let item_goal = match context.goal.independence() {
            None => context.goal,
            Some(container) => {
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
                stacking.set_size(&mut independent, false);
                LayoutGoal::Commit {
                    content_independent: independent,
                }
            }
        };
        let output = tree.compute_layout(
            state,
            item.key.node,
            LayoutInput::new(item_goal, known, containing_block, available)
                .reading_last_baseline(context.reads_last_baseline && !item_goal.commits())
                .reading_first_baseline(context.reads_first_baseline && !item_goal.commits()),
        );

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

        let position = running[start..start + span]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let stacking_extent = (stacking.size(output.size) + stacking.sum(margin)).max(0.0);
        for lane in &mut running[start..start + span] {
            *lane = position + stacking_extent + context.stacking_gap;
        }
        stacking_range = stacking_range.max(position + stacking_extent);
        let first_in_track = used[start..start + span].iter().any(|&value| !value);
        for lane in &mut used[start..start + span] {
            *lane = true;
        }

        let relative = relative_item_offset(tree, item, containing_block);
        let mut layout = Layout::with_order(item.key.layout_order);
        layout.size = output.size;
        layout.content_size = output.content_size;
        layout.border = item.border;
        layout.padding = item.padding;
        layout.margin = margin;
        let sticky_area = (item.position == PositionProperty::Sticky).then(|| {
            let mut containing = Edges::ZERO;
            let area_start = axis.point(context.content_origin) + grid_start;
            axis.set_start(&mut containing, area_start);
            axis.set_end(&mut containing, area_start + track_area);
            containing
        });
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
            sticky_area,
            stacking_start: position,
            stacking_extent,
            stacking_bias: stacking.point(context.content_origin)
                + stacking.start(margin)
                + stacking.point(relative),
            first_track: start,
            last_track: start + span - 1,
            first_in_track,
            last_in_track: false,
            baseline: output.first_baseline,
            last_baseline: output.last_baseline,
            relative,
            overflow: item.overflow,
        });
    }
    used.fill(false);
    for item in placed.iter_mut().rev() {
        let tracks = item.first_track..=item.last_track;
        item.last_in_track = used[tracks.clone()].iter().any(|&value| !value);
        used[tracks].fill(true);
    }

    LanesPass {
        items: placed,
        stacking_range,
    }
}

fn lanes_baselines<N>(items: &[PlacedItem<N>], grid_axis: Axis) -> [Option<f32>; 2] {
    let at = |item: &PlacedItem<N>, baseline: Option<f32>| {
        baseline.map(|baseline| item.layout.location.y - item.relative.y + baseline)
    };
    match grid_axis {
        Axis::Horizontal => [
            items
                .iter()
                .filter(|item| item.first_in_track)
                .filter_map(|item| at(item, item.baseline))
                .reduce(f32::min),
            items
                .iter()
                .filter(|item| item.last_in_track)
                .filter_map(|item| at(item, item.last_baseline))
                .reduce(f32::max),
        ],
        Axis::Vertical => [
            items
                .iter()
                .filter(|item| item.baseline.is_some())
                .min_by_key(|item| item.first_track)
                .and_then(|item| at(item, item.baseline)),
            items
                .iter()
                .filter(|item| item.last_baseline.is_some())
                .max_by_key(|item| item.last_track)
                .and_then(|item| at(item, item.last_baseline)),
        ],
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
    let containment = style.containment();
    let contained = contained_axes(containment, &style);
    let layout_contained = containment.contains(Contain::LAYOUT);
    let scroll_container = is_scroll_container(style.overflow());
    let gap_value = style.gap();
    let rtl = style.direction() == direction::T::Rtl;
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
    let justify_items = normalize_item_alignment(style.justify_items().computed.0.0, true, rtl);
    let item_defaults = ItemDefaults {
        align_items: align_items.unwrap_or(AlignFlags::STRETCH),
        align_items_normal: align_items.is_none(),
        justify_items: justify_items.unwrap_or(AlignFlags::STRETCH),
        justify_items_normal: justify_items.is_none(),
        rtl,
    };

    let ContainerPrologue {
        metrics,
        fit_content_width,
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
    range.end = range
        .end
        .max(range.start.saturating_add(line_offset(largest_auto_span)))
        .min(GRID_LINE_LIMIT);
    let occupied = if explicit.auto_fit.is_some() {
        auto_fit_occupancy(range, &placements, auto_span_total)
    } else {
        Vec::new()
    };
    let auto_tracks = &match grid_axis {
        Axis::Horizontal => style.grid_auto_columns(),
        Axis::Vertical => style.grid_auto_rows(),
    }
    .0;
    let (first_line, specs) = build_axis_tracks(
        &explicit,
        &auto_tracks[..auto_tracks.len().min(MAX_MATERIALIZED_TRACKS)],
        range,
        &occupied,
    );

    let stacking_basis = stacking_axis.size(percentage_basis);
    let item_basis = grid_axis.pack(None, stacking_basis);
    let mut items = in_flow
        .iter()
        .map(|pending| {
            let key = pending.key();
            let mut item = resolve_grid_item(
                &tree.style(key.node),
                key,
                GridArea::default(),
                item_basis,
                item_defaults,
            );
            for alignment in [&mut item.align_self, &mut item.justify_self] {
                if *alignment == AlignFlags::BASELINE {
                    *alignment = AlignFlags::START;
                } else if *alignment == AlignFlags::LAST_BASELINE {
                    *alignment = AlignFlags::END;
                }
            }
            item
        })
        .collect::<Vec<_>>();

    let mut tracks = TrackSet {
        first_coordinate: first_line,
        ..TrackSet::default()
    };
    let mut scratch = IntrinsicSizingScratch::default();
    let mut items_fresh = true;
    let grid_basis = grid_axis.size(percentage_basis);
    let grid_alignment = grid_axis.size(content_alignment);
    let decided_grid = fit_content_width
        .filter(|_| grid_axis == Axis::Horizontal)
        .map(|stretch_fit| {
            let content = contained.width().unwrap_or_else(|| {
                fit_content_inner_size(stretch_fit, |constraint| {
                    size_grid_axis(
                        tree,
                        state,
                        grid_axis,
                        &mut tracks,
                        &specs,
                        &mut items,
                        core::mem::take(&mut items_fresh),
                        &placements,
                        percentage_basis,
                        grid_axis.size(initial_gap),
                        constraint,
                        grid_alignment,
                        &mut scratch,
                    );
                    tracks.used_size()
                })
            });
            clamped_inner_axis(&metrics, grid_axis, content)
        });
    let grid_content = decided_grid.unwrap_or_else(|| {
        size_grid_axis(
            tree,
            state,
            grid_axis,
            &mut tracks,
            &specs,
            &mut items,
            core::mem::take(&mut items_fresh),
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
        grid_axis
            .size(contained.extents())
            .unwrap_or_else(|| tracks.used_size())
    });
    let grid_outer = final_outer_axis(&metrics, grid_axis, grid_content);
    let grid_inner = (grid_outer - grid_axis.size(metrics.box_inset)).max(0.0);
    let grid_gap = resolve_gap_axis(grid_axis.size(gap_value), Some(grid_inner));
    #[allow(
        clippy::float_cmp,
        reason = "the rerun is keyed on the used gutter being a different value, not a near one"
    )]
    let needs_definite_rerun =
        decided_grid.is_some() || grid_basis.is_none() || grid_gap != grid_axis.size(initial_gap);
    if needs_definite_rerun {
        size_grid_axis(
            tree,
            state,
            grid_axis,
            &mut tracks,
            &specs,
            &mut items,
            core::mem::take(&mut items_fresh),
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
        !track.collapsed
            && (track.min.is_intrinsic() || track.max.is_intrinsic() || track.is_flexible())
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
        inner_grid: grid_inner,
        content_origin: Point::new(
            metrics.border.left + metrics.padding.left,
            metrics.border.top + metrics.padding.top,
        ),
        stacking_gap: stacking_axis.size(initial_gap),
        stacking_basis,
        tolerance: tie_threshold(style.fit_tolerance(), style.font_size(), grid_inner),
        rtl,
        goal: container_goal,
        all_tracks_fixed,
        reads_last_baseline: input.reads_last_baseline && !input.goal.commits(),
        reads_first_baseline: input.reads_first_baseline && !input.goal.commits(),
    };
    let stacking_content = |range: f32| stacking_axis.size(contained.extents()).unwrap_or(range);
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

    let stacking_reverse = rtl && stacking_axis == Axis::Horizontal;
    let mut content_size = Size::ZERO;
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
            item_end_margin(scroll_container, placed.layout.margin),
        );
    }
    let [first_baseline, last_baseline] = if layout_contained {
        [None, None]
    } else {
        lanes_baselines(&pass.items, grid_axis)
    };
    if commits_layout {
        for placed in pass.items {
            let start = stacking_axis.start(metrics.border) + stacking_axis.start(metrics.padding);
            let end = if scroll_container {
                let scrolling_end =
                    content_size.zip_map(padding_box_end(outer_size, metrics.border), f32::max);
                stacking_axis.size(scrolling_end) - stacking_axis.end(metrics.padding)
            } else {
                start + stacking_inner
            };
            let sticky_area = placed.sticky_area.map(|mut containing| {
                stacking_axis.set_start(&mut containing, start);
                stacking_axis.set_end(&mut containing, end);
                containing
            });
            tree.set_unrounded_layout(state, placed.node, placed.layout);
            tree.set_sticky_containing_block(state, placed.node, sticky_area);
        }
        for (document_index, child) in hidden.expect("commit keeps hidden grid-lanes items") {
            hide_subtree(tree, state, child);
            tree.set_unrounded_layout(
                state,
                child,
                Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
            );
        }
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
        let explicit_lines = grid_axis.pack(explicit_len, 1);
        let absolute = absolute.expect("commit keeps out-of-flow grid-lanes items");
        let absolute_content_size =
            if absolute.is_empty() && !tree.has_hoisted_children(state, node) {
                Size::ZERO
            } else {
                layout_absolute_items(
                    tree,
                    state,
                    node,
                    &absolute,
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
                    crate::compute::scrollable_containing_block(
                        scroll_container,
                        outer_size,
                        metrics.border,
                        content_size,
                    ),
                )
            };
        content_size = content_size.zip_map(absolute_content_size, f32::max);
    }
    let content_size = own_scrollable_overflow(
        layout_contained,
        scroll_container,
        outer_size,
        metrics.border,
        content_size,
    );
    LayoutOutput::new(outer_size, content_size)
        .with_first_baseline(first_baseline)
        .with_last_baseline(last_baseline)
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
        assert_eq!(tie_threshold(&FitTolerance::Normal, 18.0, 200.0), 18.0);
        assert_eq!(
            tie_threshold(&FitTolerance::Infinite, 18.0, 200.0),
            f32::INFINITY
        );
        let length = FitTolerance::LengthPercentage(NonNegative(LengthPercentage::new_length(
            Length::new(7.0),
        )));
        assert_eq!(tie_threshold(&length, 18.0, 200.0), 7.0);
        let percentage = FitTolerance::LengthPercentage(NonNegative(
            LengthPercentage::new_percent(Percentage(0.25)),
        ));
        assert_eq!(tie_threshold(&percentage, 18.0, 200.0), 50.0);
        assert_eq!(tie_threshold(&percentage, 18.0, 0.0), 0.0);
    }

    #[test]
    fn auto_fit_occupancy_keeps_definite_tracks_and_one_track_per_auto_span() {
        let range = TrackSpan { start: 0, end: 6 };
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
        assert_eq!(
            auto_fit_occupancy(range, &[definite(2, 4)], 0),
            vec![false, false, true, true, false, false]
        );
        assert_eq!(
            auto_fit_occupancy(
                TrackSpan { start: -1, end: 2 },
                &[definite(-1, 0), AxisPlacement::Indefinite { span: 2 }],
                2,
            ),
            vec![true, true, true]
        );
    }

    fn lanes(collapsed: &[bool]) -> Vec<Track> {
        collapsed
            .iter()
            .map(|&collapsed| Track {
                collapsed,
                ..Track::default()
            })
            .collect()
    }

    #[test]
    fn choose_lane_takes_the_shortest_lane_then_the_cursor_within_tolerance() {
        let open = lanes(&[false, false, false]);
        assert_eq!(choose_lane(&[30.0, 10.0, 20.0], &open, 1, 0, 0.0), 1);
        assert_eq!(choose_lane(&[10.0, 10.0, 10.0], &open, 1, 2, 0.0), 2);
        assert_eq!(choose_lane(&[10.0, 10.0, 30.0], &open, 1, 2, 0.0), 0);
        assert_eq!(choose_lane(&[10.0, 20.0, 10.0], &open, 1, 1, 16.0), 1);
        assert_eq!(choose_lane(&[10.0, 20.0, 10.0], &open, 1, 1, 0.0), 2);
        assert_eq!(
            choose_lane(&[0.0, 50.0, 10.0, 0.0], &lanes(&[false; 4]), 2, 0, 0.0),
            2
        );
        assert_eq!(
            choose_lane(&[0.0, 0.0, 10.0], &lanes(&[false, true, false]), 1, 0, 0.0),
            0
        );
        assert_eq!(
            choose_lane(&[30.0, 0.0, 10.0], &lanes(&[false, true, false]), 1, 1, 0.0),
            2
        );
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
        assert_eq!(stacking_content_offset(-40.0, A::END), 0.0);
    }
}
