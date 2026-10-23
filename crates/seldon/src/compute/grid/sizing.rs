//! CSS Grid §12 track sizing.

#![allow(clippy::cast_precision_loss)]

use stylo::computed_values::box_sizing;
use stylo::values::computed::TrackBreadth;
use stylo::values::specified::align::AlignFlags;

use super::super::util::{BaselineSet, clamp, resolve_intrinsic, resolve_length_percentage};
use super::placement::TrackSpan;
use super::tracks::{AxisTrackSpec, fit_content_limit, max_breadth, min_breadth};
use super::types::{Axis, GridItem, MaxKind, MinKind, Track, TrackSet};
use crate::style::CoreStyle;
use crate::tree::{AvailableSpace, LayoutInput, LayoutTree, SizingMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContributionKind {
    Minimum,
    MinContent,
    MaxContent,
}

#[inline]
fn available_for(kind: ContributionKind) -> AvailableSpace {
    match kind {
        ContributionKind::Minimum | ContributionKind::MinContent => AvailableSpace::MinContent,
        ContributionKind::MaxContent => AvailableSpace::MaxContent,
    }
}

pub(super) fn initialize_tracks(
    set: &mut TrackSet,
    specs: &[AxisTrackSpec<'_>],
    percentage_basis: Option<f32>,
    gap: f32,
) {
    set.tracks.clear();
    set.tracks
        .extend(specs.iter().map(|spec| plan_track(spec, percentage_basis)));
    set.gap = gap;
    set.collapsed_line_positions = None;
}

/// §12.4: one track's sizing-function kinds and initial used state against
/// one percentage basis.
fn plan_track(spec: &AxisTrackSpec<'_>, percentage_basis: Option<f32>) -> Track {
    let minimum = min_breadth(spec.sizing);
    let maximum = max_breadth(spec.sizing);
    let min_fixed = match minimum {
        TrackBreadth::Breadth(value) => resolve_length_percentage(value, percentage_basis),
        _ => None,
    };
    let max_fixed = match maximum {
        TrackBreadth::Breadth(value) => resolve_length_percentage(value, percentage_basis),
        _ => None,
    };
    let min = match minimum {
        TrackBreadth::Breadth(_) if min_fixed.is_some() => MinKind::Fixed,
        TrackBreadth::Breadth(_) | TrackBreadth::Auto | TrackBreadth::Flex(_) => MinKind::Auto,
        TrackBreadth::MinContent => MinKind::MinContent,
        TrackBreadth::MaxContent => MinKind::MaxContent,
    };
    let max = match maximum {
        TrackBreadth::Breadth(_) => max_fixed.map_or(MaxKind::Auto, |maximum| {
            MaxKind::Fixed(maximum.max(min_fixed.unwrap_or(0.0)).max(0.0))
        }),
        TrackBreadth::Auto => MaxKind::Auto,
        TrackBreadth::MinContent => MaxKind::MinContent,
        TrackBreadth::MaxContent => MaxKind::MaxContent,
        TrackBreadth::Flex(_) => MaxKind::Flex,
    };
    let fit_content_limit = fit_content_limit(spec.sizing).map_or(f32::INFINITY, |value| {
        resolve_length_percentage(value, percentage_basis).unwrap_or(f32::INFINITY)
    });
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
        match maximum {
            TrackBreadth::Flex(flex) if flex.0.is_finite() && flex.0 > 0.0 => flex.0,
            _ => 0.0,
        }
    };
    Track {
        min,
        max,
        base,
        growth_limit,
        fit_content_limit,
        flex_factor,
        infinitely_growable: false,
        collapsed,
        position: 0.0,
    }
}

#[inline]
fn span_for<N>(item: &GridItem<N>, axis: Axis) -> super::placement::TrackSpan {
    match axis {
        Axis::Horizontal => item.area.column,
        Axis::Vertical => item.area.row,
    }
}

#[inline]
fn margin_sum<N>(item: &GridItem<N>, axis: Axis) -> f32 {
    axis.sum(item.margin)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum CrossAxisTracks<'a> {
    DefiniteMaximums {
        tracks: &'a TrackSet,
        distributed_gap: f32,
    },
    Resolved(&'a TrackSet),
}

impl<'a> CrossAxisTracks<'a> {
    #[inline]
    pub(super) const fn resolved(tracks: &'a TrackSet) -> Self {
        Self::Resolved(tracks)
    }
}

fn cross_area_size<N>(
    item: &GridItem<N>,
    axis: Axis,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> Option<f32> {
    let cross = axis.other();
    let span = span_for(item, cross);
    let size = match cross_tracks? {
        CrossAxisTracks::DefiniteMaximums {
            tracks,
            distributed_gap,
        } => tracks.definite_max_area_size(span.start, span.end, distributed_gap)?,
        CrossAxisTracks::Resolved(tracks) => tracks.area_size(span.start, span.end),
    };
    Some((size - margin_sum(item, cross)).max(0.0))
}

fn raw_content_size<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    kind: ContributionKind,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> f32
where
    T: LayoutTree,
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
    let cross_stretches = item.stretches(cross);
    let mut known = crate::geometry::Size::NONE;
    let resolved_cross = cross
        .size(item.preferred_size)
        .or_else(|| cross_stretches.then_some(cross_area).flatten())
        .map(|value| clamp(value, cross.size(item.min_size), cross.size(item.max_size)));
    cross.set_size(&mut known, resolved_cross);
    // css-grid-1 §11.5 with css-sizing-4 §5: an item with a preferred aspect
    // ratio contributes the size that ratio *transfers*, so the transfer needs
    // a size in the other axis. A definite one — an authored size, or a track
    // the item is stretched into — is the whole answer. Failing that, replaced
    // content still has its natural size in that axis to transfer from, which
    // is what an image carrying its own `aspect-ratio` property contributes to
    // an `auto` track.
    let transferred_cross = resolved_cross.or_else(|| {
        item.aspect_ratio?;
        cross
            .size(tree.style(item.key.node).natural_size())
            .map(|value| clamp(value, cross.size(item.min_size), cross.size(item.max_size)))
    });

    let target_available = available_for(kind);
    let cross_available = cross_area.map_or(AvailableSpace::MaxContent, AvailableSpace::Definite);
    let available = match axis {
        Axis::Horizontal => crate::geometry::Size::new(target_available, cross_available),
        Axis::Vertical => crate::geometry::Size::new(cross_available, target_available),
    };
    let parent_size = match axis {
        Axis::Horizontal => crate::geometry::Size::new(None, cross_area),
        Axis::Vertical => crate::geometry::Size::new(cross_area, None),
    };
    // A row's last-baseline group reads the item's last baseline, its §11.5
    // shim included: ask for it exactly.
    let reads_last_baseline =
        axis == Axis::Vertical && item.baseline_participation() == Some(BaselineSet::Last);
    let reads_first_baseline = axis == Axis::Vertical && item.align_self == AlignFlags::BASELINE;
    let mut input = LayoutInput::measure(known, parent_size, available, axis.requested())
        .reading_last_baseline(reads_last_baseline)
        .reading_first_baseline(reads_first_baseline);
    input.sizing_mode = SizingMode::IgnoreSizeStyles;
    let output = tree.compute_layout(state, item.key.node, input);
    let size = output.size;
    if axis == Axis::Vertical {
        // A box with no baseline set is aligned by one synthesized from its
        // border box's bottom edge (css-align-3 §9.1).
        if item.align_self == AlignFlags::BASELINE {
            item.measured_baseline = Some(output.first_baseline.unwrap_or(size.height));
        } else if item.align_self == AlignFlags::LAST_BASELINE {
            item.measured_last_descent =
                Some(size.height - output.last_baseline.unwrap_or(size.height));
        }
    }
    let mut measured = axis.size(size);
    if let (Some(ratio), Some(cross_size)) = (item.aspect_ratio, transferred_cross)
        && ratio.is_finite()
        && ratio > 0.0
    {
        let axis_inset = axis.sum(item.padding) + axis.sum(item.border);
        let cross_inset = cross.sum(item.padding) + cross.sum(item.border);
        let sizing_cross = if item.box_sizing == box_sizing::T::ContentBox {
            (cross_size - cross_inset).max(0.0)
        } else {
            cross_size
        };
        let sizing_axis = match axis {
            Axis::Horizontal => sizing_cross * ratio,
            Axis::Vertical => sizing_cross / ratio,
        };
        let ratio_size = if item.box_sizing == box_sizing::T::ContentBox {
            sizing_axis + axis_inset
        } else {
            sizing_axis
        };
        // The transferred size *is* the contribution — it does not merely
        // floor the measured one. A ratio flatter than the content's own is
        // meant to cut the box down and let the content overflow.
        measured = ratio_size;
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

pub(super) fn probe_raw_min_content<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> f32
where
    T: LayoutTree,
{
    raw_content_size(
        tree,
        state,
        item,
        axis,
        ContributionKind::MinContent,
        cross_tracks,
    )
}

pub(super) fn resolve_item_intrinsic_dimensions<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    inner_size: crate::geometry::Size<Option<f32>>,
) where
    T: LayoutTree,
{
    let needs_min_content = item.intrinsic.needs_min_content(axis);
    let needs_max_content = item.intrinsic.needs_max_content(axis);
    if !needs_min_content && !needs_max_content {
        return;
    }

    let min_content = needs_min_content.then(|| {
        raw_content_size(
            tree,
            state,
            item,
            axis,
            ContributionKind::MinContent,
            cross_tracks,
        )
    });
    let max_content = needs_max_content.then(|| {
        raw_content_size(
            tree,
            state,
            item,
            axis,
            ContributionKind::MaxContent,
            cross_tracks,
        )
    });
    let style = tree.style(item.key.node);
    let size = axis.size(style.size());
    let min_size = axis.size(style.min_size());
    let max_size = axis.size(style.max_size());
    let basis = axis.size(inner_size);
    if axis.size(item.preferred_size).is_none() {
        let resolved = resolve_intrinsic(
            item.intrinsic.preferred(axis),
            size,
            None,
            min_content,
            max_content,
            basis,
            0.0,
            box_sizing::T::BorderBox,
        );
        axis.set_size(&mut item.preferred_size, resolved);
    }
    if axis.size(item.min_size).is_none() {
        let resolved = resolve_intrinsic(
            item.intrinsic.minimum(axis),
            min_size,
            None,
            min_content,
            max_content,
            basis,
            0.0,
            box_sizing::T::BorderBox,
        );
        axis.set_size(&mut item.min_size, resolved);
    }
    if axis.size(item.max_size).is_none() {
        let resolved = resolve_intrinsic(
            item.intrinsic.maximum(axis),
            max_size,
            None,
            min_content,
            max_content,
            basis,
            0.0,
            box_sizing::T::BorderBox,
        );
        axis.set_size(&mut item.max_size, resolved);
    }
}

#[allow(clippy::too_many_arguments)]
fn measure_contribution<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    kind: ContributionKind,
    tracks: &TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> f32
where
    T: LayoutTree,
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
                let span = span_for(item, axis);
                let indexes = tracks.span_indices(span.start, span.end);
                if minimum_may_be_automatic(item, axis)
                    && automatic_minimum_applies(tracks, indexes.clone(), item.span(axis))
                {
                    let suggestion =
                        automatic_minimum_suggestion(tree, state, item, axis, cross_tracks);
                    fixed_max_span_limit(tracks, indexes)
                        .map_or(suggestion, |limit| suggestion.min(limit))
                } else {
                    minimum_fallback(tree, state, item, axis, cross_tracks)
                }
            }
        }
        ContributionKind::MinContent | ContributionKind::MaxContent => {
            if preferred_behaves_auto_or_depends {
                raw_content_size(tree, state, item, axis, kind, cross_tracks)
                    + margin_sum(item, axis)
            } else {
                preferred.unwrap_or_else(|| {
                    raw_content_size(tree, state, item, axis, kind, cross_tracks)
                        + margin_sum(item, axis)
                })
            }
        }
    };
    contribution = finish_contribution(
        contribution,
        explicit_min,
        explicit_max,
        axis,
        item.baseline_shim,
    );

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

/// The clamp every contribution ends with: the item's own min/max sizes (both
/// margin-box values), a floor at zero, and a row's baseline shim.
#[inline]
fn finish_contribution(
    contribution: f32,
    explicit_min: Option<f32>,
    explicit_max: Option<f32>,
    axis: Axis,
    baseline_shim: f32,
) -> f32 {
    let mut contribution = clamp(contribution, explicit_min, explicit_max).max(0.0);
    if axis == Axis::Vertical {
        contribution += baseline_shim;
    }
    contribution
}

/// Whether the item's minimum contribution is its automatic minimum wherever
/// the tracks it spans allow one (css-grid-2 §6.6): it has no definite
/// minimum size, its minimum size is `auto`, and it is not a scroll
/// container.
#[inline]
fn minimum_may_be_automatic<N>(item: &GridItem<N>, axis: Axis) -> bool {
    axis.size(item.min_size).is_none()
        && axis.size(item.minimum_is_auto)
        && !axis.point(item.overflow).is_scrollable()
}

/// Whether the tracks in `range` give an item spanning `span` of them its
/// automatic minimum (§6.6): one of them has an `auto` (or flexible) minimum,
/// and a spanning item crosses no flexible track.
#[inline]
fn automatic_minimum_applies(
    tracks: &TrackSet,
    range: core::ops::Range<usize>,
    span: usize,
) -> bool {
    tracks.tracks[range.clone()]
        .iter()
        .any(|track| track.min == MinKind::Auto)
        && (span == 1 || !tracks.tracks[range].iter().any(Track::is_flexible))
}

/// §6.6's content-based minimum size as a margin-box value, before the
/// fixed-maximum clamp of the tracks it spans: the min-content size, capped
/// by a preferred size.
fn automatic_minimum_suggestion<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> f32
where
    T: LayoutTree,
{
    let preferred = axis
        .size(item.preferred_size)
        .map(|value| value + margin_sum(item, axis));
    let raw_outer = raw_content_size(
        tree,
        state,
        item,
        axis,
        ContributionKind::MinContent,
        cross_tracks,
    ) + margin_sum(item, axis);
    preferred.map_or(raw_outer, |size| raw_outer.min(size))
}

/// The minimum contribution of an item with no definite minimum size where
/// its automatic minimum does not apply, before the final clamp.
fn minimum_fallback<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> f32
where
    T: LayoutTree,
{
    let margin = margin_sum(item, axis);
    if axis.size(item.preferred_behaves_auto_or_depends) {
        axis.sum(item.padding) + axis.sum(item.border) + margin
    } else if let Some(preferred) = axis.size(item.preferred_size) {
        preferred + margin
    } else {
        raw_content_size(
            tree,
            state,
            item,
            axis,
            ContributionKind::Minimum,
            cross_tracks,
        ) + margin
    }
}

fn span_gap(tracks: &TrackSet, range: core::ops::Range<usize>) -> f32 {
    let visible = tracks.tracks[range]
        .iter()
        .filter(|track| !track.collapsed)
        .count();
    tracks.gap * visible.saturating_sub(1) as f32
}

/// §6.6's fixed-maximum clamp of an automatic minimum across the tracks in
/// `range`: their fixed maximums (each floored by its fixed minimum,
/// [`MaxKind::Fixed`]) and the gutters between them, or `None` when a track
/// that is not collapsed has no fixed maximum.
fn fixed_max_span_limit(tracks: &TrackSet, range: core::ops::Range<usize>) -> Option<f32> {
    let mut limit = span_gap(tracks, range.clone());
    for track in &tracks.tracks[range] {
        if track.collapsed {
            continue;
        }
        let MaxKind::Fixed(maximum) = track.max else {
            return None;
        };
        limit += maximum;
    }
    Some(limit)
}

#[allow(clippy::too_many_arguments)]
fn measure_limited_contribution<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut GridItem<T::NodeId>,
    axis: Axis,
    kind: ContributionKind,
    tracks: &TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
) -> f32
where
    T: LayoutTree,
{
    debug_assert!(matches!(
        kind,
        ContributionKind::MinContent | ContributionKind::MaxContent
    ));
    let contribution = measure_contribution(tree, state, item, axis, kind, tracks, cross_tracks);
    let span = span_for(item, axis);
    let range = tracks.span_indices(span.start, span.end);
    let fixed_limit = fixed_max_span_limit(tracks, range.clone()).or_else(|| {
        (range.len() == 1)
            .then(|| tracks.tracks[range.start].fit_content_limit)
            .filter(|limit| limit.is_finite())
    });
    let Some(limit) = fixed_limit else {
        return contribution;
    };
    let minimum = measure_contribution(
        tree,
        state,
        item,
        axis,
        ContributionKind::Minimum,
        tracks,
        cross_tracks,
    );
    contribution.min(limit).max(minimum)
}

/// css-grid-1 §11.5 step 1: shims baseline-aligned items so their intrinsic
/// row contributions reflect their alignment. A `baseline` item is shimmed
/// on its start side, against the other members of its startmost row's
/// first-baseline group; a `last baseline` item on its end side, against its
/// endmost row's last-baseline group (css-align-3 §9.2: a spanning item
/// joins the startmost or endmost shared alignment context).
fn prepare_baseline_shims<T>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    items: &mut [GridItem<T::NodeId>],
) where
    T: LayoutTree,
{
    if axis != Axis::Vertical {
        return;
    }
    let mut candidates = Vec::<(BaselineSet, i32, usize)>::new();
    for (index, item) in items.iter_mut().enumerate() {
        item.baseline_shim = 0.0;
        let Some(set) = item.baseline_participation() else {
            continue;
        };
        let span = span_for(item, axis);
        debug_assert!(tracks.span_indices(span.start, span.end).start < tracks.tracks.len());
        let line = match set {
            BaselineSet::First => span.start,
            BaselineSet::Last => span.end,
        };
        candidates.push((set, line, index));
    }
    candidates.sort_unstable_by_key(|&(set, line, _)| (set, line));
    // The distance a group aligns: from a member's outer start edge down to
    // its first baseline, or from its last baseline down to its outer end
    // edge.
    let extent = |item: &GridItem<T::NodeId>, set: BaselineSet| match set {
        BaselineSet::First => item
            .measured_baseline
            .map(|baseline| item.margin.top + baseline),
        BaselineSet::Last => item
            .measured_last_descent
            .map(|descent| descent + item.margin.bottom),
    };
    let mut start = 0;
    while start < candidates.len() {
        let (set, line, _) = candidates[start];
        let mut end = start + 1;
        while end < candidates.len() && (candidates[end].0, candidates[end].1) == (set, line) {
            end += 1;
        }
        let group = &candidates[start..end];
        let contribution_uses_shim = group.iter().any(|&(_, _, item_index)| {
            let span = span_for(&items[item_index], axis);
            tracks.tracks[tracks.span_indices(span.start, span.end)]
                .iter()
                .any(|track| {
                    !track.collapsed
                        && (track.min.is_intrinsic()
                            || track.max.is_intrinsic()
                            || track.is_flexible())
                })
        });
        if group.len() > 1 && contribution_uses_shim {
            let mut maximum = None::<f32>;
            for &(_, _, item_index) in group {
                let item = &mut items[item_index];
                let _ = raw_content_size(
                    tree,
                    state,
                    item,
                    axis,
                    ContributionKind::MinContent,
                    cross_tracks,
                );
                if let Some(extent) = extent(item, set) {
                    maximum = Some(maximum.map_or(extent, |maximum| maximum.max(extent)));
                }
            }
            if let Some(maximum) = maximum {
                for &(_, _, item_index) in group {
                    let item = &mut items[item_index];
                    if let Some(extent) = extent(item, set) {
                        item.baseline_shim = (maximum - extent).max(0.0);
                    }
                }
            }
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

#[derive(Default)]
struct DistributionScratch {
    planned: Vec<f32>,
    touched: Vec<usize>,
    affected: Vec<DistributionEntry>,
    non_affected: Vec<DistributionEntry>,
}

/// Reusable buffers for one recursive Grid layout frame.
#[derive(Default)]
pub(super) struct IntrinsicSizingScratch {
    single_growth_limits: Vec<Option<f32>>,
    non_flexible: Vec<usize>,
    crosses_flexible: Vec<usize>,
    distribution: DistributionScratch,
    track_distribution: Vec<(usize, f32)>,
}

impl IntrinsicSizingScratch {
    fn reset(&mut self) {
        self.single_growth_limits.clear();
        self.non_flexible.clear();
        self.crosses_flexible.clear();
        self.distribution.planned.clear();
        self.distribution.touched.clear();
        self.distribution.affected.clear();
        self.distribution.non_affected.clear();
        self.track_distribution.clear();
    }
}

fn distribute_up_to_limits(entries: &mut [DistributionEntry], remaining: &mut f32) {
    if entries.is_empty() || *remaining <= 0.0 {
        return;
    }
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
                    track.max.is_intrinsic()
                }
                (PlannedSize::Base, ContributionKind::MaxContent) => {
                    matches!(track.max, MaxKind::MaxContent | MaxKind::Auto)
                }
                (PlannedSize::GrowthLimit, _) => {
                    track.max.is_intrinsic() && track.fit_content_limit.is_infinite()
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
fn run_spanning_base_phase<T, I, P>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    items: &mut I,
    item_indices: &[usize],
    kind: ContributionKind,
    limited: bool,
    weighted_flex: bool,
    eligible: P,
    scratch: &mut DistributionScratch,
) where
    T: LayoutTree,
    I: SizingItems<T::NodeId> + ?Sized,
    P: Fn(&Track) -> bool + Copy,
{
    for &item_index in item_indices {
        let span = items.entry_span(item_index, axis);
        let range = tracks.span_indices(span.start, span.end);
        if !tracks.tracks[range.clone()]
            .iter()
            .any(|track| !track.collapsed && eligible(track))
        {
            continue;
        }
        let contribution = if limited {
            items.limited_contribution(tree, state, item_index, axis, kind, tracks, cross_tracks)
        } else {
            items.contribution(tree, state, item_index, axis, kind, tracks, cross_tracks)
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
            &mut scratch.planned,
            &mut scratch.touched,
            &mut scratch.affected,
            &mut scratch.non_affected,
        );
    }
    apply_planned_base(tracks, &mut scratch.planned, &mut scratch.touched);
}

#[allow(clippy::too_many_arguments)]
fn run_spanning_growth_phase<T, I, P>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    items: &mut I,
    item_indices: &[usize],
    kind: ContributionKind,
    eligible: P,
    scratch: &mut DistributionScratch,
) where
    T: LayoutTree,
    I: SizingItems<T::NodeId> + ?Sized,
    P: Fn(&Track) -> bool + Copy,
{
    for &item_index in item_indices {
        let span = items.entry_span(item_index, axis);
        let range = tracks.span_indices(span.start, span.end);
        if !tracks.tracks[range.clone()]
            .iter()
            .any(|track| !track.collapsed && eligible(track))
        {
            continue;
        }
        let contribution =
            items.contribution(tree, state, item_index, axis, kind, tracks, cross_tracks);
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
            &mut scratch.planned,
            &mut scratch.touched,
            &mut scratch.affected,
            &mut scratch.non_affected,
        );
    }
    apply_planned_growth(tracks, &mut scratch.planned, &mut scratch.touched);
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn resolve_intrinsic_sizes<T, I>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    items: &mut I,
    available: AvailableSpace,
    scratch: &mut IntrinsicSizingScratch,
) where
    T: LayoutTree,
    I: SizingItems<T::NodeId> + ?Sized,
{
    let IntrinsicSizingScratch {
        single_growth_limits,
        non_flexible,
        crosses_flexible,
        distribution,
        ..
    } = scratch;
    for item in 0..items.entry_count() {
        if items.entry_span_len(item, axis) != 1 {
            continue;
        }
        let span = items.entry_span(item, axis);
        let index = tracks.index_of(span.start);
        let track = &tracks.tracks[index];
        if track.collapsed || track.is_flexible() {
            continue;
        }
        let max_kind = match track.max {
            MaxKind::Fixed(_) | MaxKind::Flex => None,
            MaxKind::MinContent => Some(ContributionKind::MinContent),
            MaxKind::MaxContent | MaxKind::Auto => Some(ContributionKind::MaxContent),
        };
        let base = match track.min {
            MinKind::Fixed => track.base,
            MinKind::MinContent => items.contribution(
                tree,
                state,
                item,
                axis,
                ContributionKind::MinContent,
                tracks,
                cross_tracks,
            ),
            MinKind::MaxContent => items.contribution(
                tree,
                state,
                item,
                axis,
                ContributionKind::MaxContent,
                tracks,
                cross_tracks,
            ),
            MinKind::Auto if available == AvailableSpace::MinContent => items.limited_contribution(
                tree,
                state,
                item,
                axis,
                ContributionKind::MinContent,
                tracks,
                cross_tracks,
            ),
            MinKind::Auto if available == AvailableSpace::MaxContent => items.limited_contribution(
                tree,
                state,
                item,
                axis,
                ContributionKind::MaxContent,
                tracks,
                cross_tracks,
            ),
            MinKind::Auto => items.contribution(
                tree,
                state,
                item,
                axis,
                ContributionKind::Minimum,
                tracks,
                cross_tracks,
            ),
        };
        tracks.tracks[index].base = tracks.tracks[index].base.max(base);

        if let Some(kind) = max_kind {
            let contribution =
                items.contribution(tree, state, item, axis, kind, tracks, cross_tracks);
            let limit = contribution
                .max(tracks.tracks[index].base)
                .min(tracks.tracks[index].fit_content_limit);
            if single_growth_limits.is_empty() {
                single_growth_limits.resize(tracks.tracks.len(), None);
            }
            single_growth_limits[index] =
                Some(single_growth_limits[index].map_or(limit, |current| current.max(limit)));
        }
    }
    if !single_growth_limits.is_empty() {
        for (index, contribution) in single_growth_limits.iter().copied().enumerate() {
            if let Some(contribution) = contribution {
                tracks.tracks[index].growth_limit = contribution
                    .min(tracks.tracks[index].fit_content_limit)
                    .max(tracks.tracks[index].base);
            }
        }
    }
    for track in &mut tracks.tracks {
        track.growth_limit = track.growth_limit.max(track.base);
    }

    for index in 0..items.entry_count() {
        let span = items.entry_span(index, axis);
        let range = tracks.span_indices(span.start, span.end);
        if tracks.tracks[range.clone()].iter().any(Track::is_flexible) {
            crosses_flexible.push(index);
        } else if items.entry_span_len(index, axis) > 1
            && tracks.tracks[range]
                .iter()
                .any(|track| track.min.is_intrinsic() || track.max.is_intrinsic())
        {
            non_flexible.push(index);
        }
    }
    non_flexible.sort_unstable_by_key(|&index| items.entry_span_len(index, axis));
    if non_flexible.is_empty() && crosses_flexible.is_empty() {
        finalize_growth_limits(&mut tracks.tracks);
        return;
    }
    distribution.planned.resize(tracks.tracks.len(), 0.0);
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
        let span = items.entry_span_len(non_flexible[start], axis);
        let mut end = start + 1;
        while end < non_flexible.len() && items.entry_span_len(non_flexible[end], axis) == span {
            end += 1;
        }
        let group = &non_flexible[start..end];
        run_spanning_base_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            spanning_minimum_kind,
            use_limited_min_content,
            false,
            |track| track.min.is_intrinsic(),
            distribution,
        );
        run_spanning_base_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            ContributionKind::MinContent,
            false,
            false,
            |track| matches!(track.min, MinKind::MinContent | MinKind::MaxContent),
            distribution,
        );
        if available == AvailableSpace::MaxContent {
            run_spanning_base_phase(
                tree,
                state,
                axis,
                tracks,
                cross_tracks,
                items,
                group,
                ContributionKind::MaxContent,
                true,
                false,
                |track| matches!(track.min, MinKind::Auto | MinKind::MaxContent),
                distribution,
            );
        }
        run_spanning_base_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            ContributionKind::MaxContent,
            false,
            false,
            |track| track.min == MinKind::MaxContent,
            distribution,
        );
        run_spanning_growth_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            ContributionKind::MinContent,
            |track| track.max.is_intrinsic(),
            distribution,
        );
        run_spanning_growth_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            group,
            ContributionKind::MaxContent,
            |track| matches!(track.max, MaxKind::MaxContent | MaxKind::Auto),
            distribution,
        );
        for track in &mut tracks.tracks {
            track.infinitely_growable = false;
        }
        start = end;
    }

    if !crosses_flexible.is_empty() {
        let spanned_flex_sum = items.spanned_flex_sum(crosses_flexible, tracks, axis);
        let flexible_base_kind =
            if available != AvailableSpace::MinContent && spanned_flex_sum < 1.0 {
                ContributionKind::MaxContent
            } else {
                spanning_minimum_kind
            };
        run_spanning_base_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            crosses_flexible,
            flexible_base_kind,
            use_limited_min_content,
            true,
            Track::is_flexible,
            distribution,
        );
        run_spanning_base_phase(
            tree,
            state,
            axis,
            tracks,
            cross_tracks,
            items,
            crosses_flexible,
            ContributionKind::MinContent,
            false,
            true,
            |track| {
                track.is_flexible()
                    && matches!(track.min, MinKind::MinContent | MinKind::MaxContent)
            },
            distribution,
        );
    }

    finalize_growth_limits(&mut tracks.tracks);
}

fn finalize_growth_limits(tracks: &mut [Track]) {
    for track in tracks {
        if !track.growth_limit.is_finite() {
            track.growth_limit = track
                .base
                .max(0.0)
                .min(track.fit_content_limit)
                .max(track.base);
        }
    }
}

fn maximize_tracks(
    tracks: &mut TrackSet,
    available: AvailableSpace,
    scratch: &mut Vec<(usize, f32)>,
) {
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
    scratch.clear();
    scratch.extend(
        tracks
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| !track.collapsed && !track.is_flexible())
            .map(|(index, track)| (index, (track.growth_limit - track.base).max(0.0)))
            .filter(|&(_, capacity)| capacity > 0.0),
    );
    scratch.sort_unstable_by(|left, right| left.1.total_cmp(&right.1));
    let mut cursor = 0;
    while cursor < scratch.len() {
        let count = scratch.len() - cursor;
        let share = remaining / count as f32;
        let capacity = scratch[cursor].1;
        if capacity <= share {
            tracks.tracks[scratch[cursor].0].base += capacity;
            remaining -= capacity;
            cursor += 1;
        } else {
            for &(index, _) in &scratch[cursor..] {
                tracks.tracks[index].base += share;
            }
            return;
        }
    }
}

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
fn expand_flexible_tracks<T, I>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    items: &mut I,
    available: AvailableSpace,
    scratch: &mut Vec<(usize, f32)>,
) where
    T: LayoutTree,
    I: SizingItems<T::NodeId> + ?Sized,
{
    if !tracks.tracks.iter().any(Track::is_flexible) {
        return;
    }

    if available == AvailableSpace::MinContent {
        return;
    }

    let definite_space = match available {
        AvailableSpace::Definite(space) => Some(space),
        _ => None,
    };
    let mut flex_fraction = 0.0_f32;
    if let Some(space) = definite_space {
        flex_fraction = find_fr_size(tracks, 0..tracks.tracks.len(), space, scratch);
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
        for item in 0..items.entry_count() {
            let span = items.entry_span(item, axis);
            let range = tracks.span_indices(span.start, span.end);
            if !tracks.tracks[range.clone()].iter().any(Track::is_flexible) {
                continue;
            }
            let contribution = items.contribution(
                tree,
                state,
                item,
                axis,
                ContributionKind::MaxContent,
                tracks,
                cross_tracks,
            );
            flex_fraction = flex_fraction.max(find_fr_size(tracks, range, contribution, scratch));
        }
    }
    for track in &mut tracks.tracks {
        if track.is_flexible() {
            track.base = track.base.max(flex_fraction * track.flex_factor);
        }
    }
}

fn stretch_auto_tracks(tracks: &mut TrackSet, available: AvailableSpace, alignment: AlignFlags) {
    if alignment != AlignFlags::STRETCH {
        return;
    }
    let AvailableSpace::Definite(space) = available else {
        return;
    };
    let free = (space - tracks.used_size()).max(0.0);
    let count = tracks
        .tracks
        .iter()
        .filter(|track| track.max == MaxKind::Auto && !track.collapsed)
        .count();
    if count == 0 || free <= 0.0 {
        return;
    }
    let share = free / count as f32;
    for track in &mut tracks.tracks {
        if track.max == MaxKind::Auto && !track.collapsed {
            track.base += share;
            track.growth_limit = track.growth_limit.max(track.base);
        }
    }
}

/// Whether track sizing consults the items placed in this set at all: some
/// track's used size depends on the contributions it collects, rather than
/// standing on its own sizing function. A caller that has to assemble the item
/// list before calling [`size_tracks`] can skip assembling it when this is
/// false — the same condition `size_tracks` itself fast-paths on, named once so
/// the two cannot drift.
///
/// A flexible track counts: §12.5's "increase sizes to accommodate spanning
/// items crossing flexible tracks" step runs under every available space, not
/// only under a max-content constraint.
pub(super) fn track_sizing_reads_items(tracks: &TrackSet) -> bool {
    tracks
        .tracks
        .iter()
        .any(|track| track.min.is_intrinsic() || track.max.is_intrinsic() || track.is_flexible())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn size_tracks<T, I>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: Option<CrossAxisTracks<'_>>,
    items: &mut I,
    inner_size: crate::geometry::Size<Option<f32>>,
    available: AvailableSpace,
    alignment: AlignFlags,
    scratch: &mut IntrinsicSizingScratch,
) where
    T: LayoutTree,
    I: SizingItems<T::NodeId> + ?Sized,
{
    scratch.reset();
    if tracks.tracks.is_empty() {
        return;
    }
    if !track_sizing_reads_items(tracks) {
        maximize_tracks(tracks, available, &mut scratch.track_distribution);
        tracks.rebuild_positions();
        return;
    }
    items.prepare(tree, state, axis, tracks, cross_tracks, inner_size);
    resolve_intrinsic_sizes(
        tree,
        state,
        axis,
        tracks,
        cross_tracks,
        items,
        available,
        scratch,
    );
    maximize_tracks(tracks, available, &mut scratch.track_distribution);
    expand_flexible_tracks(
        tree,
        state,
        axis,
        tracks,
        cross_tracks,
        items,
        available,
        &mut scratch.track_distribution,
    );
    stretch_auto_tracks(tracks, available, alignment);
    tracks.rebuild_positions();
}

/// What one §12 track-sizing run reads from the items it sizes the tracks
/// for, addressed by entry index: Grid's own items (a `[GridItem]`), or the
/// entries grid-lanes sizes its grid axis from ([`LaneSizingItems`]), where
/// one entry stands for every span-1 item that may sit in one track.
pub(super) trait SizingItems<N> {
    fn entry_count(&self) -> usize;

    fn entry_span(&self, index: usize, axis: Axis) -> TrackSpan;

    /// The number of tracks the entry spans, at least 1.
    fn entry_span_len(&self, index: usize, axis: Axis) -> usize;

    /// Resolves intrinsic-keyword sizes and a row's baseline shims before
    /// any contribution is read.
    fn prepare<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        axis: Axis,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
        inner_size: crate::geometry::Size<Option<f32>>,
    ) where
        T: LayoutTree<NodeId = N>;

    #[allow(clippy::too_many_arguments)]
    fn contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>;

    #[allow(clippy::too_many_arguments)]
    fn limited_contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>;

    /// The flex factors of the flexible tracks each of `crosses_flexible`'s
    /// entries spans, summed in entry order.
    fn spanned_flex_sum(&self, crosses_flexible: &[usize], tracks: &TrackSet, axis: Axis) -> f32;
}

impl<N> SizingItems<N> for [GridItem<N>] {
    #[inline]
    fn entry_count(&self) -> usize {
        self.len()
    }

    #[inline]
    fn entry_span(&self, index: usize, axis: Axis) -> TrackSpan {
        span_for(&self[index], axis)
    }

    #[inline]
    fn entry_span_len(&self, index: usize, axis: Axis) -> usize {
        self[index].span(axis)
    }

    fn prepare<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        axis: Axis,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
        inner_size: crate::geometry::Size<Option<f32>>,
    ) where
        T: LayoutTree<NodeId = N>,
    {
        for item in self.iter_mut() {
            resolve_item_intrinsic_dimensions(tree, state, item, axis, cross_tracks, inner_size);
        }
        prepare_baseline_shims(tree, state, axis, tracks, cross_tracks, self);
    }

    #[inline]
    fn contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>,
    {
        measure_contribution(
            tree,
            state,
            &mut self[index],
            axis,
            kind,
            tracks,
            cross_tracks,
        )
    }

    #[inline]
    fn limited_contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>,
    {
        measure_limited_contribution(
            tree,
            state,
            &mut self[index],
            axis,
            kind,
            tracks,
            cross_tracks,
        )
    }

    fn spanned_flex_sum(&self, crosses_flexible: &[usize], tracks: &TrackSet, axis: Axis) -> f32 {
        crosses_flexible
            .iter()
            .flat_map(|&index| {
                let span = span_for(&self[index], axis);
                tracks.tracks[tracks.span_indices(span.start, span.end)]
                    .iter()
                    .filter(|track| track.is_flexible())
                    .map(|track| track.flex_factor)
            })
            .sum::<f32>()
    }
}

/// A maximum over a group of items, computed when first asked for: `None`
/// for a group with no member it covers.
#[derive(Debug, Clone, Copy, Default)]
enum LazyMaximum {
    #[default]
    Pending,
    Known(Option<f32>),
}

impl LazyMaximum {
    #[inline]
    fn get_or_compute(&mut self, compute: impl FnOnce() -> Option<f32>) -> Option<f32> {
        match *self {
            Self::Known(maximum) => maximum,
            Self::Pending => {
                let maximum = compute();
                *self = Self::Known(maximum);
                maximum
            }
        }
    }
}

#[inline]
fn max_of(values: impl Iterator<Item = f32>) -> Option<f32> {
    values.fold(None, |maximum, value| {
        Some(maximum.map_or(value, |maximum: f32| maximum.max(value)))
    })
}

#[inline]
fn max_option(left: Option<f32>, right: Option<f32>) -> Option<f32> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (value, None) | (None, value) => value,
    }
}

/// The parts of a group of span-1 items' contributions that do not depend on
/// which track the group sits in, each the maximum over the members it
/// covers. Every track-sizing step that reads a span-1 contribution takes
/// the maximum of the items' contributions to the track (§12.5 steps 2 and
/// 4 through `record_planned`, a growth limit, an fr size), and every step
/// from a member's parts to its contribution is monotone, so the maximum of
/// the members' contributions is the contribution of their maxima.
///
/// A minimum contribution depends on the track only through whether the
/// automatic minimum applies there and the fixed maximum it is clamped by,
/// so it is kept in three parts: `fixed_min` for the members it never
/// applies to (a definite minimum size, a non-`auto` one, a scroll
/// container), and for the rest `auto_min_candidate` — the content-based
/// minimum already capped by the member's own maximum size, the clamp the
/// track's fixed maximum then joins — and `auto_min_fallback`, their
/// contribution where it does not apply.
/// Whether a track gives a span-1 item its automatic minimum (§6.6), and the
/// track's fixed maximum (`fixed_max_span_limit`) clamping it when it does.
#[derive(Debug, Clone, Copy)]
enum AutomaticMinimum {
    Applies { limit: Option<f32> },
    DoesNotApply,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct GroupMaxima {
    min_content: LazyMaximum,
    max_content: LazyMaximum,
    fixed_min: LazyMaximum,
    auto_min_candidate: LazyMaximum,
    auto_min_fallback: LazyMaximum,
}

impl GroupMaxima {
    #[allow(clippy::too_many_arguments)]
    fn content<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        items: &mut [GridItem<T::NodeId>],
        members: &[usize],
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
    ) -> Option<f32>
    where
        T: LayoutTree,
    {
        let slot = match kind {
            ContributionKind::MinContent => &mut self.min_content,
            ContributionKind::MaxContent => &mut self.max_content,
            ContributionKind::Minimum => unreachable!("a minimum contribution reads its parts"),
        };
        slot.get_or_compute(|| {
            max_of(members.iter().map(|&member| {
                measure_contribution(tree, state, &mut items[member], axis, kind, tracks, None)
            }))
        })
    }

    /// The group's minimum contribution in a span-1 track: whether the
    /// automatic minimum applies there, and the fixed maximum it is then
    /// clamped by.
    fn minimum<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        items: &mut [GridItem<T::NodeId>],
        members: &[usize],
        axis: Axis,
        automatic: AutomaticMinimum,
    ) -> Option<f32>
    where
        T: LayoutTree,
    {
        let fixed = self.fixed_min.get_or_compute(|| {
            max_of(members.iter().filter_map(|&member| {
                let item = &mut items[member];
                if minimum_may_be_automatic(item, axis) {
                    return None;
                }
                let margin = margin_sum(item, axis);
                let explicit_min = axis.size(item.min_size).map(|value| value + margin);
                let explicit_max = axis.size(item.max_size).map(|value| value + margin);
                let raw = match explicit_min {
                    Some(minimum) => minimum,
                    None => minimum_fallback(tree, state, item, axis, None),
                };
                Some(finish_contribution(
                    raw,
                    explicit_min,
                    explicit_max,
                    axis,
                    item.baseline_shim,
                ))
            }))
        });
        let automatic_part = match automatic {
            AutomaticMinimum::Applies { limit } => {
                let candidate = self.auto_min_candidate.get_or_compute(|| {
                    max_of(members.iter().filter_map(|&member| {
                        let item = &mut items[member];
                        if !minimum_may_be_automatic(item, axis) {
                            return None;
                        }
                        // The track's fixed maximum joins the member's own
                        // maximum below, a clamp the shim would follow.
                        debug_assert!(item.baseline_shim == 0.0);
                        let explicit_max = axis
                            .size(item.max_size)
                            .map(|value| value + margin_sum(item, axis));
                        Some(
                            automatic_minimum_suggestion(tree, state, item, axis, None)
                                .min(explicit_max.unwrap_or(f32::INFINITY)),
                        )
                    }))
                });
                candidate.map(|candidate| {
                    let raw = limit.map_or(candidate, |limit| candidate.min(limit));
                    finish_contribution(raw, None, None, axis, 0.0)
                })
            }
            AutomaticMinimum::DoesNotApply => self.auto_min_fallback.get_or_compute(|| {
                max_of(members.iter().filter_map(|&member| {
                    let item = &mut items[member];
                    if !minimum_may_be_automatic(item, axis) {
                        return None;
                    }
                    let explicit_max = axis
                        .size(item.max_size)
                        .map(|value| value + margin_sum(item, axis));
                    let raw = minimum_fallback(tree, state, item, axis, None);
                    Some(finish_contribution(
                        raw,
                        None,
                        explicit_max,
                        axis,
                        item.baseline_shim,
                    ))
                }))
            }),
        };
        max_option(fixed, automatic_part)
    }
}

/// One grid-lanes grid-axis sizing entry (css-grid-3 §3.4).
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "the clones sit inline as Grid's own item list holds them; the aggregates number at most one per track"
)]
pub(super) enum LaneEntry<N> {
    /// An item spanning several tracks at one candidate position: a clone
    /// at that position, sized as Grid sizes an item.
    Item(GridItem<N>),
    /// Every span-1 item that may sit in one track: the definite-position
    /// items placed there (`members`, a range of `LaneSizingItems::members`)
    /// and, when `pooled`, every auto-placed one.
    Aggregate {
        span: TrackSpan,
        members: core::ops::Range<usize>,
        pooled: bool,
        maxima: GroupMaxima,
    },
}

/// Grid-lanes' grid-axis sizing entries: §3.4 has a definite-position item
/// contribute at its own span and an auto-placed item at every start
/// position its span fits. A spanning item is cloned per position
/// ([`LaneEntry::Item`]); the span-1 items, measured once each in place in
/// `items`, contribute through one aggregate per track
/// ([`LaneEntry::Aggregate`]), whose contributions are the maxima of its
/// members' ([`GroupMaxima`]) — the same numbers the per-position clones
/// give the tracks, which a debug build checks after every sizing run.
#[derive(Debug)]
pub(super) struct LaneSizingItems<'a, N> {
    /// Every in-flow item, in item order.
    pub(super) items: &'a mut [GridItem<N>],
    /// The `items` indices of the auto-placed span-1 items.
    pub(super) pooled: Vec<usize>,
    pub(super) pool: GroupMaxima,
    /// The `items` indices of the definite-position span-1 items, grouped
    /// by track; each aggregate names its range.
    pub(super) members: Vec<usize>,
    pub(super) entries: Vec<LaneEntry<N>>,
    /// The per-position clones' flex sum (`SizingItems::spanned_flex_sum`),
    /// summed in their order.
    pub(super) spanned_flex_sum: f32,
}

impl<'a, N> LaneSizingItems<'a, N> {
    /// No entry yet over `items`.
    pub(super) fn new(items: &'a mut [GridItem<N>]) -> Self {
        Self {
            items,
            pooled: Vec::new(),
            pool: GroupMaxima::default(),
            members: Vec::new(),
            entries: Vec::new(),
            spanned_flex_sum: 0.0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn aggregate_contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>,
    {
        let Self {
            items,
            pooled,
            pool,
            members,
            entries,
            ..
        } = self;
        let LaneEntry::Aggregate {
            span,
            members: range,
            pooled: includes_pool,
            maxima,
        } = &mut entries[index]
        else {
            unreachable!("only an aggregate entry reads group maxima")
        };
        let own = &members[range.clone()];
        let pool_members = if *includes_pool { &pooled[..] } else { &[] };
        match kind {
            ContributionKind::MinContent | ContributionKind::MaxContent => max_option(
                pool.content(tree, state, items, pool_members, axis, kind, tracks),
                maxima.content(tree, state, items, own, axis, kind, tracks),
            ),
            ContributionKind::Minimum => {
                let track = tracks.index_of(span.start);
                let spanned = track..track + 1;
                let automatic = if automatic_minimum_applies(tracks, spanned.clone(), 1) {
                    AutomaticMinimum::Applies {
                        limit: fixed_max_span_limit(tracks, spanned),
                    }
                } else {
                    AutomaticMinimum::DoesNotApply
                };
                max_option(
                    pool.minimum(tree, state, items, pool_members, axis, automatic),
                    maxima.minimum(tree, state, items, own, axis, automatic),
                )
            }
        }
        .expect("an aggregate has at least one member")
    }
}

impl<N> SizingItems<N> for LaneSizingItems<'_, N> {
    #[inline]
    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    fn entry_span(&self, index: usize, axis: Axis) -> TrackSpan {
        match &self.entries[index] {
            LaneEntry::Item(item) => span_for(item, axis),
            LaneEntry::Aggregate { span, .. } => *span,
        }
    }

    #[inline]
    fn entry_span_len(&self, index: usize, axis: Axis) -> usize {
        match &self.entries[index] {
            LaneEntry::Item(item) => item.span(axis),
            LaneEntry::Aggregate { .. } => 1,
        }
    }

    fn prepare<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        axis: Axis,
        _tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
        inner_size: crate::geometry::Size<Option<f32>>,
    ) where
        T: LayoutTree<NodeId = N>,
    {
        let mut prepare = |item: &mut GridItem<N>| {
            resolve_item_intrinsic_dimensions(tree, state, item, axis, cross_tracks, inner_size);
            // Grid lanes takes the fallback alignment for every baseline
            // value, so no item joins a baseline-sharing group and every
            // shim is zero, as `prepare_baseline_shims` would leave it.
            debug_assert!(item.baseline_participation().is_none());
            if axis == Axis::Vertical {
                item.baseline_shim = 0.0;
            }
        };
        // Every span-1 item is pooled or a member, once.
        for &index in self.pooled.iter().chain(&self.members) {
            prepare(&mut self.items[index]);
        }
        for entry in &mut self.entries {
            if let LaneEntry::Item(item) = entry {
                prepare(item);
            }
        }
    }

    fn contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>,
    {
        if let LaneEntry::Item(item) = &mut self.entries[index] {
            return measure_contribution(tree, state, item, axis, kind, tracks, cross_tracks);
        }
        self.aggregate_contribution(tree, state, index, axis, kind, tracks)
    }

    fn limited_contribution<T>(
        &mut self,
        tree: &T,
        state: &mut T::State,
        index: usize,
        axis: Axis,
        kind: ContributionKind,
        tracks: &TrackSet,
        cross_tracks: Option<CrossAxisTracks<'_>>,
    ) -> f32
    where
        T: LayoutTree<NodeId = N>,
    {
        if let LaneEntry::Item(item) = &mut self.entries[index] {
            return measure_limited_contribution(
                tree,
                state,
                item,
                axis,
                kind,
                tracks,
                cross_tracks,
            );
        }
        // `measure_limited_contribution` for one track: each member's
        // `min(contribution, limit).max(minimum)` is monotone in both, so
        // the maximum over the members is the same expression of the group
        // maxima.
        debug_assert!(matches!(
            kind,
            ContributionKind::MinContent | ContributionKind::MaxContent
        ));
        let contribution = self.aggregate_contribution(tree, state, index, axis, kind, tracks);
        let span = self.entry_span(index, axis);
        let range = tracks.span_indices(span.start, span.end);
        let fixed_limit = fixed_max_span_limit(tracks, range.clone()).or_else(|| {
            (range.len() == 1)
                .then(|| tracks.tracks[range.start].fit_content_limit)
                .filter(|limit| limit.is_finite())
        });
        let Some(limit) = fixed_limit else {
            return contribution;
        };
        let minimum = self.aggregate_contribution(
            tree,
            state,
            index,
            axis,
            ContributionKind::Minimum,
            tracks,
        );
        contribution.min(limit).max(minimum)
    }

    #[inline]
    fn spanned_flex_sum(
        &self,
        _crosses_flexible: &[usize],
        _tracks: &TrackSet,
        _axis: Axis,
    ) -> f32 {
        self.spanned_flex_sum
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use core::cell::RefCell;
    use core::marker::PhantomData;

    use stylo::computed_values::direction;
    use stylo::values::computed::{
        Display, GridTemplateComponent, ImplicitGridTracks, Length, LengthPercentage,
        MaxSize as StyleMaxSize, Overflow, PositionProperty, Size as StyleSize,
    };
    use stylo::values::generics::NonNegative;
    use stylo::values::generics::grid::ImplicitGridTracks as GenericImplicitGridTracks;

    use super::*;
    use crate::compute::grid::placement::{GridArea, TrackSpan};
    use crate::geometry::Size;
    use crate::style::{CoreStyle, GridStyle};
    use crate::tree::{LayoutInput, LayoutOutput, LayoutSlot, LayoutTree};

    fn lp_px(value: f32) -> LengthPercentage {
        LengthPercentage::new_length(Length::new(value))
    }

    #[derive(Debug, Clone)]
    struct TestStyle {
        size: Size<StyleSize>,
        min_size: Size<StyleSize>,
        max_size: Size<StyleMaxSize>,
        template: GridTemplateComponent,
        auto_tracks: ImplicitGridTracks,
    }

    impl Default for TestStyle {
        fn default() -> Self {
            Self {
                size: Size::new(StyleSize::Auto, StyleSize::Auto),
                min_size: Size::new(StyleSize::Auto, StyleSize::Auto),
                max_size: Size::new(StyleMaxSize::None, StyleMaxSize::None),
                template: GridTemplateComponent::None,
                auto_tracks: GenericImplicitGridTracks(Vec::new().into()),
            }
        }
    }

    impl CoreStyle for TestStyle {
        fn display(&self) -> Display {
            Display::Grid
        }

        fn size(&self) -> Size<&StyleSize> {
            self.size.as_ref()
        }

        fn min_size(&self) -> Size<&StyleSize> {
            self.min_size.as_ref()
        }

        fn max_size(&self) -> Size<&StyleMaxSize> {
            self.max_size.as_ref()
        }
    }

    impl GridStyle for TestStyle {
        fn grid_template_rows(&self) -> &GridTemplateComponent {
            &self.template
        }

        fn grid_template_columns(&self) -> &GridTemplateComponent {
            &self.template
        }

        fn grid_auto_rows(&self) -> &ImplicitGridTracks {
            &self.auto_tracks
        }

        fn grid_auto_columns(&self) -> &ImplicitGridTracks {
            &self.auto_tracks
        }
    }

    /// Minimal tree/state test fixture with no children, canned intrinsic
    /// measurements, and an interior-mutable call log.
    #[derive(Debug)]
    struct TestTree {
        style: TestStyle,
        min_content: Size<f32>,
        max_content: Size<f32>,
        first_baseline: Option<f32>,
        calls: RefCell<Vec<LayoutInput>>,
    }

    impl Default for TestTree {
        fn default() -> Self {
            Self {
                style: TestStyle::default(),
                min_content: Size::new(20.0, 10.0),
                max_content: Size::new(80.0, 40.0),
                first_baseline: None,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct TestRef<'t> {
        tree: &'t TestTree,
    }

    #[derive(Debug, Default)]
    struct TestHost<'t>(PhantomData<&'t TestTree>);

    impl<'t> LayoutTree for TestHost<'t> {
        type NodeId = TestRef<'t>;
        type State = LayoutSlot;
        type Style<'tree>
            = &'t TestStyle
        where
            Self: 'tree;
        type ChildIter<'tree>
            = core::iter::Empty<Self::NodeId>
        where
            Self: 'tree;

        fn children(&self, _node: Self::NodeId) -> Self::ChildIter<'_> {
            core::iter::empty()
        }

        fn style(&self, node: Self::NodeId) -> Self::Style<'_> {
            &node.tree.style
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            _node: Self::NodeId,
        ) -> &'state LayoutSlot {
            state
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            _node: Self::NodeId,
        ) -> &'state mut LayoutSlot {
            state
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            node: Self::NodeId,
            input: LayoutInput,
        ) -> LayoutOutput {
            node.tree.calls.borrow_mut().push(input);
            let measured = Size::new(
                match input.available_space.width {
                    AvailableSpace::MinContent => node.tree.min_content.width,
                    AvailableSpace::MaxContent => node.tree.max_content.width,
                    AvailableSpace::Definite(value) => value,
                },
                match input.available_space.height {
                    AvailableSpace::MinContent => node.tree.min_content.height,
                    AvailableSpace::MaxContent => node.tree.max_content.height,
                    AvailableSpace::Definite(value) => value,
                },
            );
            let size = Size::new(
                input.known_dimensions.width.unwrap_or(measured.width),
                input.known_dimensions.height.unwrap_or(measured.height),
            );
            LayoutOutput::new(size, size).with_first_baseline(node.tree.first_baseline)
        }
    }

    fn test_item(node: TestRef<'_>, column_start: i32, column_end: i32) -> GridItem<TestRef<'_>> {
        let style = &node.tree.style;
        GridItem {
            geometry: crate::compute::util::resolve_item_geometry(style, Size::NONE),
            key: crate::compute::util::ItemKey {
                node,
                layout_order: 0,
            },
            area: GridArea {
                column: TrackSpan {
                    start: column_start,
                    end: column_end,
                },
                row: TrackSpan { start: 0, end: 1 },
            },
            position: PositionProperty::Relative,
            align_self: AlignFlags::START,
            justify_self: AlignFlags::START,
            direction: direction::T::Ltr,
            preferred_behaves_auto_or_depends: Size::new(true, true),
            minimum_is_auto: Size::new(true, true),
            raw_min_content: Size::NONE,
            raw_max_content: Size::NONE,
            minimum_contribution: Size::NONE,
            min_content_contribution: Size::NONE,
            max_content_contribution: Size::NONE,
            measured_baseline: None,
            measured_last_descent: None,
            baseline_shim: 0.0,
        }
    }

    fn test_track(base: f32, growth_limit: f32) -> Track {
        Track {
            min: MinKind::Auto,
            max: MaxKind::Auto,
            base,
            growth_limit,
            fit_content_limit: f32::INFINITY,
            flex_factor: 0.0,
            infinitely_growable: false,
            collapsed: false,
            position: 0.0,
        }
    }

    /// A track sized by its own function alone: `growth_limit` as both its
    /// fixed minimum and maximum.
    fn fixed_track(base: f32, growth_limit: f32) -> Track {
        Track {
            min: MinKind::Fixed,
            max: MaxKind::Fixed(growth_limit),
            ..test_track(base, growth_limit)
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
    fn initial_column_cross_area_uses_only_definite_row_maximums() {
        let tree = TestTree::default();
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        item.area.row = TrackSpan { start: 0, end: 2 };
        item.margin.top = 2.0;
        item.margin.bottom = 3.0;
        let mut rows = track_set(vec![test_track(4.0, 10.0), test_track(6.0, 20.0)]);
        rows.gap = 5.0;

        let estimated = CrossAxisTracks::DefiniteMaximums {
            tracks: &rows,
            distributed_gap: 3.0,
        };
        assert_eq!(
            cross_area_size(&item, Axis::Horizontal, Some(estimated)),
            Some(33.0)
        );

        rows.tracks[1].growth_limit = f32::INFINITY;
        let estimated = CrossAxisTracks::DefiniteMaximums {
            tracks: &rows,
            distributed_gap: 0.0,
        };
        assert_eq!(
            cross_area_size(&item, Axis::Horizontal, Some(estimated)),
            None
        );

        rows.rebuild_positions();
        assert_eq!(
            cross_area_size(
                &item,
                Axis::Horizontal,
                Some(CrossAxisTracks::resolved(&rows)),
            ),
            Some(10.0)
        );
    }

    #[test]
    fn intrinsic_keywords_resolve_each_raw_dimension_form() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tree = TestTree {
            style: TestStyle {
                size: Size::new(StyleSize::MinContent, StyleSize::Auto),
                min_size: Size::new(StyleSize::MaxContent, StyleSize::Auto),
                max_size: Size::new(
                    StyleMaxSize::FitContentFunction(NonNegative(lp_px(50.0))),
                    StyleMaxSize::None,
                ),
                ..TestStyle::default()
            },
            ..TestTree::default()
        };
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        resolve_item_intrinsic_dimensions(
            &host,
            &mut state,
            &mut item,
            Axis::Horizontal,
            None,
            Size::new(Some(100.0), None),
        );
        assert_eq!(item.preferred_size.width, Some(20.0));
        assert_eq!(item.min_size.width, Some(80.0));
        assert_eq!(item.max_size.width, Some(50.0));
        assert_eq!(tree.calls.borrow().len(), 2);

        let tree = TestTree {
            style: TestStyle {
                min_size: Size::new(StyleSize::MinContent, StyleSize::Auto),
                ..TestStyle::default()
            },
            ..TestTree::default()
        };
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        resolve_item_intrinsic_dimensions(
            &host,
            &mut state,
            &mut item,
            Axis::Horizontal,
            None,
            Size::NONE,
        );
        assert_eq!(item.preferred_size.width, None);
        assert_eq!(item.min_size.width, Some(20.0));
        assert_eq!(item.max_size.width, None);
        assert_eq!(tree.calls.borrow().len(), 1);

        let tree = TestTree {
            style: TestStyle {
                size: Size::new(StyleSize::MaxContent, StyleSize::Auto),
                ..TestStyle::default()
            },
            ..TestTree::default()
        };
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        resolve_item_intrinsic_dimensions(
            &host,
            &mut state,
            &mut item,
            Axis::Horizontal,
            None,
            Size::NONE,
        );
        assert_eq!(item.preferred_size.width, Some(80.0));
        assert_eq!(tree.calls.borrow().len(), 1);
    }

    #[test]
    fn keyword_sizes_behave_as_auto() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tree = TestTree {
            style: TestStyle {
                size: Size::new(StyleSize::Stretch, StyleSize::WebkitFillAvailable),
                min_size: Size::new(StyleSize::FitContent, StyleSize::Auto),
                ..TestStyle::default()
            },
            ..TestTree::default()
        };
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        resolve_item_intrinsic_dimensions(
            &host,
            &mut state,
            &mut item,
            Axis::Horizontal,
            None,
            Size::NONE,
        );
        assert_eq!(item.preferred_size.width, None);
        assert_eq!(item.min_size.width, None);
        assert!(tree.calls.borrow().is_empty());
    }

    #[test]
    fn vertical_border_box_ratio_and_synthesized_baseline_affect_raw_content() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tree = TestTree::default();
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        item.preferred_size.width = Some(40.0);
        item.aspect_ratio = Some(2.0);
        item.box_sizing = box_sizing::T::BorderBox;
        item.align_self = AlignFlags::BASELINE;

        let measured = raw_content_size(
            &host,
            &mut state,
            &mut item,
            Axis::Vertical,
            ContributionKind::MinContent,
            None,
        );
        assert_eq!(measured, 20.0);
        assert_eq!(item.measured_baseline, Some(10.0));
    }

    #[test]
    fn baseline_shims_skip_groups_that_cannot_affect_track_sizing() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let singleton_tree = TestTree {
            first_baseline: Some(4.0),
            ..TestTree::default()
        };
        let mut singleton = test_item(
            TestRef {
                tree: &singleton_tree,
            },
            0,
            1,
        );
        singleton.align_self = AlignFlags::BASELINE;
        let intrinsic_track = test_track(0.0, f32::INFINITY);
        prepare_baseline_shims(
            &host,
            &mut state,
            Axis::Vertical,
            &track_set(vec![intrinsic_track]),
            None,
            core::slice::from_mut(&mut singleton),
        );
        assert!(singleton_tree.calls.borrow().is_empty());
        assert_eq!(singleton.baseline_shim, 0.0);

        let first_tree = TestTree {
            first_baseline: Some(4.0),
            ..TestTree::default()
        };
        let second_tree = TestTree {
            first_baseline: Some(10.0),
            ..TestTree::default()
        };
        let mut fixed_items = vec![
            test_item(TestRef { tree: &first_tree }, 0, 1),
            test_item(TestRef { tree: &second_tree }, 0, 1),
        ];
        for item in &mut fixed_items {
            item.align_self = AlignFlags::BASELINE;
        }
        prepare_baseline_shims(
            &host,
            &mut state,
            Axis::Vertical,
            &track_set(vec![fixed_track(10.0, 10.0)]),
            None,
            &mut fixed_items,
        );
        assert!(first_tree.calls.borrow().is_empty());
        assert!(second_tree.calls.borrow().is_empty());
        assert!(fixed_items.iter().all(|item| item.baseline_shim == 0.0));
    }

    #[test]
    fn baseline_shims_probe_every_item_in_an_active_group() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tall_tree = TestTree {
            first_baseline: Some(12.0),
            ..TestTree::default()
        };
        let short_tree = TestTree {
            first_baseline: Some(4.0),
            ..TestTree::default()
        };
        let mut items = vec![
            test_item(TestRef { tree: &tall_tree }, 0, 1),
            test_item(TestRef { tree: &short_tree }, 0, 1),
        ];
        for item in &mut items {
            item.align_self = AlignFlags::BASELINE;
        }
        items[1].area.row.end = 2;
        let fixed_track = fixed_track(10.0, 10.0);
        let intrinsic_track = test_track(0.0, f32::INFINITY);

        prepare_baseline_shims(
            &host,
            &mut state,
            Axis::Vertical,
            &track_set(vec![fixed_track, intrinsic_track]),
            None,
            &mut items,
        );

        assert_eq!(tall_tree.calls.borrow().len(), 1);
        assert_eq!(short_tree.calls.borrow().len(), 1);
        assert_eq!(items[0].baseline_shim, 0.0);
        assert_eq!(items[1].baseline_shim, 8.0);
    }

    #[test]
    fn max_content_auto_track_skips_unconsumed_min_content_probe() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tree = TestTree::default();
        let mut items = vec![test_item(TestRef { tree: &tree }, 0, 1)];
        let mut tracks = track_set(vec![test_track(0.0, f32::INFINITY)]);

        size_tracks(
            &host,
            &mut state,
            Axis::Horizontal,
            &mut tracks,
            None,
            items.as_mut_slice(),
            Size::NONE,
            AvailableSpace::MaxContent,
            AlignFlags::START,
            &mut IntrinsicSizingScratch::default(),
        );

        let calls = tree.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].available_space.width, AvailableSpace::MaxContent);
        assert_eq!(items[0].raw_min_content.width, None);
        assert_eq!(tracks.tracks[0].base, 80.0);
    }

    #[test]
    fn non_auto_contribution_without_a_preferred_size_falls_back_to_content() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tree = TestTree::default();
        let mut item = test_item(TestRef { tree: &tree }, 0, 1);
        item.preferred_behaves_auto_or_depends.width = false;
        let tracks = track_set(vec![test_track(0.0, f32::INFINITY)]);

        let minimum = measure_contribution(
            &host,
            &mut state,
            &mut item,
            Axis::Horizontal,
            ContributionKind::Minimum,
            &tracks,
            None,
        );
        let maximum = measure_contribution(
            &host,
            &mut state,
            &mut item,
            Axis::Horizontal,
            ContributionKind::MaxContent,
            &tracks,
            None,
        );
        assert_eq!(minimum, 20.0);
        assert_eq!(maximum, 80.0);
        assert_eq!(tree.calls.borrow().len(), 2);
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
                test_track(0.0, 0.0),
            ),
            (
                PlannedSize::Base,
                ContributionKind::MaxContent,
                test_track(0.0, 0.0),
            ),
            (
                PlannedSize::GrowthLimit,
                ContributionKind::MaxContent,
                test_track(0.0, 0.0),
            ),
            (
                PlannedSize::Base,
                ContributionKind::MaxContent,
                fixed_track(0.0, 0.0),
            ),
        ];
        for (planned_size, kind, track) in cases {
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
        maximize_tracks(&mut tracks, AvailableSpace::Definite(10.0), &mut Vec::new());
        assert_eq!(tracks.tracks[0].base, 5.0);
        assert_eq!(tracks.tracks[1].base, 5.0);

        let mut finite_growth = track_set(vec![test_track(1.0, 5.0)]);
        let mut planned = vec![2.0];
        let mut touched = vec![0];
        apply_planned_growth(&mut finite_growth, &mut planned, &mut touched);
        assert_eq!(finite_growth.tracks[0].growth_limit, 7.0);
        assert!(!finite_growth.tracks[0].infinitely_growable);

        let mut zero_factor = test_track(10.0, f32::INFINITY);
        zero_factor.max = MaxKind::Flex;
        let zero_factor = track_set(vec![zero_factor]);
        let fraction = find_fr_size(&zero_factor, 0..1, 50.0, &mut Vec::new());
        assert_eq!(fraction, 40.0);

        let mut tracks = track_set(vec![test_track(10.0, 10.0)]);
        stretch_auto_tracks(
            &mut tracks,
            AvailableSpace::Definite(50.0),
            AlignFlags::STRETCH,
        );
        assert_eq!(tracks.tracks[0].base, 50.0);
        assert_eq!(tracks.tracks[0].growth_limit, 50.0);
    }

    #[test]
    fn indefinite_fr_sizing_considers_only_items_crossing_flexible_tracks() {
        let host = TestHost::default();
        let mut state = LayoutSlot::default();
        let tree = TestTree::default();
        let node = TestRef { tree: &tree };
        let fixed = fixed_track(10.0, 10.0);
        let mut flexible = test_track(10.0, f32::INFINITY);
        flexible.max = MaxKind::Flex;
        flexible.flex_factor = 2.0;
        let mut subunit_flexible = test_track(7.0, f32::INFINITY);
        subunit_flexible.max = MaxKind::Flex;
        subunit_flexible.flex_factor = 0.5;
        let mut tracks = track_set(vec![fixed, flexible, subunit_flexible]);
        let mut items = vec![test_item(node, 0, 1), test_item(node, 1, 2)];

        expand_flexible_tracks(
            &host,
            &mut state,
            Axis::Horizontal,
            &mut tracks,
            None,
            items.as_mut_slice(),
            AvailableSpace::MaxContent,
            &mut Vec::new(),
        );
        assert_eq!(tracks.tracks[0].base, 10.0);
        assert_eq!(tracks.tracks[1].base, 80.0);
        assert_eq!(tracks.tracks[2].base, 20.0);
        assert_eq!(tree.calls.borrow().len(), 1);
        assert_eq!(Axis::Vertical.point(items[0].overflow), Overflow::Visible);
    }
}
