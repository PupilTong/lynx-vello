//! CSS Grid §12 track sizing.
//!
//! The public protocol deliberately exposes no track storage.  This module
//! materializes a compact vector for one axis, runs intrinsic contributions
//! in span order, then maximizes fixed/intrinsic tracks and expands flexible
//! tracks.  Child measurements always round-trip through the generic host
//! callback, so mixed Grid/Flex/custom subtrees retain one cache policy.

#![allow(clippy::cast_precision_loss)]

use super::super::util::{clamp, resolve_length_percentage};
use super::tracks::AxisTrackSpec;
use super::types::{Axis, GridItem, Track, TrackSet};
use crate::style::alignment::AlignContent;
use crate::style::{CoreStyle, Dimension, MaxTrackSizingFunction, MinTrackSizingFunction};
use crate::tree::{
    AvailableSpace, GridSource, LayoutInput, LayoutSession, RequestedAxis, SizingMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContributionKind {
    Minimum,
    MinContent,
    MaxContent,
}

#[inline]
fn requested_axis(axis: Axis) -> RequestedAxis {
    match axis {
        Axis::Horizontal => RequestedAxis::Horizontal,
        Axis::Vertical => RequestedAxis::Vertical,
    }
}

#[inline]
fn available_for(kind: ContributionKind) -> AvailableSpace {
    match kind {
        ContributionKind::Minimum | ContributionKind::MinContent => AvailableSpace::MinContent,
        ContributionKind::MaxContent => AvailableSpace::MaxContent,
    }
}

/// Initializes used track state per Grid §12.4.
pub(super) fn initialize_tracks<Source: GridSource>(
    source: &Source,
    specs: &[AxisTrackSpec],
    percentage_basis: Option<f32>,
    gap: f32,
) -> TrackSet {
    let mut tracks = Vec::with_capacity(specs.len());
    for spec in specs {
        let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
        let min_fixed = match spec.sizing.min {
            MinTrackSizingFunction::Fixed(value) => {
                resolve_length_percentage(value, percentage_basis, &resolve_calc)
            }
            _ => None,
        };
        let max_fixed = match spec.sizing.max {
            MaxTrackSizingFunction::Fixed(value) => {
                resolve_length_percentage(value, percentage_basis, &resolve_calc)
            }
            _ => None,
        };
        // Percentage/calc tracks in an indefinite axis are treated as
        // `auto` during intrinsic sizing, then reconstructed with a definite
        // basis by the bounded post-sizing rerun.
        let mut effective_sizing = spec.sizing;
        if matches!(effective_sizing.min, MinTrackSizingFunction::Fixed(_)) && min_fixed.is_none() {
            effective_sizing.min = MinTrackSizingFunction::Auto;
        }
        if matches!(effective_sizing.max, MaxTrackSizingFunction::Fixed(_)) && max_fixed.is_none() {
            effective_sizing.max = MaxTrackSizingFunction::Auto;
        }
        let fit_content_limit = match spec.sizing.max {
            MaxTrackSizingFunction::FitContent(value) => {
                resolve_length_percentage(value, percentage_basis, &resolve_calc)
                    .unwrap_or(f32::INFINITY)
            }
            _ => f32::INFINITY,
        };
        let collapsed = spec.collapsed;
        let base = if collapsed {
            0.0
        } else {
            min_fixed.unwrap_or(0.0).max(0.0)
        };
        let growth_limit = if collapsed {
            0.0
        } else {
            max_fixed.map_or(f32::INFINITY, |value| value.max(base).max(0.0))
        };
        let flex_factor = if collapsed {
            0.0
        } else {
            match effective_sizing.max {
                MaxTrackSizingFunction::Fr(factor) if factor.is_finite() && factor > 0.0 => factor,
                _ => 0.0,
            }
        };
        let flexible = !collapsed && matches!(effective_sizing.max, MaxTrackSizingFunction::Fr(_));
        tracks.push(Track {
            sizing: effective_sizing,
            base,
            growth_limit,
            fit_content_limit,
            flex_factor,
            flexible,
            intrinsic_min: !matches!(effective_sizing.min, MinTrackSizingFunction::Fixed(_)),
            intrinsic_max: !matches!(
                effective_sizing.max,
                MaxTrackSizingFunction::Fixed(_) | MaxTrackSizingFunction::Fr(_)
            ),
            auto_max: matches!(effective_sizing.max, MaxTrackSizingFunction::Auto),
            infinitely_growable: false,
            collapsed,
            position: 0.0,
        });
    }
    TrackSet {
        tracks,
        gap,
        first_coordinate: specs.first().map_or(0, |spec| spec.coordinate),
        collapsed_line_positions: None,
    }
}

#[inline]
fn span_for(item: &GridItem, axis: Axis) -> super::placement::TrackSpan {
    match axis {
        Axis::Horizontal => item.area.column,
        Axis::Vertical => item.area.row,
    }
}

#[inline]
fn margin_sum(item: &GridItem, axis: Axis) -> f32 {
    axis.sum(item.margin)
}

fn cross_area_size(item: &GridItem, axis: Axis, cross_tracks: Option<&TrackSet>) -> Option<f32> {
    let cross = axis.other();
    let tracks = cross_tracks?;
    let span = span_for(item, cross);
    Some((tracks.area_size(span.start, span.end) - margin_sum(item, cross)).max(0.0))
}

fn raw_content_size<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut GridItem,
    axis: Axis,
    kind: ContributionKind,
    cross_tracks: Option<&TrackSet>,
    inner_size: crate::geometry::Size<Option<f32>>,
) -> f32
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    let cached = match kind {
        ContributionKind::Minimum | ContributionKind::MinContent => axis.size(item.raw_min_content),
        ContributionKind::MaxContent => axis.size(item.raw_max_content),
    };
    if let Some(cached) = cached {
        return cached;
    }

    let cross = axis.other();
    let cross_area = cross_area_size(item, axis, cross_tracks);
    let cross_stretches = match cross {
        Axis::Horizontal => item.justify_self,
        Axis::Vertical => item.align_self,
    } == crate::style::AlignItems::Stretch
        && cross.size(item.preferred_size).is_none()
        && !cross.start(item.margin_auto)
        && !cross.end(item.margin_auto);
    let mut known = crate::geometry::Size::NONE;
    let resolved_cross = cross
        .size(item.preferred_size)
        .or_else(|| cross_stretches.then_some(cross_area).flatten())
        .map(|value| clamp(value, cross.size(item.min_size), cross.size(item.max_size)));
    cross.set_size(&mut known, resolved_cross);

    let target_available = available_for(kind);
    let cross_available = cross_area
        .or_else(|| cross.size(inner_size))
        .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite);
    let available = match axis {
        Axis::Horizontal => crate::geometry::Size::new(target_available, cross_available),
        Axis::Vertical => crate::geometry::Size::new(cross_available, target_available),
    };
    let parent_size = match axis {
        Axis::Horizontal => crate::geometry::Size::new(None, cross_area),
        Axis::Vertical => crate::geometry::Size::new(cross_area, None),
    };
    let mut input = LayoutInput::compute_size(known, parent_size, available, requested_axis(axis));
    input.sizing_mode = SizingMode::ContentSize;
    let output = session.compute_child_layout(source, item.key.node, input);
    let size = output.size;
    if axis == Axis::Vertical && item.align_self == crate::style::AlignItems::Baseline {
        // CSS synthesizes a baseline when the child does not expose one. A
        // bottom-border-edge fallback gives the correct ascent/descent
        // envelope for track sizing and matches the final layout pass.
        item.measured_baselines.y = Some(output.first_baselines.y.unwrap_or(size.height));
    }
    let mut measured = axis.size(size);
    if let (Some(ratio), Some(cross_size)) = (item.aspect_ratio, resolved_cross)
        && ratio.is_finite()
        && ratio > 0.0
    {
        let axis_inset = axis.sum(item.padding) + axis.sum(item.border);
        let cross_inset = cross.sum(item.padding) + cross.sum(item.border);
        let sizing_cross = if item.box_sizing == crate::style::BoxSizing::ContentBox {
            (cross_size - cross_inset).max(0.0)
        } else {
            cross_size
        };
        let sizing_axis = match axis {
            Axis::Horizontal => sizing_cross * ratio,
            Axis::Vertical => sizing_cross / ratio,
        };
        let ratio_size = if item.box_sizing == crate::style::BoxSizing::ContentBox {
            sizing_axis + axis_inset
        } else {
            sizing_axis
        };
        measured = measured.max(ratio_size);
    }
    match kind {
        ContributionKind::Minimum | ContributionKind::MinContent => {
            axis.set_size(&mut item.raw_min_content, Some(measured));
        }
        ContributionKind::MaxContent => {
            axis.set_size(&mut item.raw_max_content, Some(measured));
        }
    }
    measured
}

pub(super) fn probe_raw_min_content<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut GridItem,
    axis: Axis,
    cross_tracks: Option<&TrackSet>,
    inner_size: crate::geometry::Size<Option<f32>>,
) -> f32
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    raw_content_size(
        source,
        session,
        item,
        axis,
        ContributionKind::MinContent,
        cross_tracks,
        inner_size,
    )
}

pub(super) fn resolve_item_intrinsic_dimensions<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut GridItem,
    axis: Axis,
    cross_tracks: Option<&TrackSet>,
    inner_size: crate::geometry::Size<Option<f32>>,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    let (preferred_value, min_value, max_value) = {
        let style = source.grid_item_style(item.key.node);
        (
            axis.size(style.size()),
            axis.size(style.min_size()),
            axis.size(style.max_size()),
        )
    };
    let values = [preferred_value, min_value, max_value];
    let needs_min_content = values
        .iter()
        .any(|value| matches!(value, Dimension::MinContent | Dimension::FitContent(_)));
    let needs_max_content = values
        .iter()
        .any(|value| matches!(value, Dimension::MaxContent | Dimension::FitContent(_)));
    if !needs_min_content && !needs_max_content {
        return;
    }

    let min_content = if needs_min_content {
        raw_content_size(
            source,
            session,
            item,
            axis,
            ContributionKind::MinContent,
            cross_tracks,
            inner_size,
        )
    } else {
        0.0
    };
    let max_content = if needs_max_content {
        raw_content_size(
            source,
            session,
            item,
            axis,
            ContributionKind::MaxContent,
            cross_tracks,
            inner_size,
        )
    } else {
        0.0
    };
    let resolve = |value: Dimension| -> Option<f32> {
        match value {
            Dimension::MinContent => Some(min_content),
            Dimension::MaxContent => Some(max_content),
            Dimension::FitContent(limit) => {
                let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
                let limit = resolve_length_percentage(limit, axis.size(inner_size), &resolve_calc)
                    .unwrap_or(max_content);
                Some(max_content.min(limit.max(min_content)))
            }
            Dimension::Length(_) | Dimension::Percent(_) | Dimension::Calc(_) | Dimension::Auto => {
                None
            }
        }
    };
    if axis.size(item.preferred_size).is_none() {
        axis.set_size(&mut item.preferred_size, resolve(preferred_value));
    }
    if axis.size(item.min_size).is_none() {
        axis.set_size(&mut item.min_size, resolve(min_value));
    }
    if axis.size(item.max_size).is_none() {
        axis.set_size(&mut item.max_size, resolve(max_value));
    }
}

#[allow(clippy::too_many_arguments)]
fn measure_contribution<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut GridItem,
    axis: Axis,
    kind: ContributionKind,
    tracks: &TrackSet,
    cross_tracks: Option<&TrackSet>,
    inner_size: crate::geometry::Size<Option<f32>>,
) -> f32
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    let cached = match kind {
        ContributionKind::Minimum => axis.size(item.minimum_contribution),
        ContributionKind::MinContent => axis.size(item.min_content_contribution),
        ContributionKind::MaxContent => axis.size(item.max_content_contribution),
    };
    if let Some(cached) = cached {
        return cached;
    }

    let preferred = axis
        .size(item.preferred_size)
        .map(|value| value + margin_sum(item, axis));
    let explicit_min = axis
        .size(item.min_size)
        .map(|value| value + margin_sum(item, axis));
    let explicit_max = axis
        .size(item.max_size)
        .map(|value| value + margin_sum(item, axis));

    let preferred_behaves_auto_or_depends = axis.size(item.preferred_behaves_auto_or_depends);
    let mut contribution = match kind {
        ContributionKind::Minimum => {
            if let Some(minimum) = explicit_min {
                minimum
            } else {
                // Grid §6.6: an automatic minimum is content-based when
                // the item spans an auto-min track; only multi-track spans
                // are disqualified by crossing a flexible track.
                let span = span_for(item, axis);
                let indexes = tracks.span_indices(span.start, span.end);
                let automatic_min_applies = axis.size(item.minimum_is_auto)
                    && !item.overflow_point(axis).is_scroll_container()
                    && tracks.tracks[indexes.clone()]
                        .iter()
                        .any(|track| matches!(track.sizing.min, MinTrackSizingFunction::Auto))
                    && (item.span(axis) == 1
                        || !tracks.tracks[indexes.clone()]
                            .iter()
                            .any(|track| track.is_flexible()));
                if automatic_min_applies {
                    let raw_outer = raw_content_size(
                        source,
                        session,
                        item,
                        axis,
                        ContributionKind::MinContent,
                        cross_tracks,
                        inner_size,
                    ) + margin_sum(item, axis);
                    // The specified-size suggestion caps the content-size
                    // suggestion when a percentage became definite.
                    let suggestion = preferred.map_or(raw_outer, |size| raw_outer.min(size));
                    fixed_max_span_limit(source, axis, tracks, indexes, inner_size)
                        .map_or(suggestion, |limit| suggestion.min(limit))
                } else if !preferred_behaves_auto_or_depends {
                    // A definite preferred size defines the box's minimum
                    // contribution only when Grid's content-based automatic
                    // minimum does not apply.
                    preferred.unwrap_or_else(|| {
                        raw_content_size(
                            source,
                            session,
                            item,
                            axis,
                            kind,
                            cross_tracks,
                            inner_size,
                        ) + margin_sum(item, axis)
                    })
                } else {
                    axis.sum(item.padding)
                        + axis.sum(item.border)
                        + axis.size(item.scrollbar)
                        + margin_sum(item, axis)
                }
            }
        }
        ContributionKind::MinContent | ContributionKind::MaxContent => {
            if preferred_behaves_auto_or_depends {
                raw_content_size(source, session, item, axis, kind, cross_tracks, inner_size)
                    + margin_sum(item, axis)
            } else {
                preferred.unwrap_or_else(|| {
                    raw_content_size(source, session, item, axis, kind, cross_tracks, inner_size)
                        + margin_sum(item, axis)
                })
            }
        }
    };
    contribution = clamp(contribution, explicit_min, explicit_max).max(0.0);
    if axis == Axis::Vertical {
        contribution += item.baseline_shim;
    }

    match kind {
        ContributionKind::Minimum => {
            axis.set_size(&mut item.minimum_contribution, Some(contribution));
        }
        ContributionKind::MinContent => {
            axis.set_size(&mut item.min_content_contribution, Some(contribution));
        }
        ContributionKind::MaxContent => {
            axis.set_size(&mut item.max_content_contribution, Some(contribution));
        }
    }
    contribution
}

fn span_gap(tracks: &TrackSet, range: core::ops::Range<usize>) -> f32 {
    let visible = tracks.tracks[range]
        .iter()
        .filter(|track| !track.collapsed)
        .count();
    tracks.gap * visible.saturating_sub(1) as f32
}

/// Returns the maximum grid-area size when every visible track in the span
/// has a fixed max track sizing function. Grid §6.6 includes intervening
/// gutters in this stretch-fit clamp; collapsed tracks contribute zero, and
/// `span_gap` preserves only the gutters between surviving visible tracks.
fn fixed_max_span_limit<Source: GridSource>(
    source: &Source,
    axis: Axis,
    tracks: &TrackSet,
    range: core::ops::Range<usize>,
    inner_size: crate::geometry::Size<Option<f32>>,
) -> Option<f32> {
    let mut limit = span_gap(tracks, range.clone());
    let percentage_basis = axis.size(inner_size);
    let resolve_calc = |handle, basis| source.resolve_calc(handle, basis);
    for track in &tracks.tracks[range] {
        if track.collapsed {
            continue;
        }
        let MaxTrackSizingFunction::Fixed(maximum) = track.sizing.max else {
            return None;
        };
        let maximum = resolve_length_percentage(maximum, percentage_basis, &resolve_calc)?;
        let minimum = match track.sizing.min {
            MinTrackSizingFunction::Fixed(minimum) => {
                resolve_length_percentage(minimum, percentage_basis, &resolve_calc)?
            }
            MinTrackSizingFunction::Auto
            | MinTrackSizingFunction::MinContent
            | MinTrackSizingFunction::MaxContent => 0.0,
        };
        // `minmax()` gives its minimum precedence when a declared fixed max
        // is smaller. Resolve from immutable sizing functions rather than a
        // growth limit that earlier item distribution may have raised.
        limit += maximum.max(minimum).max(0.0);
    }
    Some(limit)
}

/// Computes Grid §12.5's limited min-/max-content contribution. A spanning
/// item is capped only when every track has a fixed max sizing function; for
/// one track, a resolved `fit-content()` argument is also an allowed limit.
/// The result is always floored by the item's minimum contribution.
#[allow(clippy::too_many_arguments)]
fn measure_limited_contribution<Source, Session>(
    source: &Source,
    session: &mut Session,
    item: &mut GridItem,
    axis: Axis,
    kind: ContributionKind,
    tracks: &TrackSet,
    cross_tracks: Option<&TrackSet>,
    inner_size: crate::geometry::Size<Option<f32>>,
) -> f32
where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    debug_assert!(matches!(
        kind,
        ContributionKind::MinContent | ContributionKind::MaxContent
    ));
    let contribution = measure_contribution(
        source,
        session,
        item,
        axis,
        kind,
        tracks,
        cross_tracks,
        inner_size,
    );
    let span = span_for(item, axis);
    let range = tracks.span_indices(span.start, span.end);
    let fixed_limit = fixed_max_span_limit(source, axis, tracks, range.clone(), inner_size)
        .or_else(|| {
            (range.len() == 1)
                .then(|| tracks.tracks[range.start].fit_content_limit)
                .filter(|limit| limit.is_finite())
        });
    let Some(limit) = fixed_limit else {
        return contribution;
    };
    let minimum = measure_contribution(
        source,
        session,
        item,
        axis,
        ContributionKind::Minimum,
        tracks,
        cross_tracks,
        inner_size,
    );
    contribution.min(limit).max(minimum)
}

/// Computes first-baseline start shims before row contributions are used.
/// Groups are keyed by their shared start row; sorting once keeps this
/// linear after `O(B log B)` setup and avoids pairwise baseline scans.
fn prepare_baseline_shims<Source, Session>(
    source: &Source,
    session: &mut Session,
    axis: Axis,
    tracks: &TrackSet,
    cross_tracks: Option<&TrackSet>,
    items: &mut [GridItem],
    inner_size: crate::geometry::Size<Option<f32>>,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    if axis != Axis::Vertical {
        return;
    }
    let mut candidates = Vec::<(i32, usize, f32)>::new();
    for (index, item) in items.iter_mut().enumerate() {
        item.baseline_shim = 0.0;
        if item.align_self != crate::style::AlignItems::Baseline
            || item.margin_auto.top
            || item.margin_auto.bottom
        {
            continue;
        }
        let _ = raw_content_size(
            source,
            session,
            item,
            axis,
            ContributionKind::MinContent,
            cross_tracks,
            inner_size,
        );
        let Some(baseline) = item.measured_baselines.y else {
            continue;
        };
        let span = span_for(item, axis);
        debug_assert!(tracks.span_indices(span.start, span.end).start < tracks.tracks.len());
        candidates.push((span.start, index, item.margin.top + baseline));
    }
    candidates.sort_unstable_by_key(|&(row, _, _)| row);
    let mut start = 0;
    while start < candidates.len() {
        let row = candidates[start].0;
        let mut end = start + 1;
        let mut maximum_ascent = candidates[start].2;
        while end < candidates.len() && candidates[end].0 == row {
            maximum_ascent = maximum_ascent.max(candidates[end].2);
            end += 1;
        }
        for &(_, item_index, ascent) in &candidates[start..end] {
            items[item_index].baseline_shim = (maximum_ascent - ascent).max(0.0);
        }
        start = end;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedSize {
    Base,
    GrowthLimit,
}

fn span_affected_size(
    tracks: &TrackSet,
    range: core::ops::Range<usize>,
    affected_size: PlannedSize,
) -> f32 {
    let size = tracks.tracks[range.clone()]
        .iter()
        .map(|track| match affected_size {
            PlannedSize::GrowthLimit if track.growth_limit.is_finite() => track.growth_limit,
            PlannedSize::Base | PlannedSize::GrowthLimit => track.base,
        })
        .sum::<f32>();
    size + span_gap(tracks, range)
}

#[inline]
fn record_planned(index: usize, increase: f32, planned: &mut [f32], touched: &mut Vec<usize>) {
    if increase <= planned[index] {
        return;
    }
    if planned[index] == 0.0 {
        touched.push(index);
    }
    planned[index] = increase;
}

/// Grid §12.5.1's freeze-and-redistribute operation. Scratch is reused
/// across every item, avoiding an allocation per spanned contribution.
#[derive(Debug, Clone, Copy)]
struct DistributionEntry {
    index: usize,
    capacity: f32,
    increase: f32,
}

fn distribute_up_to_limits(entries: &mut [DistributionEntry], remaining: &mut f32) {
    if entries.is_empty() || *remaining <= 0.0 {
        return;
    }
    // Intrinsic tracks commonly all start with infinite capacity. Avoid the
    // comparison sort entirely on that hot path.
    if entries.iter().all(|entry| entry.capacity.is_infinite()) {
        let share = *remaining / entries.len() as f32;
        for entry in entries {
            entry.increase = share;
        }
        *remaining = 0.0;
        return;
    }

    entries.sort_unstable_by(|left, right| left.capacity.total_cmp(&right.capacity));
    let mut level = 0.0;
    let mut cursor = 0;
    while cursor < entries.len() {
        let unfrozen = entries.len() - cursor;
        let capacity = entries[cursor].capacity;
        let required = (capacity - level).max(0.0) * unfrozen as f32;
        if required >= *remaining {
            level += *remaining / unfrozen as f32;
            *remaining = 0.0;
            break;
        }
        level = capacity;
        *remaining -= required;
        cursor += 1;
    }
    for entry in entries {
        entry.increase = level.min(entry.capacity);
    }
}

#[inline]
fn distribution_capacity(track: &Track, affected_size: PlannedSize) -> f32 {
    let starting = match affected_size {
        PlannedSize::GrowthLimit if track.growth_limit.is_finite() => track.growth_limit,
        PlannedSize::Base | PlannedSize::GrowthLimit => track.base,
    };
    let limit = match affected_size {
        PlannedSize::Base => track.growth_limit.min(track.fit_content_limit),
        PlannedSize::GrowthLimit
            if track.growth_limit.is_finite() && !track.infinitely_growable =>
        {
            starting
        }
        PlannedSize::GrowthLimit => track.fit_content_limit,
    };
    (limit - starting).max(0.0)
}

#[allow(clippy::too_many_arguments)]
fn distribute_extra<P>(
    tracks: &TrackSet,
    range: core::ops::Range<usize>,
    extra: f32,
    affected_size: PlannedSize,
    contribution_kind: ContributionKind,
    weighted_flex: bool,
    eligible: P,
    planned: &mut [f32],
    touched: &mut Vec<usize>,
    affected: &mut Vec<DistributionEntry>,
    non_affected: &mut Vec<DistributionEntry>,
) where
    P: Fn(&Track) -> bool,
{
    if extra <= 0.0 {
        return;
    }
    affected.clear();
    non_affected.clear();
    for index in range {
        let track = &tracks.tracks[index];
        if track.collapsed {
            continue;
        }
        let entry = DistributionEntry {
            index,
            capacity: distribution_capacity(track, affected_size),
            increase: 0.0,
        };
        if eligible(track) {
            affected.push(entry);
        } else {
            non_affected.push(entry);
        }
    }
    if affected.is_empty() {
        return;
    }

    if weighted_flex {
        let factor_sum = affected
            .iter()
            .map(|entry| tracks.tracks[entry.index].flex_factor)
            .sum::<f32>();
        let equal_remainder = (1.0 - factor_sum).max(0.0) * extra / affected.len() as f32;
        let denominator = factor_sum.max(1.0);
        for entry in affected {
            let proportional = extra * tracks.tracks[entry.index].flex_factor / denominator;
            record_planned(
                entry.index,
                proportional + equal_remainder,
                planned,
                touched,
            );
        }
        return;
    }

    let mut remaining = extra;
    distribute_up_to_limits(affected, &mut remaining);
    if remaining > 0.0 && !non_affected.is_empty() {
        distribute_up_to_limits(non_affected, &mut remaining);
    }

    if remaining > 0.0 {
        let preferred = |entry: &&mut DistributionEntry| {
            let track = &tracks.tracks[entry.index];
            match (affected_size, contribution_kind) {
                (PlannedSize::Base, ContributionKind::Minimum | ContributionKind::MinContent) => {
                    track.intrinsic_max
                }
                (PlannedSize::Base, ContributionKind::MaxContent) => matches!(
                    track.sizing.max,
                    MaxTrackSizingFunction::MaxContent
                        | MaxTrackSizingFunction::Auto
                        | MaxTrackSizingFunction::FitContent(_)
                ),
                (PlannedSize::GrowthLimit, _) => {
                    track.intrinsic_max && track.fit_content_limit.is_infinite()
                }
            }
        };
        let preferred_count = affected.iter_mut().filter(preferred).count();
        let use_all = preferred_count == 0;
        let count = if use_all {
            affected.len()
        } else {
            preferred_count
        };
        let share = remaining / count as f32;
        for entry in affected.iter_mut() {
            if use_all || preferred(&entry) {
                entry.increase += share;
            }
        }
    }

    for entry in affected.iter().chain(non_affected.iter()) {
        record_planned(entry.index, entry.increase, planned, touched);
    }
}

fn apply_planned_base(tracks: &mut TrackSet, planned: &mut [f32], touched: &mut Vec<usize>) {
    for index in touched.drain(..) {
        let increase = core::mem::take(&mut planned[index]);
        let track = &mut tracks.tracks[index];
        track.base += increase;
        track.growth_limit = track.growth_limit.max(track.base);
    }
}

fn apply_planned_growth(tracks: &mut TrackSet, planned: &mut [f32], touched: &mut Vec<usize>) {
    for index in touched.drain(..) {
        let increase = core::mem::take(&mut planned[index]);
        let track = &mut tracks.tracks[index];
        let was_infinite = !track.growth_limit.is_finite();
        let starting = if track.growth_limit.is_finite() {
            track.growth_limit
        } else {
            track.base
        };
        track.growth_limit = (starting + increase)
            .min(track.fit_content_limit)
            .max(track.base);
        if was_infinite && track.growth_limit.is_finite() {
            track.infinitely_growable = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_spanning_base_phase<Source, Session, P>(
    source: &Source,
    session: &mut Session,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<&TrackSet>,
    items: &mut [GridItem],
    item_indices: &[usize],
    inner_size: crate::geometry::Size<Option<f32>>,
    kind: ContributionKind,
    limited: bool,
    weighted_flex: bool,
    eligible: P,
    planned: &mut [f32],
    touched: &mut Vec<usize>,
    affected_scratch: &mut Vec<DistributionEntry>,
    non_affected_scratch: &mut Vec<DistributionEntry>,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
    P: Fn(&Track) -> bool + Copy,
{
    for &item_index in item_indices {
        let span = span_for(&items[item_index], axis);
        let range = tracks.span_indices(span.start, span.end);
        if !tracks.tracks[range.clone()]
            .iter()
            .any(|track| !track.collapsed && eligible(track))
        {
            continue;
        }
        let contribution = if limited {
            measure_limited_contribution(
                source,
                session,
                &mut items[item_index],
                axis,
                kind,
                tracks,
                cross_tracks,
                inner_size,
            )
        } else {
            measure_contribution(
                source,
                session,
                &mut items[item_index],
                axis,
                kind,
                tracks,
                cross_tracks,
                inner_size,
            )
        };
        let extra = contribution - span_affected_size(tracks, range.clone(), PlannedSize::Base);
        distribute_extra(
            tracks,
            range,
            extra,
            PlannedSize::Base,
            kind,
            weighted_flex,
            eligible,
            planned,
            touched,
            affected_scratch,
            non_affected_scratch,
        );
    }
    apply_planned_base(tracks, planned, touched);
}

#[allow(clippy::too_many_arguments)]
fn run_spanning_growth_phase<Source, Session, P>(
    source: &Source,
    session: &mut Session,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<&TrackSet>,
    items: &mut [GridItem],
    item_indices: &[usize],
    inner_size: crate::geometry::Size<Option<f32>>,
    kind: ContributionKind,
    eligible: P,
    planned: &mut [f32],
    touched: &mut Vec<usize>,
    affected_scratch: &mut Vec<DistributionEntry>,
    non_affected_scratch: &mut Vec<DistributionEntry>,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
    P: Fn(&Track) -> bool + Copy,
{
    for &item_index in item_indices {
        let span = span_for(&items[item_index], axis);
        let range = tracks.span_indices(span.start, span.end);
        if !tracks.tracks[range.clone()]
            .iter()
            .any(|track| !track.collapsed && eligible(track))
        {
            continue;
        }
        let contribution = measure_contribution(
            source,
            session,
            &mut items[item_index],
            axis,
            kind,
            tracks,
            cross_tracks,
            inner_size,
        );
        let extra =
            contribution - span_affected_size(tracks, range.clone(), PlannedSize::GrowthLimit);
        distribute_extra(
            tracks,
            range,
            extra,
            PlannedSize::GrowthLimit,
            kind,
            false,
            eligible,
            planned,
            touched,
            affected_scratch,
            non_affected_scratch,
        );
    }
    apply_planned_growth(tracks, planned, touched);
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn resolve_intrinsic_sizes<Source, Session>(
    source: &Source,
    session: &mut Session,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<&TrackSet>,
    items: &mut [GridItem],
    inner_size: crate::geometry::Size<Option<f32>>,
    available: AvailableSpace,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    // Non-flexible single-track items resolve directly to maxima without
    // span scratch (Grid §12.5 step 2).
    let mut single_growth_limits = None::<Vec<Option<f32>>>;
    for item in items.iter_mut().filter(|item| item.span(axis) == 1) {
        let span = span_for(item, axis);
        let index = tracks.index_of(span.start);
        let track = tracks.tracks[index];
        if track.collapsed || track.is_flexible() {
            continue;
        }
        let base = match track.sizing.min {
            MinTrackSizingFunction::Fixed(_) => track.base,
            MinTrackSizingFunction::MinContent => measure_contribution(
                source,
                session,
                item,
                axis,
                ContributionKind::MinContent,
                tracks,
                cross_tracks,
                inner_size,
            ),
            MinTrackSizingFunction::MaxContent => measure_contribution(
                source,
                session,
                item,
                axis,
                ContributionKind::MaxContent,
                tracks,
                cross_tracks,
                inner_size,
            ),
            MinTrackSizingFunction::Auto if available == AvailableSpace::MinContent => {
                measure_limited_contribution(
                    source,
                    session,
                    item,
                    axis,
                    ContributionKind::MinContent,
                    tracks,
                    cross_tracks,
                    inner_size,
                )
            }
            MinTrackSizingFunction::Auto if available == AvailableSpace::MaxContent => {
                // The max-content constrained branch still performs the
                // limited min-content probe first. Besides providing the
                // automatic-minimum floor required by §12.5, this preserves
                // cross-axis feedback when the two intrinsic contributions
                // respond differently to the resolved opposite track.
                let _ = measure_limited_contribution(
                    source,
                    session,
                    item,
                    axis,
                    ContributionKind::MinContent,
                    tracks,
                    cross_tracks,
                    inner_size,
                );
                measure_limited_contribution(
                    source,
                    session,
                    item,
                    axis,
                    ContributionKind::MaxContent,
                    tracks,
                    cross_tracks,
                    inner_size,
                )
            }
            MinTrackSizingFunction::Auto => measure_contribution(
                source,
                session,
                item,
                axis,
                ContributionKind::Minimum,
                tracks,
                cross_tracks,
                inner_size,
            ),
        };
        tracks.tracks[index].base = tracks.tracks[index].base.max(base);

        let max_kind = match track.sizing.max {
            MaxTrackSizingFunction::Fixed(_) | MaxTrackSizingFunction::Fr(_) => None,
            MaxTrackSizingFunction::MinContent => Some(ContributionKind::MinContent),
            MaxTrackSizingFunction::MaxContent
            | MaxTrackSizingFunction::Auto
            | MaxTrackSizingFunction::FitContent(_) => Some(ContributionKind::MaxContent),
        };
        if let Some(kind) = max_kind {
            let contribution = measure_contribution(
                source,
                session,
                item,
                axis,
                kind,
                tracks,
                cross_tracks,
                inner_size,
            );
            let limit = contribution
                .max(tracks.tracks[index].base)
                .min(tracks.tracks[index].fit_content_limit);
            let limits =
                single_growth_limits.get_or_insert_with(|| vec![None::<f32>; tracks.tracks.len()]);
            limits[index] = Some(limits[index].map_or(limit, |current| current.max(limit)));
        }
    }
    if let Some(single_growth_limits) = single_growth_limits {
        for (index, contribution) in single_growth_limits.into_iter().enumerate() {
            if let Some(contribution) = contribution {
                tracks.tracks[index].growth_limit = contribution
                    .min(tracks.tracks[index].fit_content_limit)
                    .max(tracks.tracks[index].base);
            }
        }
    }
    // Grid §12.5 step 2 requires every growth limit to be at least its base
    // size before spanning-item distribution. This also covers fixed maxima
    // and fit-content clamps whose declared limit is smaller than an
    // intrinsic minimum established by a single-track item.
    for track in &mut tracks.tracks {
        track.growth_limit = track.growth_limit.max(track.base);
    }

    // Sort once by span, then process contiguous equal-span buckets. Each
    // phase visits only its bucket; per-item freeze distribution is bounded
    // by that item's span and skips sorting when capacities are all infinite.
    let mut non_flexible = Vec::<usize>::new();
    let mut crosses_flexible = Vec::<usize>::new();
    for (index, item) in items.iter().enumerate() {
        let span = span_for(item, axis);
        let range = tracks.span_indices(span.start, span.end);
        if tracks.tracks[range.clone()]
            .iter()
            .any(|track| track.is_flexible())
        {
            crosses_flexible.push(index);
        } else if item.span(axis) > 1
            && tracks.tracks[range]
                .iter()
                .any(|track| track.intrinsic_min || track.intrinsic_max)
        {
            non_flexible.push(index);
        }
    }
    non_flexible.sort_unstable_by_key(|&index| items[index].span(axis));
    if non_flexible.is_empty() && crosses_flexible.is_empty() {
        for track in &mut tracks.tracks {
            if !track.growth_limit.is_finite() {
                track.growth_limit = track
                    .base
                    .max(0.0)
                    .min(track.fit_content_limit)
                    .max(track.base);
            }
        }
        return;
    }
    let mut planned = vec![0.0; tracks.tracks.len()];
    let mut touched = Vec::<usize>::new();
    let mut affected_scratch = Vec::<DistributionEntry>::new();
    let mut non_affected_scratch = Vec::<DistributionEntry>::new();
    let mut start = 0;
    let use_limited_min_content = matches!(
        available,
        AvailableSpace::MinContent | AvailableSpace::MaxContent
    );
    let spanning_minimum_kind = if use_limited_min_content {
        ContributionKind::MinContent
    } else {
        ContributionKind::Minimum
    };
    while start < non_flexible.len() {
        let span = items[non_flexible[start]].span(axis);
        let mut end = start + 1;
        while end < non_flexible.len() && items[non_flexible[end]].span(axis) == span {
            end += 1;
        }
        let group = &non_flexible[start..end];
        run_spanning_base_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            inner_size,
            spanning_minimum_kind,
            use_limited_min_content,
            false,
            |track| track.intrinsic_min,
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
        run_spanning_base_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            inner_size,
            ContributionKind::MinContent,
            false,
            false,
            |track| {
                matches!(
                    track.sizing.min,
                    MinTrackSizingFunction::MinContent | MinTrackSizingFunction::MaxContent
                )
            },
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
        if available == AvailableSpace::MaxContent {
            run_spanning_base_phase(
                source,
                session,
                axis,
                tracks,
                cross_tracks,
                items,
                group,
                inner_size,
                ContributionKind::MaxContent,
                true,
                false,
                |track| {
                    matches!(
                        track.sizing.min,
                        MinTrackSizingFunction::Auto | MinTrackSizingFunction::MaxContent
                    )
                },
                &mut planned,
                &mut touched,
                &mut affected_scratch,
                &mut non_affected_scratch,
            );
        }
        run_spanning_base_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            inner_size,
            ContributionKind::MaxContent,
            false,
            false,
            |track| matches!(track.sizing.min, MinTrackSizingFunction::MaxContent),
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
        run_spanning_growth_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            inner_size,
            ContributionKind::MinContent,
            |track| track.intrinsic_max,
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
        run_spanning_growth_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            inner_size,
            ContributionKind::MaxContent,
            |track| {
                matches!(
                    track.sizing.max,
                    MaxTrackSizingFunction::MaxContent
                        | MaxTrackSizingFunction::Auto
                        | MaxTrackSizingFunction::FitContent(_)
                )
            },
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
        for track in &mut tracks.tracks {
            track.infinitely_growable = false;
        }
        start = end;
    }

    // Step 4 considers every item crossing a flexible track together. The
    // flex-factor weighting includes the specified <1 remainder rule.
    if !crosses_flexible.is_empty() {
        // Under a max-content constraint, flexible tracks' intrinsic base
        // growth is driven by the item's max-content contribution. Using the
        // automatic minimum here loses all contribution for ordinary
        // measured items and makes sub-unit/zero `fr` tracks collapse before
        // §12.7 can find a flex fraction.
        let spanned_flex_sum = crosses_flexible
            .iter()
            .flat_map(|&index| {
                let span = span_for(&items[index], axis);
                tracks.tracks[tracks.span_indices(span.start, span.end).clone()]
                    .iter()
                    .filter(|track| track.is_flexible())
                    .map(|track| track.flex_factor)
            })
            .sum::<f32>();
        let flexible_base_kind =
            if available != AvailableSpace::MinContent && spanned_flex_sum < 1.0 {
                ContributionKind::MaxContent
            } else {
                spanning_minimum_kind
            };
        run_spanning_base_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            &crosses_flexible,
            inner_size,
            flexible_base_kind,
            use_limited_min_content,
            true,
            |track| track.is_flexible(),
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
        run_spanning_base_phase(
            source,
            session,
            axis,
            tracks,
            cross_tracks,
            items,
            &crosses_flexible,
            inner_size,
            ContributionKind::MinContent,
            false,
            true,
            |track| {
                track.is_flexible()
                    && matches!(
                        track.sizing.min,
                        MinTrackSizingFunction::MinContent | MinTrackSizingFunction::MaxContent
                    )
            },
            &mut planned,
            &mut touched,
            &mut affected_scratch,
            &mut non_affected_scratch,
        );
    }

    // Step 5 resolves every remaining infinity, including flexible tracks.
    for track in &mut tracks.tracks {
        if !track.growth_limit.is_finite() {
            track.growth_limit = track
                .base
                .max(0.0)
                .min(track.fit_content_limit)
                .max(track.base);
        }
    }
}

fn maximize_tracks(tracks: &mut TrackSet, available: AvailableSpace) {
    let AvailableSpace::Definite(space) = available else {
        if matches!(available, AvailableSpace::MaxContent) {
            for track in &mut tracks.tracks {
                if track.growth_limit.is_finite() && !track.is_flexible() {
                    track.base = track.growth_limit.max(track.base);
                }
            }
        }
        return;
    };
    let mut remaining = (space - tracks.used_size()).max(0.0);
    if remaining <= 0.0 {
        return;
    }
    let mut active = tracks
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| !track.collapsed && !track.is_flexible())
        .map(|(index, track)| (index, (track.growth_limit - track.base).max(0.0)))
        .filter(|&(_, capacity)| capacity > 0.0)
        .collect::<Vec<_>>();
    active.sort_unstable_by(|left, right| left.1.total_cmp(&right.1));
    let mut cursor = 0;
    while cursor < active.len() {
        let count = active.len() - cursor;
        let share = remaining / count as f32;
        let capacity = active[cursor].1;
        if capacity <= share {
            tracks.tracks[active[cursor].0].base += capacity;
            remaining -= capacity;
            cursor += 1;
        } else {
            for &(index, _) in &active[cursor..] {
                tracks.tracks[index].base += share;
            }
            return;
        }
    }
}

/// Finds an `fr` size by sorting freeze thresholds once instead of restarting
/// and rescanning every track for each newly inflexible track (§12.7.1).
fn find_fr_size(
    tracks: &TrackSet,
    range: core::ops::Range<usize>,
    space_to_fill: f32,
    scratch: &mut Vec<(usize, f32)>,
) -> f32 {
    let mut remaining = space_to_fill - span_gap(tracks, range.clone());
    let mut factor_sum = 0.0;
    scratch.clear();
    for index in range {
        let track = &tracks.tracks[index];
        if track.is_flexible() {
            factor_sum += track.flex_factor;
            let threshold = if track.flex_factor > 0.0 {
                track.base / track.flex_factor
            } else if track.base > 0.0 {
                f32::INFINITY
            } else {
                0.0
            };
            scratch.push((index, threshold));
        } else {
            remaining -= track.base;
        }
    }
    scratch.sort_unstable_by(|left, right| right.1.total_cmp(&left.1));
    let mut cursor = 0;
    loop {
        let fraction = remaining.max(0.0) / factor_sum.max(1.0);
        let Some(&(index, threshold)) = scratch.get(cursor) else {
            return fraction;
        };
        if threshold <= fraction {
            return fraction;
        }
        remaining -= tracks.tracks[index].base;
        factor_sum -= tracks.tracks[index].flex_factor;
        cursor += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_flexible_tracks<Source, Session>(
    source: &Source,
    session: &mut Session,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<&TrackSet>,
    items: &mut [GridItem],
    inner_size: crate::geometry::Size<Option<f32>>,
    available: AvailableSpace,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    if !tracks.tracks.iter().any(|track| track.is_flexible()) {
        return;
    }

    // Grid §12.7: a min-content constraint forces the used flex fraction to
    // zero. Intrinsic sizing has already established each flexible track's
    // base, so no max-content item probes are needed in this branch.
    if available == AvailableSpace::MinContent {
        return;
    }

    let definite_space = match available {
        AvailableSpace::Definite(space) => Some(space),
        _ => None,
    };
    let mut flex_fraction = 0.0_f32;
    let mut scratch = Vec::<(usize, f32)>::new();
    if let Some(space) = definite_space {
        flex_fraction = find_fr_size(tracks, 0..tracks.tracks.len(), space, &mut scratch);
    } else {
        for track in &tracks.tracks {
            if track.is_flexible() {
                let candidate = if track.flex_factor > 1.0 {
                    track.base / track.flex_factor
                } else {
                    track.base
                };
                flex_fraction = flex_fraction.max(candidate);
            }
        }
        for item in items.iter_mut() {
            let span = span_for(item, axis);
            let range = tracks.span_indices(span.start, span.end);
            if !tracks.tracks[range.clone()]
                .iter()
                .any(|track| track.is_flexible())
            {
                continue;
            }
            let contribution = measure_contribution(
                source,
                session,
                item,
                axis,
                ContributionKind::MaxContent,
                tracks,
                cross_tracks,
                inner_size,
            );
            flex_fraction =
                flex_fraction.max(find_fr_size(tracks, range, contribution, &mut scratch));
        }
    }
    for track in &mut tracks.tracks {
        if track.is_flexible() {
            track.base = track.base.max(flex_fraction * track.flex_factor);
        }
    }
}

fn stretch_auto_tracks(tracks: &mut TrackSet, available: AvailableSpace, alignment: AlignContent) {
    if alignment != AlignContent::Stretch {
        return;
    }
    let AvailableSpace::Definite(space) = available else {
        return;
    };
    let free = (space - tracks.used_size()).max(0.0);
    let count = tracks
        .tracks
        .iter()
        .filter(|track| track.auto_max && !track.collapsed)
        .count();
    if count == 0 || free <= 0.0 {
        return;
    }
    let share = free / count as f32;
    for track in &mut tracks.tracks {
        if track.auto_max && !track.collapsed {
            track.base += share;
            track.growth_limit = track.growth_limit.max(track.base);
        }
    }
}

/// Runs the track sizing algorithm for one physical axis.
#[allow(clippy::too_many_arguments)]
pub(super) fn size_tracks<Source, Session>(
    source: &Source,
    session: &mut Session,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<&TrackSet>,
    items: &mut [GridItem],
    inner_size: crate::geometry::Size<Option<f32>>,
    available: AvailableSpace,
    alignment: AlignContent,
) where
    Source: GridSource,
    Session: LayoutSession<Source>,
{
    if tracks.tracks.is_empty() {
        return;
    }
    if tracks
        .tracks
        .iter()
        .all(|track| !track.intrinsic_min && !track.intrinsic_max && !track.is_flexible())
    {
        maximize_tracks(tracks, available);
        tracks.rebuild_positions();
        return;
    }
    for item in items.iter_mut() {
        resolve_item_intrinsic_dimensions(source, session, item, axis, cross_tracks, inner_size);
    }
    prepare_baseline_shims(
        source,
        session,
        axis,
        tracks,
        cross_tracks,
        items,
        inner_size,
    );
    resolve_intrinsic_sizes(
        source,
        session,
        axis,
        tracks,
        cross_tracks,
        items,
        inner_size,
        available,
    );
    maximize_tracks(tracks, available);
    expand_flexible_tracks(
        source,
        session,
        axis,
        tracks,
        cross_tracks,
        items,
        inner_size,
        available,
    );
    stretch_auto_tracks(tracks, available, alignment);
    tracks.rebuild_positions();
}

trait ItemOverflowAxis {
    fn overflow_point(&self, axis: Axis) -> crate::style::Overflow;
}

impl ItemOverflowAxis for GridItem {
    #[inline]
    fn overflow_point(&self, axis: Axis) -> crate::style::Overflow {
        match axis {
            Axis::Horizontal => self.overflow.x,
            Axis::Vertical => self.overflow.y,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::compute::grid::placement::{GridArea, TrackSpan};
    use crate::geometry::{Edges, Point, Size};
    use crate::style::{
        BoxSizing, CalcHandle, CoreStyle, GridContainerStyle, GridItemStyle, GridTemplateComponent,
        GridTemplateRepetition, LengthPercentage, Overflow, RepetitionCount, TrackSizingFunction,
    };
    use crate::tree::{
        CacheState, GridSource, Layout, LayoutInput, LayoutOutput, LayoutSource, LayoutState,
        NodeId, TraverseTree,
    };

    #[derive(Debug, Clone, Copy)]
    struct TestStyle {
        size: Size<Dimension>,
        min_size: Size<Dimension>,
        max_size: Size<Dimension>,
    }

    impl Default for TestStyle {
        fn default() -> Self {
            Self {
                size: Size::new(Dimension::Auto, Dimension::Auto),
                min_size: Size::new(Dimension::Auto, Dimension::Auto),
                max_size: Size::new(Dimension::Auto, Dimension::Auto),
            }
        }
    }

    impl CoreStyle for TestStyle {
        fn size(&self) -> Size<Dimension> {
            self.size
        }

        fn min_size(&self) -> Size<Dimension> {
            self.min_size
        }

        fn max_size(&self) -> Size<Dimension> {
            self.max_size
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct EmptyRepetition;

    impl GridTemplateRepetition for EmptyRepetition {
        type Tracks<'a> = core::iter::Empty<TrackSizingFunction>;

        fn count(&self) -> RepetitionCount {
            RepetitionCount::Count(1)
        }

        fn tracks(&self) -> Self::Tracks<'_> {
            core::iter::empty()
        }
    }

    impl GridContainerStyle for TestStyle {
        type Repetition<'a> = EmptyRepetition;
        type TemplateTracks<'a> = core::iter::Empty<GridTemplateComponent<EmptyRepetition>>;
        type AutoTracks<'a> = core::iter::Empty<TrackSizingFunction>;

        fn grid_template_rows(&self) -> Self::TemplateTracks<'_> {
            core::iter::empty()
        }

        fn grid_template_columns(&self) -> Self::TemplateTracks<'_> {
            core::iter::empty()
        }

        fn grid_auto_rows(&self) -> Self::AutoTracks<'_> {
            core::iter::empty()
        }

        fn grid_auto_columns(&self) -> Self::AutoTracks<'_> {
            core::iter::empty()
        }
    }

    impl GridItemStyle for TestStyle {}

    #[derive(Debug, Default)]
    struct TestSource {
        style: TestStyle,
    }

    impl TraverseTree for TestSource {
        type ChildIter<'a> = core::iter::Empty<NodeId>;

        fn child_ids(&self, _parent: NodeId) -> Self::ChildIter<'_> {
            core::iter::empty()
        }

        fn child_count(&self, _parent: NodeId) -> usize {
            0
        }

        fn child_id(&self, _parent: NodeId, _index: usize) -> NodeId {
            unreachable!("the sizing test source has no children")
        }
    }

    impl LayoutSource for TestSource {
        type CoreStyle<'a> = &'a TestStyle;

        fn core_style(&self, _node: NodeId) -> Self::CoreStyle<'_> {
            &self.style
        }

        fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
            unreachable!("sizing test styles do not contain calc()")
        }
    }

    impl GridSource for TestSource {
        type ContainerStyle<'a> = &'a TestStyle;
        type ItemStyle<'a> = &'a TestStyle;

        fn grid_container_style(&self, _container: NodeId) -> Self::ContainerStyle<'_> {
            &self.style
        }

        fn grid_item_style(&self, _item: NodeId) -> Self::ItemStyle<'_> {
            &self.style
        }
    }

    #[derive(Debug)]
    struct TestSession {
        min_content: Size<f32>,
        max_content: Size<f32>,
        first_baseline: Option<f32>,
        calls: Vec<LayoutInput>,
    }

    impl Default for TestSession {
        fn default() -> Self {
            Self {
                min_content: Size::new(20.0, 10.0),
                max_content: Size::new(80.0, 40.0),
                first_baseline: None,
                calls: Vec::new(),
            }
        }
    }

    impl LayoutState for TestSession {
        fn set_unrounded_layout(&mut self, _node: NodeId, _layout: &Layout) {}

        fn set_static_position(&mut self, _child: NodeId, _static_position: Point<f32>) {}
    }

    impl CacheState for TestSession {
        fn cache_get(&self, _node: NodeId, _input: LayoutInput) -> Option<LayoutOutput> {
            None
        }

        fn cache_store(&mut self, _node: NodeId, _input: LayoutInput, _output: LayoutOutput) {}

        fn cache_clear(&mut self, _node: NodeId) {}
    }

    impl LayoutSession<TestSource> for TestSession {
        fn compute_child_layout(
            &mut self,
            _source: &TestSource,
            _child: NodeId,
            input: LayoutInput,
        ) -> LayoutOutput {
            self.calls.push(input);
            let measured = Size::new(
                match input.available_space.width {
                    AvailableSpace::MinContent => self.min_content.width,
                    AvailableSpace::MaxContent => self.max_content.width,
                    AvailableSpace::Definite(value) => value,
                },
                match input.available_space.height {
                    AvailableSpace::MinContent => self.min_content.height,
                    AvailableSpace::MaxContent => self.max_content.height,
                    AvailableSpace::Definite(value) => value,
                },
            );
            let size = Size::new(
                input.known_dimensions.width.unwrap_or(measured.width),
                input.known_dimensions.height.unwrap_or(measured.height),
            );
            LayoutOutput::new(size, size)
                .with_first_baselines(Point::new(None, self.first_baseline))
        }
    }

    fn test_item(column_start: i32, column_end: i32) -> GridItem {
        GridItem {
            key: crate::compute::util::ItemKey {
                node: NodeId::from(0_usize),
                layout_order: 0,
            },
            area: GridArea {
                column: TrackSpan {
                    start: column_start,
                    end: column_end,
                },
                row: TrackSpan { start: 0, end: 1 },
            },
            align_self: crate::style::AlignItems::Start,
            justify_self: crate::style::AlignItems::Start,
            direction: crate::style::Direction::Ltr,
            aspect_ratio: None,
            box_sizing: BoxSizing::ContentBox,
            overflow: Point::new(Overflow::Visible, Overflow::Visible),
            preferred_behaves_auto_or_depends: Size::new(true, true),
            minimum_is_auto: Size::new(true, true),
            preferred_size: Size::NONE,
            min_size: Size::NONE,
            max_size: Size::NONE,
            margin: Edges::uniform(0.0),
            margin_auto: Edges::uniform(false),
            padding: Edges::uniform(0.0),
            border: Edges::uniform(0.0),
            scrollbar: Size::ZERO,
            inset: Edges::uniform(None),
            raw_min_content: Size::NONE,
            raw_max_content: Size::NONE,
            minimum_contribution: Size::NONE,
            min_content_contribution: Size::NONE,
            max_content_contribution: Size::NONE,
            measured_baselines: Point::NONE,
            baseline_shim: 0.0,
        }
    }

    fn test_track(base: f32, growth_limit: f32) -> Track {
        Track {
            sizing: TrackSizingFunction::AUTO,
            base,
            growth_limit,
            fit_content_limit: f32::INFINITY,
            flex_factor: 0.0,
            flexible: false,
            intrinsic_min: false,
            intrinsic_max: false,
            auto_max: false,
            infinitely_growable: false,
            collapsed: false,
            position: 0.0,
        }
    }

    fn track_set(tracks: Vec<Track>) -> TrackSet {
        TrackSet {
            tracks,
            gap: 0.0,
            first_coordinate: 0,
            collapsed_line_positions: None,
        }
    }

    #[test]
    fn intrinsic_keywords_resolve_each_raw_dimension_form() {
        let source = TestSource {
            style: TestStyle {
                size: Size::new(Dimension::MinContent, Dimension::Auto),
                min_size: Size::new(Dimension::MaxContent, Dimension::Auto),
                max_size: Size::new(
                    Dimension::FitContent(LengthPercentage::length(50.0)),
                    Dimension::Auto,
                ),
            },
        };
        let mut session = TestSession::default();
        let mut item = test_item(0, 1);
        resolve_item_intrinsic_dimensions(
            &source,
            &mut session,
            &mut item,
            Axis::Horizontal,
            None,
            Size::new(Some(100.0), None),
        );
        assert_eq!(item.preferred_size.width, Some(20.0));
        assert_eq!(item.min_size.width, Some(80.0));
        assert_eq!(item.max_size.width, Some(50.0));
        assert_eq!(session.calls.len(), 2);

        let source = TestSource {
            style: TestStyle {
                min_size: Size::new(Dimension::MinContent, Dimension::Auto),
                ..TestStyle::default()
            },
        };
        let mut session = TestSession::default();
        let mut item = test_item(0, 1);
        resolve_item_intrinsic_dimensions(
            &source,
            &mut session,
            &mut item,
            Axis::Horizontal,
            None,
            Size::NONE,
        );
        assert_eq!(item.preferred_size.width, None);
        assert_eq!(item.min_size.width, Some(20.0));
        assert_eq!(item.max_size.width, None);
        assert_eq!(session.calls.len(), 1);

        let source = TestSource {
            style: TestStyle {
                size: Size::new(Dimension::MaxContent, Dimension::Auto),
                ..TestStyle::default()
            },
        };
        let mut session = TestSession::default();
        let mut item = test_item(0, 1);
        resolve_item_intrinsic_dimensions(
            &source,
            &mut session,
            &mut item,
            Axis::Horizontal,
            None,
            Size::NONE,
        );
        assert_eq!(item.preferred_size.width, Some(80.0));
        assert_eq!(session.calls.len(), 1);
    }

    #[test]
    fn vertical_border_box_ratio_and_synthesized_baseline_affect_raw_content() {
        let source = TestSource::default();
        let mut session = TestSession::default();
        let mut item = test_item(0, 1);
        item.preferred_size.width = Some(40.0);
        item.aspect_ratio = Some(2.0);
        item.box_sizing = BoxSizing::BorderBox;
        item.align_self = crate::style::AlignItems::Baseline;

        let measured = raw_content_size(
            &source,
            &mut session,
            &mut item,
            Axis::Vertical,
            ContributionKind::MinContent,
            None,
            Size::NONE,
        );
        assert_eq!(measured, 20.0);
        assert_eq!(item.measured_baselines.y, Some(10.0));
    }

    #[test]
    fn non_auto_contribution_without_a_preferred_size_falls_back_to_content() {
        let source = TestSource::default();
        let mut session = TestSession::default();
        let mut item = test_item(0, 1);
        item.preferred_behaves_auto_or_depends.width = false;
        let tracks = track_set(vec![test_track(0.0, f32::INFINITY)]);

        let minimum = measure_contribution(
            &source,
            &mut session,
            &mut item,
            Axis::Horizontal,
            ContributionKind::Minimum,
            &tracks,
            None,
            Size::NONE,
        );
        let maximum = measure_contribution(
            &source,
            &mut session,
            &mut item,
            Axis::Horizontal,
            ContributionKind::MaxContent,
            &tracks,
            None,
            Size::NONE,
        );
        assert_eq!(minimum, 20.0);
        assert_eq!(maximum, 80.0);
        assert_eq!(session.calls.len(), 2);
    }

    #[test]
    fn redistribution_uses_preferred_tracks_after_caps_are_exhausted() {
        let mut remaining = 3.0;
        distribute_up_to_limits(&mut [], &mut remaining);
        assert_eq!(remaining, 3.0);

        let mut collapsed = test_track(0.0, 0.0);
        collapsed.collapsed = true;
        let tracks = track_set(vec![collapsed]);
        let mut planned = vec![0.0];
        let mut touched = Vec::new();
        let mut affected = Vec::new();
        let mut non_affected = Vec::new();
        distribute_extra(
            &tracks,
            0..1,
            4.0,
            PlannedSize::Base,
            ContributionKind::Minimum,
            false,
            |_| true,
            &mut planned,
            &mut touched,
            &mut affected,
            &mut non_affected,
        );
        assert!(touched.is_empty());

        let cases = [
            (
                PlannedSize::Base,
                ContributionKind::Minimum,
                TrackSizingFunction::AUTO,
                true,
            ),
            (
                PlannedSize::Base,
                ContributionKind::MaxContent,
                TrackSizingFunction::AUTO,
                true,
            ),
            (
                PlannedSize::GrowthLimit,
                ContributionKind::MaxContent,
                TrackSizingFunction::AUTO,
                true,
            ),
            (
                PlannedSize::Base,
                ContributionKind::MaxContent,
                TrackSizingFunction::fixed(LengthPercentage::ZERO),
                false,
            ),
        ];
        for (planned_size, kind, sizing, intrinsic_max) in cases {
            let mut track = test_track(0.0, 0.0);
            track.sizing = sizing;
            track.intrinsic_max = intrinsic_max;
            let tracks = track_set(vec![track]);
            let mut planned = vec![0.0];
            let mut touched = Vec::new();
            distribute_extra(
                &tracks,
                0..1,
                12.0,
                planned_size,
                kind,
                false,
                |_| true,
                &mut planned,
                &mut touched,
                &mut Vec::new(),
                &mut Vec::new(),
            );
            assert_eq!(planned, [12.0]);
            assert_eq!(touched, [0]);
        }
    }

    #[test]
    fn growth_maximization_fr_freezing_and_auto_stretch_cover_limit_edges() {
        let mut tracks = track_set(vec![test_track(0.0, 100.0), test_track(0.0, 100.0)]);
        maximize_tracks(&mut tracks, AvailableSpace::Definite(10.0));
        assert_eq!(tracks.tracks[0].base, 5.0);
        assert_eq!(tracks.tracks[1].base, 5.0);

        let mut finite_growth = track_set(vec![test_track(1.0, 5.0)]);
        let mut planned = vec![2.0];
        let mut touched = vec![0];
        apply_planned_growth(&mut finite_growth, &mut planned, &mut touched);
        assert_eq!(finite_growth.tracks[0].growth_limit, 7.0);
        assert!(!finite_growth.tracks[0].infinitely_growable);

        let mut zero_factor = test_track(10.0, f32::INFINITY);
        zero_factor.flexible = true;
        let zero_factor = track_set(vec![zero_factor]);
        let fraction = find_fr_size(&zero_factor, 0..1, 50.0, &mut Vec::new());
        assert_eq!(fraction, 40.0);

        let mut auto = test_track(10.0, 10.0);
        auto.auto_max = true;
        let mut tracks = track_set(vec![auto]);
        stretch_auto_tracks(
            &mut tracks,
            AvailableSpace::Definite(50.0),
            AlignContent::Stretch,
        );
        assert_eq!(tracks.tracks[0].base, 50.0);
        assert_eq!(tracks.tracks[0].growth_limit, 50.0);
    }

    #[test]
    fn indefinite_fr_sizing_considers_only_items_crossing_flexible_tracks() {
        let source = TestSource::default();
        let mut session = TestSession::default();
        let fixed = test_track(10.0, 10.0);
        let mut flexible = test_track(10.0, f32::INFINITY);
        flexible.sizing = TrackSizingFunction::fr(2.0);
        flexible.flex_factor = 2.0;
        flexible.flexible = true;
        let mut subunit_flexible = test_track(7.0, f32::INFINITY);
        subunit_flexible.sizing = TrackSizingFunction::fr(0.5);
        subunit_flexible.flex_factor = 0.5;
        subunit_flexible.flexible = true;
        let mut tracks = track_set(vec![fixed, flexible, subunit_flexible]);
        let mut items = vec![test_item(0, 1), test_item(1, 2)];

        expand_flexible_tracks(
            &source,
            &mut session,
            Axis::Horizontal,
            &mut tracks,
            None,
            &mut items,
            Size::NONE,
            AvailableSpace::MaxContent,
        );
        assert_eq!(tracks.tracks[0].base, 10.0);
        assert_eq!(tracks.tracks[1].base, 80.0);
        assert_eq!(tracks.tracks[2].base, 20.0);
        assert_eq!(session.calls.len(), 1);
        assert_eq!(items[0].overflow_point(Axis::Vertical), Overflow::Visible);
    }
}
