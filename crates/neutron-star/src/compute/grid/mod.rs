//! CSS Grid Layout (Level 2 wording, excluding `subgrid`).
//!
//! The implementation follows the specification's explicit-grid,
//! placement, track-sizing, alignment, and item-layout passes. Named lines
//! and areas are intentionally host-lowered to numeric lines by the public
//! protocol; writing modes, fragmentation, and subgrid are outside this
//! physical-axis engine's scope.
//!
//! All host interaction is statically dispatched through [`GridSource`].
//! Borrowed style iterators are consumed before the first mutable child
//! callback, durable storage remains host-owned, and all transient work is
//! bounded by the Grid §5.4 line limits.

#![allow(clippy::cast_precision_loss)]

mod alignment;
mod placement;
mod sizing;
mod tracks;
mod types;

use alignment::{align_tracks, alignment_spacing_from_free_space, item_alignment_offset};
use placement::{AxisPlacement, GridArea, PlacementInput, place_items, resolve_axis_placement};
use sizing::{
    CrossAxisTracks, initialize_tracks, probe_raw_min_content, resolve_item_intrinsic_dimensions,
    size_tracks,
};
use tracks::{ExpandedTemplate, MAX_MATERIALIZED_TRACKS, build_axis_tracks, expand_template};
use types::{Axis, GridItem, TrackSet};

use super::util::{
    ItemKey, OrderedItem, PendingLayoutItem, ResolvedContainerBox, ResolvedItemBox,
    apply_aspect_ratio, box_inset_size, clamp, clamp_axis, preferred_size_definiteness,
    resolve_container_box, resolve_gap, resolve_item_box, sort_and_assign_layout_order,
};
use super::{compute_absolute_layout, hide_subtree};
use crate::geometry::{Edges, Line, Point, Size};
use crate::style::value::{Dimension, LengthPercentageAuto};
use crate::style::{
    AlignContent, AlignItems, BoxGenerationMode, CoreStyle, Direction, GridContainerStyle,
    GridItemStyle, GridPlacement, Position,
};
use crate::tree::{
    AvailableSpace, GridSource, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutSession,
    NodeId, RequestedAxis, SizingMode,
};

#[derive(Debug, Clone, Copy)]
struct ItemDefaults {
    align_items: AlignItems,
    align_items_normal: bool,
    justify_items: AlignItems,
}

/// Compact order/classification record retained before an item is resolved.
/// Raw item style remains in the immutable source and is reborrowed by each
/// sizing or positioned-layout pass.
#[derive(Debug, Clone, Copy)]
struct PendingItem {
    ordered: OrderedItem,
    position: Position,
    row: Line<GridPlacement>,
    column: Line<GridPlacement>,
}

impl PendingItem {
    #[inline]
    fn key(self) -> ItemKey {
        self.ordered.key()
    }
}

impl PendingLayoutItem for PendingItem {
    #[inline]
    fn ordered(&self) -> &OrderedItem {
        &self.ordered
    }

    #[inline]
    fn ordered_mut(&mut self) -> &mut OrderedItem {
        &mut self.ordered
    }
}

fn classify_item<Source: GridSource>(
    source: &Source,
    node: NodeId,
    document_index: usize,
) -> Option<PendingItem> {
    let style = source.grid_item_style(node);
    if style.box_generation_mode() == BoxGenerationMode::None {
        return None;
    }
    let position = style.position();
    Some(PendingItem {
        ordered: OrderedItem {
            node,
            document_index,
            css_order: if position == Position::Relative {
                GridItemStyle::order(&style)
            } else {
                0
            },
            layout_order: 0,
        },
        position,
        row: style.grid_row(),
        column: style.grid_column(),
    })
}

fn resolve_grid_item<Source: GridSource>(
    source: &Source,
    key: ItemKey,
    area: GridArea,
    percentage_basis: Size<Option<f32>>,
    defaults: ItemDefaults,
) -> GridItem {
    let style = source.grid_item_style(key.node);
    let ResolvedItemBox {
        raw_size,
        raw_min_size,
        aspect_ratio,
        box_sizing,
        overflow,
        preferred_size,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        scrollbar,
        inset,
        ..
    } = resolve_item_box(source, &style, percentage_basis);
    let behaves_auto_or_depends = |value: Dimension| {
        matches!(
            value,
            Dimension::Auto | Dimension::Percent(_) | Dimension::Calc(_)
        )
    };
    GridItem {
        key,
        area,
        align_self: style.align_self().unwrap_or_else(|| {
            if defaults.align_items_normal && aspect_ratio.is_some() {
                AlignItems::Start
            } else {
                defaults.align_items
            }
        }),
        justify_self: style.justify_self().unwrap_or(defaults.justify_items),
        direction: style.direction(),
        aspect_ratio,
        box_sizing,
        overflow,
        preferred_behaves_auto_or_depends: raw_size.map(behaves_auto_or_depends),
        minimum_is_auto: raw_min_size.map(Dimension::is_auto),
        preferred_size,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        scrollbar,
        inset,
        raw_min_content: Size::NONE,
        raw_max_content: Size::NONE,
        minimum_contribution: Size::NONE,
        min_content_contribution: Size::NONE,
        max_content_contribution: Size::NONE,
        measured_baselines: Point::NONE,
        baseline_shim: 0.0,
    }
}

fn expand_explicit_tracks<Source: GridSource>(
    source: &Source,
    node: NodeId,
    repeat_max_basis: Size<Option<f32>>,
    repeat_min_basis: Size<Option<f32>>,
    gap: Size<f32>,
) -> (ExpandedTemplate, ExpandedTemplate) {
    let style = source.grid_container_style(node);
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    let columns = expand_template(
        style.grid_template_columns(),
        repeat_max_basis.width,
        repeat_min_basis.width,
        gap.width,
        &resolve_calc,
    );
    let rows = expand_template(
        style.grid_template_rows(),
        repeat_max_basis.height,
        repeat_min_basis.height,
        gap.height,
        &resolve_calc,
    );
    (columns, rows)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_track_sizing<Source, Session>(
    source: &Source,
    session: &mut Session,
    column_specs: &[tracks::AxisTrackSpec],
    row_specs: &[tracks::AxisTrackSpec],
    items: &mut [GridItem],
    inner_basis: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    gap: Size<f32>,
    justify_content: AlignContent,
    align_content: AlignContent,
) -> (TrackSet, TrackSet)
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    for item in items.iter_mut() {
        refresh_item_basis(source, item, Size::NONE);
        item.clear_contribution_cache(Axis::Horizontal);
        item.clear_contribution_cache(Axis::Vertical);
    }
    // Grid §12.1 sizes columns before rows. During that first column pass,
    // an item sees each row with a definite max track sizing function at
    // that maximum and every other row as infinite. Only when the container
    // and every row are definite does content alignment affect this estimate.
    let mut rows = initialize_tracks(source, row_specs, inner_basis.height, gap.height);
    let all_rows_definite = rows
        .tracks
        .iter()
        .all(|track| track.growth_limit.is_finite());
    let distributed_gap = if all_rows_definite {
        inner_basis.height.map_or(0.0, |height| {
            let visible = rows.tracks.iter().filter(|track| !track.collapsed).count();
            let used = rows
                .tracks
                .iter()
                .filter(|track| !track.collapsed)
                .map(|track| track.growth_limit)
                .sum::<f32>()
                + rows.total_gap();
            alignment_spacing_from_free_space(height - used, visible, align_content).1
        })
    } else {
        0.0
    };
    let initial_column_cross_tracks = CrossAxisTracks::DefiniteMaximums {
        tracks: &rows,
        distributed_gap,
    };
    let mut columns = initialize_tracks(source, column_specs, inner_basis.width, gap.width);
    size_tracks(
        source,
        session,
        Axis::Horizontal,
        &mut columns,
        Some(initial_column_cross_tracks),
        items,
        inner_basis,
        inner_basis
            .width
            .map_or(available.width, AvailableSpace::Definite),
        justify_content,
    );
    if let Some(width) = inner_basis.width {
        align_tracks(&mut columns, width, justify_content);
    }

    // Record the pre-row min-content probes only for items that can affect a
    // content/flex-sized column and whose inline size is not already fixed.
    // Comparing these after row sizing detects descendant ratios, wrapped
    // column flexboxes, and other cross-size dependencies without rerunning
    // every intrinsic grid unconditionally.
    let needs_column_feedback = columns
        .tracks
        .iter()
        .any(|track| track.intrinsic_min || track.intrinsic_max || track.is_flexible());
    let mut before_row_min_content = Vec::new();
    if needs_column_feedback {
        before_row_min_content.reserve(items.len());
        for item in items.iter_mut() {
            let range = columns.span_indices(item.area.column.start, item.area.column.end);
            let affects_intrinsic_column = columns.tracks[range]
                .iter()
                .any(|track| track.intrinsic_min || track.intrinsic_max || track.is_flexible());
            before_row_min_content.push(
                (affects_intrinsic_column && item.preferred_behaves_auto_or_depends.width).then(
                    || {
                        probe_raw_min_content(
                            source,
                            session,
                            item,
                            Axis::Horizontal,
                            Some(initial_column_cross_tracks),
                            inner_basis,
                        )
                    },
                ),
            );
        }
    }
    // Grid-item percentages use the grid area, not the whole container. The
    // inline area is now definite, so resolve width-dependent padding,
    // margins, sizes, and aspect-ratio inputs before row contributions.
    for item in items.iter_mut() {
        let width = columns.area_size(item.area.column.start, item.area.column.end);
        refresh_item_basis(source, item, Size::new(Some(width), None));
        item.clear_contribution_cache(Axis::Vertical);
    }
    let mut row_basis = inner_basis;
    row_basis.width = row_basis.width.or(Some(columns.used_size()));
    size_tracks(
        source,
        session,
        Axis::Vertical,
        &mut rows,
        Some(CrossAxisTracks::resolved(&columns)),
        items,
        row_basis,
        inner_basis
            .height
            .map_or(available.height, AvailableSpace::Definite),
        align_content,
    );
    if let Some(height) = inner_basis.height {
        align_tracks(&mut rows, height, align_content);
    }

    let mut column_feedback_changed = false;
    if needs_column_feedback {
        for (item, before) in items.iter_mut().zip(before_row_min_content) {
            let Some(before) = before else {
                continue;
            };
            item.clear_contribution_cache(Axis::Horizontal);
            let after = probe_raw_min_content(
                source,
                session,
                item,
                Axis::Horizontal,
                Some(CrossAxisTracks::resolved(&rows)),
                inner_basis,
            );
            let tolerance = f32::EPSILON * before.abs().max(after.abs()).max(1.0);
            column_feedback_changed |= (before - after).abs() > tolerance;
        }
    }

    // Cross-size-sensitive content can change an inline contribution once
    // row sizes are known. Grid bounds this feedback to a single
    // columns→rows rerun.
    if column_feedback_changed {
        for item in items.iter_mut() {
            let width = columns.area_size(item.area.column.start, item.area.column.end);
            let height = rows.area_size(item.area.row.start, item.area.row.end);
            refresh_item_basis(source, item, Size::new(Some(width), Some(height)));
        }
        for item in items.iter_mut() {
            item.clear_contribution_cache(Axis::Horizontal);
        }
        let mut rerun_columns =
            initialize_tracks(source, column_specs, inner_basis.width, gap.width);
        size_tracks(
            source,
            session,
            Axis::Horizontal,
            &mut rerun_columns,
            Some(CrossAxisTracks::resolved(&rows)),
            items,
            inner_basis,
            inner_basis
                .width
                .map_or(available.width, AvailableSpace::Definite),
            justify_content,
        );
        if let Some(width) = inner_basis.width {
            align_tracks(&mut rerun_columns, width, justify_content);
        }
        columns = rerun_columns;
        for item in items.iter_mut() {
            let width = columns.area_size(item.area.column.start, item.area.column.end);
            let height = rows.area_size(item.area.row.start, item.area.row.end);
            refresh_item_basis(source, item, Size::new(Some(width), Some(height)));
            item.clear_contribution_cache(Axis::Vertical);
        }
        let mut rerun_rows = initialize_tracks(source, row_specs, inner_basis.height, gap.height);
        let mut final_row_basis = inner_basis;
        final_row_basis.width = final_row_basis.width.or(Some(columns.used_size()));
        size_tracks(
            source,
            session,
            Axis::Vertical,
            &mut rerun_rows,
            Some(CrossAxisTracks::resolved(&columns)),
            items,
            final_row_basis,
            inner_basis
                .height
                .map_or(available.height, AvailableSpace::Definite),
            align_content,
        );
        if let Some(height) = inner_basis.height {
            align_tracks(&mut rerun_rows, height, align_content);
        }
        rows = rerun_rows;
    }
    (columns, rows)
}

fn final_outer_size(metrics: &ResolvedContainerBox, tracks: Size<f32>) -> Size<f32> {
    Size::new(
        metrics.outer.width.unwrap_or_else(|| {
            clamp_axis(
                tracks.width + metrics.box_inset.width,
                metrics.min.width,
                metrics.max.width,
                metrics.box_inset.width,
            )
        }),
        metrics.outer.height.unwrap_or_else(|| {
            clamp_axis(
                tracks.height + metrics.box_inset.height,
                metrics.min.height,
                metrics.max.height,
                metrics.box_inset.height,
            )
        }),
    )
}

#[derive(Debug)]
struct PendingBaselineItem {
    node: NodeId,
    area_row: i32,
    area_column: i32,
    align_baseline: bool,
    area_top: f32,
    layout: Layout,
    baseline: Option<f32>,
}

fn refresh_item_basis<Source: GridSource>(
    source: &Source,
    item: &mut GridItem,
    percentage_basis: Size<Option<f32>>,
) {
    let style = source.grid_item_style(item.key.node);
    let ResolvedItemBox {
        preferred_size,
        min_size,
        max_size,
        margin,
        margin_auto,
        padding,
        border,
        scrollbar,
        inset,
        ..
    } = resolve_item_box(source, &style, percentage_basis);
    item.preferred_size = preferred_size;
    item.min_size = min_size;
    item.max_size = max_size;
    item.margin = margin;
    item.margin_auto = margin_auto;
    item.padding = padding;
    item.border = border;
    item.scrollbar = scrollbar;
    item.inset = inset;
}

fn physical_area(
    item: &GridItem,
    columns: &TrackSet,
    rows: &TrackSet,
    inner_size: Size<f32>,
    rtl: bool,
) -> (Point<f32>, Size<f32>) {
    let logical_left = columns.line_position(item.area.column.start);
    let width = columns.area_size(item.area.column.start, item.area.column.end);
    let logical_right = logical_left + width;
    let top = rows.line_position(item.area.row.start);
    let height = rows.area_size(item.area.row.start, item.area.row.end);
    let left = if rtl {
        inner_size.width - logical_right
    } else {
        logical_left
    };
    (
        Point::new(left, top),
        Size::new(width.max(0.0), height.max(0.0)),
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn layout_in_flow_items<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &mut [GridItem],
    columns: &TrackSet,
    rows: &TrackSet,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    outer_size: Size<f32>,
    goal: LayoutGoal,
    rtl: bool,
) -> (Size<f32>, Point<Option<f32>>)
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    let baseline_item_count = items
        .iter()
        .filter(|item| {
            item.align_self == AlignItems::Baseline
                && !item.margin_auto.top
                && !item.margin_auto.bottom
        })
        .count();
    let needs_baseline_pass = baseline_item_count != 0;
    let mut pending = Vec::with_capacity(baseline_item_count);
    let mut content_size = outer_size;
    let mut direct_baseline_candidate = None::<(i32, i32, f32)>;

    for item in items {
        let (area_offset, area_size) = physical_area(item, columns, rows, inner_size, rtl);
        refresh_item_basis(
            source,
            item,
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        item.clear_contribution_cache(Axis::Horizontal);
        item.clear_contribution_cache(Axis::Vertical);
        resolve_item_intrinsic_dimensions(
            source,
            session,
            item,
            Axis::Horizontal,
            Some(CrossAxisTracks::resolved(rows)),
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        resolve_item_intrinsic_dimensions(
            source,
            session,
            item,
            Axis::Vertical,
            Some(CrossAxisTracks::resolved(columns)),
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        let raw_size = source.grid_item_style(item.key.node).size();
        let mut known = item.preferred_size;
        let resolved_preferred = item.preferred_size;
        let intrinsic_width = matches!(
            raw_size.width,
            Dimension::MinContent | Dimension::MaxContent | Dimension::FitContent(_)
        );
        let intrinsic_height = matches!(
            raw_size.height,
            Dimension::MinContent | Dimension::MaxContent | Dimension::FitContent(_)
        );
        if intrinsic_width {
            known.width = None;
        }
        if intrinsic_height {
            known.height = None;
        }
        let horizontal_stretch = item.justify_self == AlignItems::Stretch
            && known.width.is_none()
            && !intrinsic_width
            && !item.margin_auto.left
            && !item.margin_auto.right;
        let vertical_stretch = item.align_self == AlignItems::Stretch
            && known.height.is_none()
            && !intrinsic_height
            && !item.margin_auto.top
            && !item.margin_auto.bottom;
        if horizontal_stretch {
            known.width = Some((area_size.width - item.margin.horizontal_sum()).max(0.0));
        }
        if vertical_stretch {
            known.height = Some((area_size.height - item.margin.vertical_sum()).max(0.0));
        }
        known = apply_aspect_ratio(known, item.aspect_ratio);
        known.width = known
            .width
            .map(|value| clamp(value, item.min_size.width, item.max_size.width));
        known.height = known
            .height
            .map(|value| clamp(value, item.min_size.height, item.max_size.height));

        let available = Size::new(
            if matches!(raw_size.width, Dimension::FitContent(_)) {
                resolved_preferred
                    .width
                    .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite)
            } else {
                known.width.map_or(AvailableSpace::MaxContent, |_| {
                    AvailableSpace::Definite(
                        (area_size.width - item.margin.horizontal_sum()).max(0.0),
                    )
                })
            },
            if matches!(raw_size.height, Dimension::FitContent(_)) {
                resolved_preferred
                    .height
                    .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite)
            } else {
                known.height.map_or(AvailableSpace::MaxContent, |_| {
                    AvailableSpace::Definite(
                        (area_size.height - item.margin.vertical_sum()).max(0.0),
                    )
                })
            },
        );
        let parent_size = Size::new(Some(area_size.width), Some(area_size.height));
        let input = match goal {
            LayoutGoal::Commit => LayoutInput::perform_layout(known, parent_size, available),
            LayoutGoal::Measure(requested) => {
                LayoutInput::compute_size(known, parent_size, available, requested)
            }
        };
        let output = session.compute_child_layout(source, item.key.node, input);

        let mut margin = item.margin;
        for axis in Axis::ALL {
            let area = axis.size(area_size);
            let child = axis.size(output.size);
            let auto_start = axis.start(item.margin_auto);
            let auto_end = axis.end(item.margin_auto);
            let fixed_start = axis.start(margin);
            let fixed_end = axis.end(margin);
            let auto_count = usize::from(auto_start) + usize::from(auto_end);
            if auto_count > 0 {
                let share = ((area - child - fixed_start - fixed_end).max(0.0)) / auto_count as f32;
                if auto_start {
                    match axis {
                        Axis::Horizontal => margin.left = share,
                        Axis::Vertical => margin.top = share,
                    }
                }
                if auto_end {
                    match axis {
                        Axis::Horizontal => margin.right = share,
                        Axis::Vertical => margin.bottom = share,
                    }
                }
            }
        }

        let free_x = area_size.width - output.size.width - margin.horizontal_sum();
        let free_y = area_size.height - output.size.height - margin.vertical_sum();
        // Positive free space has already been consumed by auto margins, so
        // these offsets are zero in that case. On overflow auto margins are
        // zero and self-alignment still applies (Grid §11.2).
        let item_rtl = item.direction == Direction::Rtl;
        let offset_x = item_alignment_offset(free_x, item.justify_self, rtl, item_rtl);
        let offset_y = item_alignment_offset(free_y, item.align_self, false, false);
        let relative_x = item
            .inset
            .left
            .unwrap_or_else(|| -item.inset.right.unwrap_or(0.0));
        let relative_y = item
            .inset
            .top
            .unwrap_or_else(|| -item.inset.bottom.unwrap_or(0.0));
        let location = Point::new(
            content_origin.x + area_offset.x + margin.left + offset_x + relative_x,
            content_origin.y + area_offset.y + margin.top + offset_y + relative_y,
        );
        let mut layout = Layout::with_order(item.key.layout_order);
        layout.location = location;
        layout.size = output.size;
        layout.content_size = output.content_size;
        layout.scrollbar_size = item.scrollbar;
        layout.border = item.border;
        layout.padding = item.padding;
        layout.margin = margin;
        let item_baseline = output
            .first_baselines
            .y
            .or_else(|| (item.align_self == AlignItems::Baseline).then_some(output.size.height));
        item.measured_baselines = Point::new(output.first_baselines.x, item_baseline);
        let participates_in_baseline = item.align_self == AlignItems::Baseline
            && !item.margin_auto.top
            && !item.margin_auto.bottom;
        if !participates_in_baseline {
            let candidate = (
                item.area.row.start,
                item.area.column.start,
                layout.location.y + item_baseline.unwrap_or(output.size.height),
            );
            if direct_baseline_candidate
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                direct_baseline_candidate = Some(candidate);
            }
            content_size.width = content_size
                .width
                .max(layout.location.x + layout.size.width.max(layout.content_size.width));
            content_size.height = content_size
                .height
                .max(layout.location.y + layout.size.height.max(layout.content_size.height));
            if goal == LayoutGoal::Commit {
                session.set_unrounded_layout(item.key.node, &layout);
            }
            continue;
        }
        pending.push(PendingBaselineItem {
            node: item.key.node,
            area_row: item.area.row.start,
            area_column: item.area.column.start,
            align_baseline: item.align_self == AlignItems::Baseline
                && !item.margin_auto.top
                && !item.margin_auto.bottom,
            area_top: content_origin.y + area_offset.y,
            layout,
            baseline: item_baseline,
        });
    }

    if !needs_baseline_pass {
        return (
            content_size,
            Point::new(None, direct_baseline_candidate.map(|candidate| candidate.2)),
        );
    }

    // First-baseline sharing groups are row-local. Applying the largest
    // ascent after every child is measured avoids order-dependent results.
    let mut baseline_candidates = Vec::<(i32, f32)>::new();
    for item in pending.iter().filter(|item| item.align_baseline) {
        let Some(baseline) = item.baseline else {
            continue;
        };
        let ascent = item.layout.location.y + baseline - item.area_top;
        baseline_candidates.push((item.area_row, ascent));
    }
    baseline_candidates.sort_unstable_by_key(|&(row, _)| row);
    let mut baseline_groups = Vec::<(i32, f32)>::with_capacity(baseline_candidates.len());
    for (row, ascent) in baseline_candidates {
        if let Some((last_row, maximum)) = baseline_groups.last_mut()
            && *last_row == row
        {
            *maximum = maximum.max(ascent);
            continue;
        }
        baseline_groups.push((row, ascent));
    }
    for item in pending.iter_mut().filter(|item| item.align_baseline) {
        if let (Some(baseline), Ok(index)) = (
            item.baseline,
            baseline_groups.binary_search_by_key(&item.area_row, |&(row, _)| row),
        ) {
            let target = baseline_groups[index].1;
            let ascent = item.layout.location.y + baseline - item.area_top;
            item.layout.location.y += target - ascent;
        }
    }

    // Grid §11.6 selects from the first non-empty row, not the globally
    // smallest exposed child baseline. Prefer that row's baseline-sharing
    // group, otherwise use the first item in grid order and synthesize from
    // its bottom border edge when necessary.
    let first_row = pending
        .iter()
        .map(|item| item.area_row)
        .chain(direct_baseline_candidate.map(|candidate| candidate.0))
        .min();
    let first_baseline = first_row.and_then(|first_row| {
        pending
            .iter()
            .find(|item| item.area_row == first_row && item.align_baseline)
            .and_then(|item| {
                item.baseline
                    .map(|baseline| item.layout.location.y + baseline)
            })
            .or_else(|| {
                let pending_candidate = pending
                    .iter()
                    .filter(|item| item.area_row == first_row)
                    .min_by_key(|item| item.area_column)
                    .map(|item| {
                        (
                            item.area_column,
                            item.layout.location.y
                                + item.baseline.unwrap_or(item.layout.size.height),
                        )
                    });
                let direct_candidate = direct_baseline_candidate
                    .filter(|candidate| candidate.0 == first_row)
                    .map(|candidate| (candidate.1, candidate.2));
                match (pending_candidate, direct_candidate) {
                    (Some(pending), Some(direct)) => Some(if pending.0 <= direct.0 {
                        pending.1
                    } else {
                        direct.1
                    }),
                    (Some(pending), None) => Some(pending.1),
                    (None, Some(direct)) => Some(direct.1),
                    (None, None) => None,
                }
            })
    });
    for item in pending {
        content_size.width = content_size.width.max(
            item.layout.location.x + item.layout.size.width.max(item.layout.content_size.width),
        );
        content_size.height = content_size.height.max(
            item.layout.location.y + item.layout.size.height.max(item.layout.content_size.height),
        );
        if goal == LayoutGoal::Commit {
            session.set_unrounded_layout(item.node, &item.layout);
        }
    }
    (content_size, Point::new(None, first_baseline))
}

fn absolute_axis_lines(
    placement: Line<GridPlacement>,
    explicit_tracks: usize,
) -> (Option<i32>, Option<i32>) {
    // Unlike in-flow placement, two definite lines on an absolutely
    // positioned item are not reordered when the end precedes the start.
    // Grid §10.1 makes that a zero-sized containing block at the start line.
    // Resolve the two line coordinates independently so the subsequent area
    // calculation can retain that start edge.
    if matches!(placement.start, GridPlacement::Line(_))
        && matches!(placement.end, GridPlacement::Line(_))
    {
        let start = match resolve_axis_placement(
            Line::new(placement.start, GridPlacement::Span(1)),
            explicit_tracks,
        ) {
            AxisPlacement::Definite(span) => Some(span.start),
            AxisPlacement::Indefinite { .. } => None,
        };
        let end = match resolve_axis_placement(
            Line::new(GridPlacement::Span(1), placement.end),
            explicit_tracks,
        ) {
            AxisPlacement::Definite(span) => Some(span.end),
            AxisPlacement::Indefinite { .. } => None,
        };
        return (start, end);
    }
    if !matches!(placement.start, GridPlacement::Auto)
        && !matches!(placement.end, GridPlacement::Auto)
    {
        return match resolve_axis_placement(placement, explicit_tracks) {
            AxisPlacement::Definite(span) => (Some(span.start), Some(span.end)),
            AxisPlacement::Indefinite { .. } => (None, None),
        };
    }

    let start = match placement.start {
        GridPlacement::Line(_) => match resolve_axis_placement(
            Line::new(placement.start, GridPlacement::Span(1)),
            explicit_tracks,
        ) {
            AxisPlacement::Definite(span) => Some(span.start),
            AxisPlacement::Indefinite { .. } => None,
        },
        GridPlacement::Auto | GridPlacement::Span(_) => None,
    };
    let end = match placement.end {
        GridPlacement::Line(_) => match resolve_axis_placement(
            Line::new(GridPlacement::Span(1), placement.end),
            explicit_tracks,
        ) {
            AxisPlacement::Definite(span) => Some(span.end),
            AxisPlacement::Indefinite { .. } => None,
        },
        GridPlacement::Auto | GridPlacement::Span(_) => None,
    };
    (start, end)
}

fn absolute_axis_area(
    tracks: &TrackSet,
    placement: Line<GridPlacement>,
    explicit_tracks: usize,
    content_size: f32,
    padding_box_size: f32,
    content_start_inset: f32,
    reverse: bool,
) -> (f32, f32) {
    let (start, end) = absolute_axis_lines(placement, explicit_tracks);
    let range_start = tracks.first_coordinate;
    let range_end = tracks.first_coordinate
        + i32::try_from(tracks.tracks.len()).expect("grid tracks are clamped");
    let content_end_inset = (padding_box_size - content_start_inset - content_size).max(0.0);
    let scrollable_padding_end = padding_box_size
        .max(content_start_inset + tracks.end_line_position(range_end) + content_end_inset);
    let logical_start = start
        .filter(|line| (range_start..=range_end).contains(line))
        .map_or(0.0, |line| content_start_inset + tracks.line_position(line));
    let logical_end = end
        .filter(|line| (range_start..=range_end).contains(line))
        .map_or(scrollable_padding_end, |line| {
            content_start_inset + tracks.end_line_position(line)
        });
    let size = (logical_end - logical_start).max(0.0);
    let physical_start = if reverse {
        padding_box_size - logical_end
    } else {
        logical_start
    };
    debug_assert!(content_size <= padding_box_size + f32::EPSILON);
    (physical_start, size)
}

fn absolute_static_offset<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &GridItem,
    containing_size: Size<f32>,
    rtl: bool,
) -> Point<f32>
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    let basis = Size::new(Some(containing_size.width), Some(containing_size.height));
    let raw_size = source.grid_item_style(item.key.node).size();
    let intrinsic_available =
        raw_size.zip_map(containing_size, |dimension, available| match dimension {
            Dimension::MinContent => AvailableSpace::MinContent,
            Dimension::MaxContent => AvailableSpace::MaxContent,
            Dimension::Length(_)
            | Dimension::Percent(_)
            | Dimension::Calc(_)
            | Dimension::Auto
            | Dimension::FitContent(_) => AvailableSpace::Definite(available),
        });
    let output = session.compute_child_layout(
        source,
        item.key.node,
        LayoutInput::compute_size(
            item.preferred_size,
            basis,
            intrinsic_available,
            RequestedAxis::Both,
        ),
    );
    let item_floor = box_inset_size(item.padding, item.border, item.scrollbar);
    let used_size = Size::new(
        clamp_axis(
            item.preferred_size.width.unwrap_or(output.size.width),
            item.min_size.width,
            item.max_size.width,
            item_floor.width,
        ),
        clamp_axis(
            item.preferred_size.height.unwrap_or(output.size.height),
            item.min_size.height,
            item.max_size.height,
            item_floor.height,
        ),
    );
    let margin_box = Size::new(
        used_size.width + item.margin.horizontal_sum(),
        used_size.height + item.margin.vertical_sum(),
    );
    let auto_margin_offset = |axis: Axis| {
        let available = axis.size(containing_size) - axis.size(used_size);
        let start_auto = axis.start(item.margin_auto);
        let end_auto = axis.end(item.margin_auto);
        let fixed_start = axis.start(item.margin);
        let fixed_end = axis.end(item.margin);
        let count = usize::from(start_auto) + usize::from(end_auto);
        (count > 0).then(|| {
            let share = ((available - fixed_start - fixed_end).max(0.0)) / count as f32;
            if start_auto { share } else { 0.0 }
        })
    };
    Point::new(
        auto_margin_offset(Axis::Horizontal).unwrap_or_else(|| {
            item_alignment_offset(
                containing_size.width - margin_box.width,
                item.justify_self,
                rtl,
                item.direction == Direction::Rtl,
            )
        }),
        auto_margin_offset(Axis::Vertical).unwrap_or_else(|| {
            item_alignment_offset(
                containing_size.height - margin_box.height,
                item.align_self,
                false,
                false,
            )
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_absolute_items<Source, Session>(
    source: &Source,
    session: &mut Session,
    items: &[PendingItem],
    columns: &TrackSet,
    rows: &TrackSet,
    explicit_columns: usize,
    explicit_rows: usize,
    inner_size: Size<f32>,
    outer_size: Size<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
    scrollbar: Size<f32>,
    rtl: bool,
    defaults: ItemDefaults,
) -> Size<f32>
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    let mut content_size = outer_size;
    let padding_box_origin = Point::new(border.left, border.top);
    let padding_box_size = Size::new(
        (outer_size.width - border.horizontal_sum()).max(0.0),
        (outer_size.height - border.vertical_sum()).max(0.0),
    );
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let logical_content_start = Point::new(
        if rtl {
            padding.right + scrollbar.width
        } else {
            padding.left
        },
        padding.top,
    );
    for pending in items {
        let key = pending.key();
        let inset_auto = {
            let style = source.grid_item_style(key.node);
            style.inset().map(LengthPercentageAuto::is_auto)
        };
        let needs_static_measurement =
            (inset_auto.left && inset_auto.right) || (inset_auto.top && inset_auto.bottom);
        let content_static_offset = if needs_static_measurement {
            let item = resolve_grid_item(
                source,
                pending.key(),
                GridArea::default(),
                Size::new(Some(inner_size.width), Some(inner_size.height)),
                defaults,
            );
            absolute_static_offset(source, session, &item, inner_size, rtl)
        } else {
            Point::ZERO
        };
        match pending.position {
            Position::Absolute => {
                let (x, width) = absolute_axis_area(
                    columns,
                    pending.column,
                    explicit_columns,
                    inner_size.width,
                    padding_box_size.width,
                    logical_content_start.x,
                    rtl,
                );
                let (y, height) = absolute_axis_area(
                    rows,
                    pending.row,
                    explicit_rows,
                    inner_size.height,
                    padding_box_size.height,
                    logical_content_start.y,
                    false,
                );
                let origin = Point::new(padding_box_origin.x + x, padding_box_origin.y + y);
                let containing_size = Size::new(width, height);
                // Grid §10.2 defines the static-position rectangle as the
                // container's content box even when §10.1 selects a smaller
                // grid-area containing block. Convert that content-box point
                // into the selected containing block's local coordinates.
                let static_offset = Point::new(
                    padding.left + content_static_offset.x - x,
                    padding.top + content_static_offset.y - y,
                );
                let mut layout = compute_absolute_layout(
                    source,
                    session,
                    key.node,
                    containing_size,
                    static_offset,
                );
                layout.location.x += origin.x;
                layout.location.y += origin.y;
                layout.order = key.layout_order;
                content_size.width = content_size
                    .width
                    .max(layout.location.x + layout.size.width.max(layout.content_size.width));
                content_size.height = content_size
                    .height
                    .max(layout.location.y + layout.size.height.max(layout.content_size.height));
                session.set_unrounded_layout(key.node, &layout);
            }
            Position::AbsoluteHoisted => {
                session.set_static_position(
                    key.node,
                    Point::new(
                        content_origin.x + content_static_offset.x,
                        content_origin.y + content_static_offset.y,
                    ),
                );
            }
            Position::Relative => {}
        }
    }
    content_size
}

/// Computes CSS Grid layout for `node` using only generic, statically
/// dispatched host capabilities.
#[allow(clippy::too_many_lines)]
pub fn compute_grid_layout<Source, Session>(
    source: &Source,
    session: &mut Session,
    node: NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    // The container view stays borrowed from the immutable source while all
    // recursive work mutates only `session`; no owned raw-style snapshot is
    // needed.
    let style = source.grid_container_style(node);
    let gap_value = style.gap();
    let auto_flow = style.grid_auto_flow();
    let align_content = style.align_content().unwrap_or(AlignContent::Stretch);
    let justify_content = style.justify_content().unwrap_or(AlignContent::Stretch);
    let align_items = style.align_items();
    let item_defaults = ItemDefaults {
        align_items: align_items.unwrap_or(AlignItems::Stretch),
        align_items_normal: align_items.is_none(),
        justify_items: style.justify_items().unwrap_or(AlignItems::Stretch),
    };
    let direction = style.direction();
    let style_definite = if input.sizing_mode == SizingMode::ContentSize {
        Size::new(false, false)
    } else {
        preferred_size_definiteness(style.size(), input.parent_size, style.aspect_ratio())
    };
    let outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );
    let mut metrics = resolve_container_box(source, &style, input);
    if input.sizing_mode != SizingMode::ContentSize {
        let preferred = style.size();
        if metrics.inner.width.is_none() {
            metrics.available_inner.width = match preferred.width {
                Dimension::MinContent => AvailableSpace::MinContent,
                Dimension::MaxContent => AvailableSpace::MaxContent,
                _ => metrics.available_inner.width,
            };
        }
        if metrics.inner.height.is_none() {
            metrics.available_inner.height = match preferred.height {
                Dimension::MinContent => AvailableSpace::MinContent,
                Dimension::MaxContent => AvailableSpace::MaxContent,
                _ => metrics.available_inner.height,
            };
        }
    }
    let initial_percentage_basis = Size::new(
        outer_definite
            .width
            .then_some(metrics.inner.width)
            .flatten(),
        outer_definite
            .height
            .then_some(metrics.inner.height)
            .flatten(),
    );
    let definite_outer = Size::new(
        outer_definite
            .width
            .then_some(metrics.outer.width)
            .flatten(),
        outer_definite
            .height
            .then_some(metrics.outer.height)
            .flatten(),
    );
    let initial_gap = resolve_gap(source, gap_value, initial_percentage_basis);
    // Auto-repeat's preferred/max constraint must be clamped by min/max
    // first; CSS gives the minimum precedence when max < min. Track counts
    // use the resulting content-box constraint, not the raw border-box
    // property value.
    let repeat_max_basis = Size::new(
        definite_outer.width.or(metrics.max.width).map(|value| {
            (clamp(value, metrics.min.width, metrics.max.width) - metrics.box_inset.width).max(0.0)
        }),
        definite_outer.height.or(metrics.max.height).map(|value| {
            (clamp(value, metrics.min.height, metrics.max.height) - metrics.box_inset.height)
                .max(0.0)
        }),
    );
    let repeat_min_basis = Size::new(
        metrics
            .min
            .width
            .map(|value| (value - metrics.box_inset.width).max(0.0)),
        metrics
            .min
            .height
            .map(|value| (value - metrics.box_inset.height).max(0.0)),
    );
    // Percentage gutters are cyclic (zero) during intrinsic track sizing,
    // but auto-repeat counting resolves them against the definite preferred,
    // max, or min constraint selected above.
    let repeat_count_basis = Size::new(
        repeat_max_basis.width.or(repeat_min_basis.width),
        repeat_max_basis.height.or(repeat_min_basis.height),
    );
    let repeat_count_gap = resolve_gap(source, gap_value, repeat_count_basis);
    let (explicit_columns, explicit_rows) = expand_explicit_tracks(
        source,
        node,
        repeat_max_basis,
        repeat_min_basis,
        repeat_count_gap,
    );

    let child_count = source.child_count(node);
    let mut in_flow = Vec::with_capacity(child_count);
    let mut absolute = Vec::new();
    let mut hidden = Vec::new();
    for document_index in 0..child_count {
        let child = source.child_id(node, document_index);
        let Some(child_style) = classify_item(source, child, document_index) else {
            hidden.push((document_index, child));
            continue;
        };
        if matches!(
            child_style.position,
            Position::Absolute | Position::AbsoluteHoisted
        ) {
            absolute.push(child_style);
        } else {
            in_flow.push(child_style);
        }
    }
    // Assign paint order without the quadratic node lookup used by many
    // straightforward implementations.
    sort_and_assign_layout_order(&mut in_flow, &mut absolute);

    let placement_inputs = in_flow
        .iter()
        .map(|item| PlacementInput {
            column: item.column,
            row: item.row,
        })
        .collect::<Vec<_>>();
    let placement = place_items(
        &placement_inputs,
        explicit_columns.tracks.len(),
        explicit_rows.tracks.len(),
        auto_flow,
    );
    drop(placement_inputs);
    // Direct hosts are allowed to expose arbitrary iterators, including
    // infinite ones. Reacquire auto-track patterns only if placement created
    // implicit tracks, and cap consumption at the UA's materialized-grid
    // limit so both work and memory stay finite.
    let needs_auto_columns = placement.column_range.start < 0
        || placement.column_range.end
            > i32::try_from(explicit_columns.tracks.len()).unwrap_or(i32::MAX);
    let needs_auto_rows = placement.row_range.start < 0
        || placement.row_range.end > i32::try_from(explicit_rows.tracks.len()).unwrap_or(i32::MAX);
    let (auto_columns, auto_rows) = {
        let container = source.grid_container_style(node);
        let auto_columns = if needs_auto_columns {
            container
                .grid_auto_columns()
                .take(MAX_MATERIALIZED_TRACKS)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let auto_rows = if needs_auto_rows {
            container
                .grid_auto_rows()
                .take(MAX_MATERIALIZED_TRACKS)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        (auto_columns, auto_rows)
    };
    let column_specs = build_axis_tracks(
        &explicit_columns,
        &auto_columns,
        placement.column_range,
        &placement.occupied_columns,
    );
    let row_specs = build_axis_tracks(
        &explicit_rows,
        &auto_rows,
        placement.row_range,
        &placement.occupied_rows,
    );
    let mut items = in_flow
        .into_iter()
        .zip(placement.areas)
        .map(|(item, area)| resolve_grid_item(source, item.key(), area, Size::NONE, item_defaults))
        .collect::<Vec<_>>();

    let (mut columns, mut rows) = run_track_sizing(
        source,
        session,
        &column_specs,
        &row_specs,
        &mut items,
        initial_percentage_basis,
        metrics.available_inner,
        initial_gap,
        justify_content,
        align_content,
    );
    let provisional_track_size = Size::new(columns.used_size(), rows.used_size());
    let outer_size = final_outer_size(&metrics, provisional_track_size);
    let final_inner = Size::new(
        (outer_size.width - metrics.box_inset.width).max(0.0),
        (outer_size.height - metrics.box_inset.height).max(0.0),
    );

    // Resolve cyclic percentages and any flexible/auto track that only
    // became definite after intrinsic container sizing. Exactly one rerun is
    // allowed by Grid's bounded sizing feedback.
    let final_gap = resolve_gap(
        source,
        gap_value,
        Size::new(Some(final_inner.width), Some(final_inner.height)),
    );
    let needs_definite_rerun = initial_percentage_basis.width.is_none()
        || initial_percentage_basis.height.is_none()
        || final_gap != initial_gap;
    if needs_definite_rerun {
        (columns, rows) = run_track_sizing(
            source,
            session,
            &column_specs,
            &row_specs,
            &mut items,
            Size::new(Some(final_inner.width), Some(final_inner.height)),
            Size::new(
                AvailableSpace::Definite(final_inner.width),
                AvailableSpace::Definite(final_inner.height),
            ),
            final_gap,
            justify_content,
            align_content,
        );
    }
    align_tracks(&mut columns, final_inner.width, justify_content);
    align_tracks(&mut rows, final_inner.height, align_content);

    let content_origin = Point::new(
        metrics.border.left + metrics.padding.left,
        metrics.border.top + metrics.padding.top,
    );
    let rtl = direction == Direction::Rtl;
    let (mut content_size, baselines) = layout_in_flow_items(
        source,
        session,
        &mut items,
        &columns,
        &rows,
        final_inner,
        content_origin,
        outer_size,
        input.goal,
        rtl,
    );
    if input.goal == LayoutGoal::Commit {
        for (document_index, child) in hidden {
            hide_subtree(source, session, child);
            session.set_unrounded_layout(
                child,
                &Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
            );
        }
        let absolute_content_size = layout_absolute_items(
            source,
            session,
            &absolute,
            &columns,
            &rows,
            explicit_columns.tracks.len(),
            explicit_rows.tracks.len(),
            final_inner,
            outer_size,
            metrics.padding,
            metrics.border,
            metrics.scrollbar,
            rtl,
            item_defaults,
        );
        content_size = content_size.zip_map(absolute_content_size, f32::max);
    }
    LayoutOutput::new(outer_size, content_size).with_first_baselines(baselines)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::style::GridLine;

    fn line(value: i16) -> GridPlacement {
        GridPlacement::Line(GridLine::new(value))
    }

    #[test]
    fn container_floors_are_resolved() {
        let metrics = ResolvedContainerBox {
            padding: Edges::ZERO,
            border: Edges::ZERO,
            scrollbar: Size::ZERO,
            box_inset: Size::new(4.0, 6.0),
            min: Size::new(Some(20.0), None),
            max: Size::new(Some(30.0), Some(18.0)),
            outer: Size::new(None, None),
            inner: Size::NONE,
            available_inner: Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        };
        assert_eq!(
            final_outer_size(&metrics, Size::new(10.0, 20.0)),
            Size::new(20.0, 18.0)
        );
    }

    #[test]
    fn absolute_axis_lines_cover_definite_partial_and_invalid_forms() {
        assert_eq!(
            absolute_axis_lines(Line::new(line(2), line(4)), 3),
            (Some(1), Some(3))
        );
        assert_eq!(
            absolute_axis_lines(Line::new(line(2), GridPlacement::Auto), 3),
            (Some(1), None)
        );
        assert_eq!(
            absolute_axis_lines(Line::new(GridPlacement::Auto, line(-1)), 3),
            (None, Some(3))
        );
        assert_eq!(
            absolute_axis_lines(Line::new(GridPlacement::Span(2), GridPlacement::Span(3)), 3,),
            (None, None)
        );
        assert_eq!(
            absolute_axis_lines(Line::new(line(0), GridPlacement::Auto), 3),
            (None, None)
        );
    }

    #[test]
    fn empty_absolute_axis_uses_scrollable_padding_edges() {
        let tracks = TrackSet::default();
        assert_eq!(
            absolute_axis_area(
                &tracks,
                Line::new(GridPlacement::Auto, GridPlacement::Auto),
                0,
                10.0,
                20.0,
                2.0,
                false,
            ),
            (0.0, 20.0)
        );
        assert_eq!(
            absolute_axis_area(
                &tracks,
                Line::new(GridPlacement::Auto, GridPlacement::Auto),
                0,
                30.0,
                30.0,
                2.0,
                true,
            ),
            (0.0, 30.0)
        );
    }
}
