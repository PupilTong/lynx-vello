//! CSS Grid Layout (Level 2 wording, excluding `subgrid`).
//!
//! §6.2's item sizing lives in [`normal_self_alignment_is_start`]: `normal`
//! stretches an item only when it has no preferred aspect ratio and is not a
//! replaced box with a natural size in that axis, which is why an image with
//! default alignment stays at its own pixels while an `aspect-ratio` box still
//! fills its column. What a stretched — or authored, or natural — size in one
//! axis then transfers into the other axis's track contribution is §11.5, in
//! `sizing::raw_content_size`.

#![allow(clippy::cast_precision_loss)]

mod alignment;
mod lanes;
mod placement;
mod sizing;
mod tracks;
mod types;

use alignment::{align_tracks, alignment_spacing_from_free_space, item_alignment_offset};
pub use lanes::compute_grid_lanes_layout;
use placement::{
    AxisPlacement, GridArea, GridPlacement, PlacementInput, grid_placement, place_items,
    resolve_axis_placement,
};
use sizing::{
    CrossAxisTracks, IntrinsicSizingScratch, initialize_tracks, probe_raw_min_content,
    resolve_item_intrinsic_dimensions, size_tracks,
};
use stylo::computed_values::direction;
use stylo::values::computed::{PositionProperty, Size as StyleSize};
use stylo::values::specified::align::AlignFlags;
use tracks::{
    ExpandedTemplate, MAX_MATERIALIZED_TRACKS, build_axis_tracks, expand_template, occupied_tracks,
};
use types::{Axis, GridItem, MaxKind, MinKind, TrackSet};

use super::util::{
    BaselineSet, IntrinsicTag, ItemKey, OrderedItem, PendingLayoutItem, ResolvedContainerBox,
    accumulate_scrollable_overflow, apply_aspect_ratio, axis_has_intrinsic_style, box_inset_size,
    clamp, clamp_axis, container_content_independence, is_scroll_container, item_end_margin,
    item_value_stability, normalize_content_alignment, normalize_item_alignment_with_last_baseline,
    own_scrollable_overflow, resolve_container_box, resolve_gap, resolve_insets,
    resolve_item_geometry, sort_and_assign_layout_order,
};
use super::{AbsoluteContainingBlock, compute_absolute_layout_in, hide_subtree};
use crate::geometry::{Edges, Line, Point, Size};
use crate::style::containment::contained_axes;
use crate::style::{Contain, CoreStyle, Display, GridStyle, Overflow};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
    SizingMode,
};

#[derive(Debug, Clone, Copy)]
struct ItemDefaults {
    align_items: AlignFlags,
    align_items_normal: bool,
    justify_items: AlignFlags,
    justify_items_normal: bool,
    rtl: bool,
}

/// Compact order and classification record for an unresolved item.
#[derive(Debug, Clone, Copy)]
struct PendingItem<N> {
    ordered: OrderedItem<N>,
    position: PositionProperty,
    row: Line<GridPlacement>,
    column: Line<GridPlacement>,
}

impl<N: Copy> PendingItem<N> {
    #[inline]
    fn key(self) -> ItemKey<N> {
        self.ordered.key()
    }
}

impl<N> PlacementInput for PendingItem<N> {
    #[inline]
    fn column(&self) -> Line<GridPlacement> {
        self.column
    }

    #[inline]
    fn row(&self) -> Line<GridPlacement> {
        self.row
    }
}

impl<N> PendingLayoutItem<N> for PendingItem<N> {
    #[inline]
    fn ordered(&self) -> &OrderedItem<N> {
        &self.ordered
    }

    #[inline]
    fn ordered_mut(&mut self) -> &mut OrderedItem<N> {
        &mut self.ordered
    }
}

fn classify_item<N, S>(
    node: N,
    style: &S,
    display: Display,
    document_index: usize,
) -> Option<PendingItem<N>>
where
    S: GridStyle,
{
    if display.is_none() {
        return None;
    }
    let position = style.position();
    let in_flow = !matches!(
        position,
        PositionProperty::Absolute | PositionProperty::Fixed
    );
    Some(PendingItem {
        ordered: OrderedItem {
            node,
            document_index,
            css_order: if in_flow { style.order() } else { 0 },
            layout_order: 0,
        },
        position,
        row: Line::new(
            grid_placement(style.grid_row_start()),
            grid_placement(style.grid_row_end()),
        ),
        column: Line::new(
            grid_placement(style.grid_column_start()),
            grid_placement(style.grid_column_end()),
        ),
    })
}

/// css-grid-1 §6.2: whether `normal` self-alignment sizes this item as `start`
/// rather than as `stretch`, per axis.
///
/// `normal` is `stretch` only for an item with no preferred aspect ratio that
/// is not a replaced box with a natural size in that axis. Everything else is
/// sized by CSS 2 § 10's block-level rules for the axis, which this models as
/// `start` plus the ratio transfer [`item_area_geometry`] already applies:
///
/// * inline axis, non-replaced (CSS 2 § 10.3.3): the stretch-fit size — a box whose only ratio is
///   the `aspect-ratio` property still fills its column and derives its row size from that width,
///   so `normal` stays `stretch`;
/// * inline axis, replaced (CSS 2 § 10.3.4): the natural or ratio-transferred size, never the
///   column's;
/// * block axis (CSS 2 § 10.6.2): likewise the natural or transferred size.
///
/// seldon learns that an item is replaced only from the natural size its host
/// reports for it, so a replaced box that reports none is aligned as a
/// non-replaced one — which is what § 6.2 asks for anyway once no ratio is
/// left to preserve. Size containment deliberately does *not* suppress the
/// natural size here: it changes what the box measures to, not whether it is
/// replaced, so a `contain: size` image is start-aligned at its substituted
/// (by default zero) size rather than stretched.
fn normal_self_alignment_is_start(
    natural_size: Size<Option<f32>>,
    aspect_ratio: Option<f32>,
) -> Size<bool> {
    Size::new(
        natural_size.width.is_some(),
        natural_size.height.is_some() || aspect_ratio.is_some(),
    )
}

fn resolve_grid_item<N, S>(
    style: &S,
    key: ItemKey<N>,
    area: GridArea,
    percentage_basis: Size<Option<f32>>,
    defaults: ItemDefaults,
) -> GridItem<N>
where
    S: GridStyle,
{
    let raw_size = style.size();
    let raw_min_size = style.min_size();
    let geometry = resolve_item_geometry(style, percentage_basis);
    let behaves_auto_or_depends = |value: &StyleSize| match value {
        StyleSize::Auto
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => true,
        StyleSize::LengthPercentage(length) => length.0.to_length().is_none(),
        StyleSize::MinContent | StyleSize::MaxContent | StyleSize::FitContentFunction(_) => false,
        // A grid item is in flow, so §5.1.1 resolves nothing for it: the
        // fallback, or `auto` when the declaration is invalid.
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            match crate::compute::anchor::unresolvable_style_size(value) {
                StyleSize::LengthPercentage(length) => length.0.to_length().is_none(),
                _ => true,
            }
        }
    };
    let minimum_behaves_auto = |value: &StyleSize| {
        matches!(
            value,
            StyleSize::Auto
                | StyleSize::FitContent
                | StyleSize::Stretch
                | StyleSize::WebkitFillAvailable
        )
    };
    // A `normal` used value is the one §6.2 rules on; anything the author (or
    // the container's `*-items`) names outright is taken as written.
    let normal_is_start =
        normal_self_alignment_is_start(style.natural_size(), geometry.aspect_ratio);
    let used_align = if defaults.align_items_normal && normal_is_start.height {
        AlignFlags::START
    } else {
        defaults.align_items
    };
    let used_justify = if defaults.justify_items_normal && normal_is_start.width {
        AlignFlags::START
    } else {
        defaults.justify_items
    };
    // No size, min/max size, margin or padding is a percentage (or an anchor
    // function) and no size is an intrinsic keyword, so the geometry is the
    // same against every basis and nothing fills it in later:
    // `refresh_item_basis` leaves it as it is.
    let mut geometry = geometry;
    geometry.basis_free = !geometry.intrinsic.has_intrinsic()
        && !crate::style::box_model_reads_percentage_basis(style);
    GridItem {
        geometry,
        key,
        area,
        position: style.position(),
        align_self: normalize_item_alignment_with_last_baseline(
            style.align_self().0,
            false,
            defaults.rtl,
        )
        .unwrap_or(used_align),
        justify_self: normalize_item_alignment_with_last_baseline(
            style.justify_self().0,
            true,
            defaults.rtl,
        )
        .unwrap_or(used_justify),
        direction: style.direction(),
        preferred_behaves_auto_or_depends: Size::new(
            behaves_auto_or_depends(raw_size.width),
            behaves_auto_or_depends(raw_size.height),
        ),
        minimum_is_auto: Size::new(
            minimum_behaves_auto(raw_min_size.width),
            minimum_behaves_auto(raw_min_size.height),
        ),
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

fn expand_explicit_tracks<S>(
    style: &S,
    repeat_max_basis: Size<Option<f32>>,
    repeat_min_basis: Size<Option<f32>>,
    gap: Size<f32>,
) -> (ExpandedTemplate<'_>, ExpandedTemplate<'_>)
where
    S: GridStyle,
{
    let columns = expand_template(
        style.grid_template_columns(),
        repeat_max_basis.width,
        repeat_min_basis.width,
        gap.width,
    );
    let rows = expand_template(
        style.grid_template_rows(),
        repeat_max_basis.height,
        repeat_min_basis.height,
        gap.height,
    );
    (columns, rows)
}

#[allow(clippy::too_many_arguments)]
fn size_and_align_axis<T>(
    tree: &T,
    state: &mut T::State,
    axis: Axis,
    tracks: &mut TrackSet,
    cross_tracks: CrossAxisTracks<'_>,
    items: &mut [GridItem<T::NodeId>],
    basis: Size<Option<f32>>,
    inner_axis_size: Option<f32>,
    available: AvailableSpace,
    content_alignment: AlignFlags,
    scratch: &mut IntrinsicSizingScratch,
) where
    T: LayoutTree,
{
    size_tracks(
        tree,
        state,
        axis,
        tracks,
        Some(cross_tracks),
        items,
        basis,
        inner_axis_size.map_or(available, AvailableSpace::Definite),
        content_alignment,
        scratch,
    );
    if let Some(size) = inner_axis_size {
        align_tracks(tracks, size, content_alignment);
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_track_sizing<T>(
    tree: &T,
    state: &mut T::State,
    columns: &mut TrackSet,
    rows: &mut TrackSet,
    column_specs: &[tracks::AxisTrackSpec<'_>],
    row_specs: &[tracks::AxisTrackSpec<'_>],
    items: &mut [GridItem<T::NodeId>],
    inner_basis: Size<Option<f32>>,
    available: Size<AvailableSpace>,
    gap: Size<f32>,
    justify_content: AlignFlags,
    align_content: AlignFlags,
    scratch: &mut IntrinsicSizingScratch,
    // The items are as `resolve_grid_item` built them — resolved against no
    // basis, nothing measured — which every other run starts by restoring.
    items_fresh: bool,
) where
    T: LayoutTree,
{
    if !items_fresh {
        for item in items.iter_mut() {
            refresh_item_basis(tree, item, Size::NONE);
            item.clear_contribution_cache(Axis::Horizontal);
            item.clear_contribution_cache(Axis::Vertical);
        }
    }
    initialize_tracks(rows, row_specs, inner_basis.height, gap.height);
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
        tracks: rows,
        distributed_gap,
    };
    initialize_tracks(columns, column_specs, inner_basis.width, gap.width);
    size_and_align_axis(
        tree,
        state,
        Axis::Horizontal,
        columns,
        initial_column_cross_tracks,
        items,
        inner_basis,
        inner_basis.width,
        available.width,
        justify_content,
        scratch,
    );

    let needs_column_feedback = columns
        .tracks
        .iter()
        .any(|track| track.min.is_intrinsic() || track.max.is_intrinsic() || track.is_flexible());
    let mut before_row_min_content = Vec::new();
    if needs_column_feedback {
        before_row_min_content.reserve(items.len());
        for item in items.iter_mut() {
            let range = columns.span_indices(item.area.column.start, item.area.column.end);
            let affects_intrinsic_column = columns.tracks[range].iter().any(|track| {
                track.min.is_intrinsic() || track.max.is_intrinsic() || track.is_flexible()
            });
            before_row_min_content.push(
                (affects_intrinsic_column && item.preferred_behaves_auto_or_depends.width).then(
                    || {
                        probe_raw_min_content(
                            tree,
                            state,
                            item,
                            Axis::Horizontal,
                            Some(initial_column_cross_tracks),
                        )
                    },
                ),
            );
        }
    }
    for item in items.iter_mut() {
        let width = columns.area_size(item.area.column.start, item.area.column.end);
        refresh_item_basis(tree, item, Size::new(Some(width), None));
        item.clear_contribution_cache(Axis::Vertical);
    }
    let mut row_basis = inner_basis;
    row_basis.width = row_basis.width.or(Some(columns.used_size()));
    size_and_align_axis(
        tree,
        state,
        Axis::Vertical,
        rows,
        CrossAxisTracks::resolved(columns),
        items,
        row_basis,
        inner_basis.height,
        available.height,
        align_content,
        scratch,
    );

    let mut column_feedback_changed = false;
    if needs_column_feedback {
        for (item, before) in items.iter_mut().zip(before_row_min_content) {
            let Some(before) = before else {
                continue;
            };
            item.clear_contribution_cache(Axis::Horizontal);
            let after = probe_raw_min_content(
                tree,
                state,
                item,
                Axis::Horizontal,
                Some(CrossAxisTracks::resolved(rows)),
            );
            let tolerance = f32::EPSILON * before.abs().max(after.abs()).max(1.0);
            column_feedback_changed |= (before - after).abs() > tolerance;
        }
    }

    if column_feedback_changed {
        for item in items.iter_mut() {
            let width = columns.area_size(item.area.column.start, item.area.column.end);
            let height = rows.area_size(item.area.row.start, item.area.row.end);
            refresh_item_basis(tree, item, Size::new(Some(width), Some(height)));
        }
        for item in items.iter_mut() {
            item.clear_contribution_cache(Axis::Horizontal);
        }
        initialize_tracks(columns, column_specs, inner_basis.width, gap.width);
        size_and_align_axis(
            tree,
            state,
            Axis::Horizontal,
            columns,
            CrossAxisTracks::resolved(rows),
            items,
            inner_basis,
            inner_basis.width,
            available.width,
            justify_content,
            scratch,
        );
        for item in items.iter_mut() {
            let width = columns.area_size(item.area.column.start, item.area.column.end);
            let height = rows.area_size(item.area.row.start, item.area.row.end);
            refresh_item_basis(tree, item, Size::new(Some(width), Some(height)));
            item.clear_contribution_cache(Axis::Vertical);
        }
        initialize_tracks(rows, row_specs, inner_basis.height, gap.height);
        let mut final_row_basis = inner_basis;
        final_row_basis.width = final_row_basis.width.or(Some(columns.used_size()));
        size_and_align_axis(
            tree,
            state,
            Axis::Vertical,
            rows,
            CrossAxisTracks::resolved(columns),
            items,
            final_row_basis,
            inner_basis.height,
            available.height,
            align_content,
            scratch,
        );
    }
}

/// What one axis of the provisional track-sizing run was sized against, and
/// the size the container settled on: whether sizing the tracks again at the
/// final definite size ([`DefiniteRerun::moves_axis`]) could change them.
struct DefiniteRerun<'a> {
    specs: &'a [tracks::AxisTrackSpec<'a>],
    /// The provisional run's tracks.
    tracks: &'a TrackSet,
    /// The provisional run's percentage basis.
    basis: Option<f32>,
    /// The provisional run's available grid space.
    available: AvailableSpace,
    /// The provisional run's used track size, gaps included.
    used: f32,
    final_inner: f32,
    inset: f32,
    /// The other axis's sizing distributes this axis's free space by the
    /// content alignment, against the percentage basis.
    spacing_reads_basis: bool,
}

impl DefiniteRerun<'_> {
    /// Whether the definite rerun could size this axis differently from the
    /// provisional run — other than by the rounding of the same arithmetic
    /// in another order, which it may. The rerun differs from the
    /// provisional run in its percentage basis, now the final inner size, and
    /// in its available space, now that size as a definite one.
    ///
    /// A basis that moved changes a percentage track breadth or
    /// `fit-content()` limit (`tracks::specs_depend_on_basis`), an item's
    /// `fit-content()` size, whose percentage limit resolves against it, and
    /// column sizing's spacing of fixed rows by a distributing
    /// `align-content`; nothing else in track sizing reads it (item geometry
    /// resolves against the grid area, not the container).
    ///
    /// An available space that moved from a max-content constraint to the
    /// final size repeats the provisional sizes when that size is the tracks'
    /// own used size and every track is of a kind for which that holds. The
    /// constraint sizes an `auto`-minimum track's base from limited
    /// max-content contributions (§12.5 step 2) where the rerun uses the
    /// minimum contributions, which are no larger, and both runs reach the
    /// same growth limits; §12.6 then maximizes every track to its growth
    /// limit in both, the free space being exactly the difference; §12.7's
    /// definite fr size solves to the indefinite one, every flexible track
    /// being at least as large as its base in both; §12.8 has nothing left to
    /// stretch. The kinds it does not hold for keep the rerun:
    ///
    /// * a final size the container's min/max sizes or size containment moved away from the used
    ///   size, which leaves free space (positive or negative) the rerun distributes — an
    ///   auto-height grid's `min-height` stretching its `auto` rows, as §12.8 asks;
    /// * a flex factor below 1, whose definite fr size leaves space unfilled that the indefinite
    ///   one filled (§12.7.1 floors the factor sum at 1);
    /// * an `auto` minimum under a `min-content` maximum, whose max-content base outgrows the
    ///   min-content growth limit the rerun keeps;
    /// * an item spanning several tracks none of them flexible, which the constraint also
    ///   distributes in a max-content base phase (§12.5 step 3) the rerun does not run, and whose
    ///   growth-limit phases read the bases that differ — not shown to come out the same, so kept.
    ///
    /// A min-content constraint sizes bases from limited min-content
    /// contributions and stops there, and the rerun shares the difference to
    /// the minimum contributions out equally rather than back to the tracks
    /// it came from, so it always keeps the rerun.
    fn moves_axis<N>(&self, items: &[GridItem<N>], axis: Axis) -> bool {
        let basis_moved = self.basis != Some(self.final_inner);
        if basis_moved
            && (self.spacing_reads_basis
                || tracks::specs_depend_on_basis(self.specs)
                || items.iter().any(|item| {
                    item.intrinsic.preferred(axis) == IntrinsicTag::FitContent
                        || item.intrinsic.minimum(axis) == IntrinsicTag::FitContent
                        || item.intrinsic.maximum(axis) == IntrinsicTag::FitContent
                }))
        {
            return true;
        }
        // The final inner size is the used size passed through the outer
        // size and back, so it can be a rounding step away from it.
        let tolerance = 2.0 * f32::EPSILON * (self.used.abs() + self.inset).max(1.0);
        match self.available {
            AvailableSpace::Definite(available) => (available - self.final_inner).abs() > tolerance,
            AvailableSpace::MinContent => true,
            AvailableSpace::MaxContent => {
                (self.final_inner - self.used).abs() > tolerance
                    || self.tracks.tracks.iter().any(|track| {
                        (track.is_flexible() && track.flex_factor < 1.0)
                            || (track.min == MinKind::Auto && track.max == MaxKind::MinContent)
                    })
                    || items.iter().any(|item| {
                        let span = match axis {
                            Axis::Horizontal => item.area.column,
                            Axis::Vertical => item.area.row,
                        };
                        let spanned =
                            &self.tracks.tracks[self.tracks.span_indices(span.start, span.end)];
                        spanned.len() > 1
                            && !spanned.iter().any(types::Track::is_flexible)
                            && spanned
                                .iter()
                                .any(|track| track.min.is_intrinsic() || track.max.is_intrinsic())
                    })
            }
        }
    }
}

/// The container's used outer size on one axis: the parent's or the style's
/// own size when there is one, otherwise the content it holds plus padding and
/// border, clamped by min/max and floored at the box inset.
fn final_outer_axis(metrics: &ResolvedContainerBox, axis: Axis, content: f32) -> f32 {
    axis.size(metrics.outer).unwrap_or_else(|| {
        clamp_axis(
            content + axis.size(metrics.box_inset),
            axis.size(metrics.min),
            axis.size(metrics.max),
            axis.size(metrics.box_inset),
        )
    })
}

fn final_outer_size(metrics: &ResolvedContainerBox, tracks: Size<f32>) -> Size<f32> {
    Size::new(
        final_outer_axis(metrics, Axis::Horizontal, tracks.width),
        final_outer_axis(metrics, Axis::Vertical, tracks.height),
    )
}

/// css-sizing-3 §5.2.2's fit-content size, `min(max-content, max(min-content,
/// stretch-fit))`, of a container's content box in its inline axis, where
/// `intrinsic` sizes the container's tracks under the min- or max-content
/// constraint it is handed and answers their used size.
///
/// The max-content size is the answer whenever it fits the stretch-fit size,
/// so the min-content run happens only when it overflows.
fn fit_content_inner_size(
    stretch_fit: f32,
    mut intrinsic: impl FnMut(AvailableSpace) -> f32,
) -> f32 {
    let max_content = intrinsic(AvailableSpace::MaxContent);
    if max_content <= stretch_fit {
        return max_content;
    }
    let min_content = intrinsic(AvailableSpace::MinContent);
    max_content.min(min_content.max(stretch_fit))
}

/// The used inner size, on `axis`, of a container whose content box comes to
/// `content`: the outer size its min/max sizes clamp, less padding and border.
fn clamped_inner_axis(metrics: &ResolvedContainerBox, axis: Axis, content: f32) -> f32 {
    (final_outer_axis(metrics, axis, content) - axis.size(metrics.box_inset)).max(0.0)
}

/// The container-level values a Grid-family algorithm resolves before it looks
/// at any item: the box metrics with the available grid space applied (below),
/// which axes the style itself makes definite, and the percentage bases that
/// templates and gutters resolve against.
///
/// css-grid-2 §12.1's available grid space has two cases only: the grid
/// container's size where it is definite, otherwise the min- or max-content
/// constraint it is sized under. A definite *available* size with no size of
/// the container's own — an absolutely positioned box that does not stretch
/// (css-position-3 §4.1), `fit-content`, an unstretched flex or grid item
/// (css-grid-2 §6.2) — is neither: the container is sized first, by
/// css-sizing-3, and its tracks then against that size. In the block axis that
/// size is the max-content size (css-sizing-3 §5.2: for a box like this one,
/// whose min- and max-content block sizes coincide, `fit-content` and `auto`
/// both come to it), so `metrics.available_inner.height` becomes a max-content
/// constraint here. In the inline axis it is the fit-content size between the
/// two intrinsic sizes, which only the algorithm's own track-sizing runs
/// produce: `fit_content_width` hands it the stretch-fit size to resolve that
/// against ([`fit_content_inner_size`]), and while it is `Some`,
/// `metrics.available_inner.width` is that stretch-fit size, not the available
/// grid space.
struct ContainerPrologue {
    metrics: ResolvedContainerBox,
    fit_content_width: Option<f32>,
    style_definite: Size<bool>,
    percentage_basis: Size<Option<f32>>,
    gap: Size<f32>,
    repeat_max_basis: Size<Option<f32>>,
    repeat_min_basis: Size<Option<f32>>,
    repeat_count_gap: Size<f32>,
}

fn container_prologue<S: CoreStyle>(style: &S, input: LayoutInput) -> ContainerPrologue {
    let preferred = style.size();
    let gap_value = style.gap();
    let mut metrics = resolve_container_box(style, input);
    let style_definite = metrics.preferred_definite;
    let outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );
    if input.sizing_mode != SizingMode::IgnoreSizeStyles {
        if metrics.inner.width.is_none() {
            metrics.available_inner.width = match preferred.width {
                StyleSize::MinContent => AvailableSpace::MinContent,
                StyleSize::MaxContent => AvailableSpace::MaxContent,
                _ => metrics.available_inner.width,
            };
        }
        if metrics.inner.height.is_none() {
            metrics.available_inner.height = match preferred.height {
                StyleSize::MinContent => AvailableSpace::MinContent,
                StyleSize::MaxContent => AvailableSpace::MaxContent,
                _ => metrics.available_inner.height,
            };
        }
    }
    let fit_content_width = if metrics.inner.width.is_none() {
        metrics.available_inner.width.definite_value()
    } else {
        None
    };
    if metrics.inner.height.is_none() && metrics.available_inner.height.is_definite() {
        metrics.available_inner.height = AvailableSpace::MaxContent;
    }
    let percentage_basis = Size::new(
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
    let gap = resolve_gap(gap_value, percentage_basis);
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
    let repeat_count_basis = Size::new(
        repeat_max_basis.width.or(repeat_min_basis.width),
        repeat_max_basis.height.or(repeat_min_basis.height),
    );
    let repeat_count_gap = resolve_gap(gap_value, repeat_count_basis);
    ContainerPrologue {
        metrics,
        fit_content_width,
        style_definite,
        percentage_basis,
        gap,
        repeat_max_basis,
        repeat_min_basis,
        repeat_count_gap,
    }
}

/// An item in a row's baseline-sharing group, held back from commit until
/// its group is aligned.
#[derive(Debug)]
struct PendingBaselineItem<N> {
    node: N,
    /// The item's position in order-modified document order.
    index: usize,
    set: BaselineSet,
    area: GridArea,
    area_top: f32,
    area_bottom: f32,
    /// The item's box, its location before any relative offset; the group
    /// pass moves it, and the commit adds `relative`.
    layout: Layout,
    relative: Point<f32>,
    /// The item's first and last baselines, the one it aligns by synthesized
    /// from its border box's bottom edge when it has none.
    baselines: [Option<f32>; 2],
    overflow: Point<Overflow>,
    sticky_area: Option<Edges<f32>>,
}

impl<N> PendingBaselineItem<N> {
    /// The baseline the item aligns by, from its border-box top edge.
    fn aligned_baseline(&self) -> f32 {
        self.baselines[self.set.index()].unwrap_or(self.layout.size.height)
    }
}

/// The candidates of css-grid-1 §10.6 steps 3 and 4, kept while the items are
/// placed: the first item in grid order whose span starts in the first
/// non-empty row, and the last whose span ends in the last one, with the
/// baseline each gives — its own, or one synthesized from its border box's
/// bottom edge. Steps 3 and 4 are taken together, as the first baseline always
/// took them: that item's baseline, rather than the first (last) item that
/// has one.
///
/// Grid order visits cells row by row, an item at its first cell, and items
/// that meet at once in order-modified document order (`index`).
#[derive(Debug, Default)]
struct GridOrderCandidates {
    /// `(row start, column start, index)`, smallest first.
    first: Option<((i32, i32, usize), f32)>,
    /// `(row end, row start, column start, index)`, largest last.
    last: Option<((i32, i32, i32, usize), f32)>,
}

impl GridOrderCandidates {
    /// Offers one item at its aligned border-box top, before any relative
    /// offset: relative positioning moves no other box's baseline
    /// (css-position-3 §2), its container's included.
    fn offer(
        &mut self,
        index: usize,
        area: GridArea,
        top: f32,
        height: f32,
        baselines: [Option<f32>; 2],
    ) {
        let first_key = (area.row.start, area.column.start, index);
        if self.first.is_none_or(|(key, _)| first_key < key) {
            self.first = Some((first_key, top + baselines[0].unwrap_or(height)));
        }
        let last_key = (area.row.end, area.row.start, area.column.start, index);
        if self.last.is_none_or(|(key, _)| last_key > key) {
            self.last = Some((last_key, top + baselines[1].unwrap_or(height)));
        }
    }
}

/// Aligns each row's baseline-sharing groups (css-align-3 §9.3): the
/// `baseline` items whose areas start in the row, their first baselines
/// lined up and the group flush with the row's start edge, and the `last
/// baseline` items whose areas end in it, their last baselines lined up and
/// the group flush with the row's end edge. Rows run top to bottom in this
/// engine, so no reversal applies. A group taller than its areas overflows
/// the other edge: as in browsers, no `safe` alignment moves it back.
fn align_baseline_groups<N>(pending: &mut [PendingBaselineItem<N>]) {
    // A group is one set at one row line: the start line for first
    // baselines, the end line for last ones.
    let groups = pending
        .iter()
        .map(|item| match item.set {
            BaselineSet::First => (item.set, item.area.row.start),
            BaselineSet::Last => (item.set, item.area.row.end),
        })
        .collect::<Vec<_>>();
    let mut order = (0..pending.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| groups[index]);
    for group in order.chunk_by(|&a, &b| groups[a] == groups[b]) {
        let set = groups[group[0]].0;
        // The shared baseline: for a start-flush group, as low as the member
        // whose outer top edge rests on its area's start puts it; for an
        // end-flush group, as high as the member whose outer bottom edge rests
        // on its area's end puts it.
        let shared = group
            .iter()
            .map(|&index| {
                let item = &pending[index];
                let baseline = item.aligned_baseline();
                match set {
                    BaselineSet::First => item.area_top + item.layout.margin.top + baseline,
                    BaselineSet::Last => {
                        item.area_bottom - item.layout.margin.bottom - item.layout.size.height
                            + baseline
                    }
                }
            })
            .reduce(match set {
                BaselineSet::First => f32::max,
                BaselineSet::Last => f32::min,
            })
            .expect("a group has a member");
        for &index in group {
            let item = &mut pending[index];
            item.layout.location.y = shared - item.aligned_baseline();
        }
    }
}

/// The container's first and last baselines (css-grid-1 §10.6): a shared
/// baseline of the first (last) non-empty row's baseline-sharing groups — the
/// matching set's first, then the other set's — else the grid-order
/// candidate's. A spanning item belongs to the row its span starts in for a
/// first baseline and ends in for a last one, the other way round for the
/// other set's group.
fn grid_container_baselines<N>(
    pending: &[PendingBaselineItem<N>],
    candidates: &GridOrderCandidates,
) -> [Option<f32>; 2] {
    let shared = |set: BaselineSet, row_start: Option<i32>, row_end: Option<i32>| {
        pending
            .iter()
            .find(|item| {
                item.set == set
                    && row_start.is_none_or(|row| item.area.row.start == row)
                    && row_end.is_none_or(|row| item.area.row.end == row)
            })
            .map(|item| item.layout.location.y + item.aligned_baseline())
    };
    let first = candidates.first.map(|((row, _, _), fallback)| {
        shared(BaselineSet::First, Some(row), None)
            .or_else(|| shared(BaselineSet::Last, None, Some(row + 1)))
            .unwrap_or(fallback)
    });
    let last = candidates.last.map(|((row_end, _, _, _), fallback)| {
        shared(BaselineSet::Last, None, Some(row_end))
            .or_else(|| shared(BaselineSet::First, Some(row_end - 1), None))
            .unwrap_or(fallback)
    });
    [first, last]
}

fn refresh_item_basis<T>(
    tree: &T,
    item: &mut GridItem<T::NodeId>,
    percentage_basis: Size<Option<f32>>,
) where
    T: LayoutTree,
{
    if item.basis_free {
        return;
    }
    let style = tree.style(item.key.node);
    item.geometry = resolve_item_geometry(&style, percentage_basis);
}

fn physical_area<N>(
    item: &GridItem<N>,
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

/// Whether every track the span covers is sized by its own function alone —
/// no track that consults the items placed in it, and so no track this item's
/// content can move.
fn span_is_fixed(tracks: &TrackSet, span: crate::compute::grid::placement::TrackSpan) -> bool {
    let indices = tracks.span_indices(span.start, span.end);
    !tracks.tracks[indices]
        .iter()
        .any(|track| track.min.is_intrinsic() || track.max.is_intrinsic() || track.is_flexible())
}

/// The visual offset `position: relative` adds to an item, taken from
/// whichever inset of each pair is not `auto`, its percentages resolved
/// against `basis`, the item's area.
fn relative_item_offset<T>(
    tree: &T,
    item: &GridItem<T::NodeId>,
    basis: Size<Option<f32>>,
) -> Point<f32>
where
    T: LayoutTree,
{
    if item.position != PositionProperty::Relative {
        return Point::ZERO;
    }
    let inset = resolve_insets(tree.style(item.key.node).inset(), basis);
    Point::new(
        inset.left.unwrap_or_else(|| -inset.right.unwrap_or(0.0)),
        inset.top.unwrap_or_else(|| -inset.bottom.unwrap_or(0.0)),
    )
}

/// One item's child sizing inputs inside an area: the dimensions the area
/// decides for it, and the available space each axis offers: the area, less
/// the item's margins, whether the item stretches into it or fit-contents
/// inside it. An axis whose area is `None` is indefinite — nothing stretches
/// into it, and it offers max-content space.
fn item_area_geometry<N>(
    item: &GridItem<N>,
    area: Size<Option<f32>>,
) -> (Size<Option<f32>>, Size<AvailableSpace>) {
    let resolved_preferred = item.preferred_size;
    let margin_sum = Size::new(item.margin.horizontal_sum(), item.margin.vertical_sum());
    let inner = area.zip_map(margin_sum, |edge, margin| {
        edge.map(|value| (value - margin).max(0.0))
    });
    let intrinsic = Size::new(
        item.intrinsic.preferred(Axis::Horizontal).is_intrinsic(),
        item.intrinsic.preferred(Axis::Vertical).is_intrinsic(),
    );
    let mut known = resolved_preferred;
    if intrinsic.width {
        known.width = None;
    }
    if intrinsic.height {
        known.height = None;
    }
    let stretches = Size::new(
        item.justify_self == AlignFlags::STRETCH
            && known.width.is_none()
            && !intrinsic.width
            && !item.margin_auto.start(Axis::Horizontal)
            && !item.margin_auto.end(Axis::Horizontal),
        item.align_self == AlignFlags::STRETCH
            && known.height.is_none()
            && !intrinsic.height
            && !item.margin_auto.start(Axis::Vertical)
            && !item.margin_auto.end(Axis::Vertical),
    );
    if stretches.width {
        known.width = inner.width;
    }
    if stretches.height {
        known.height = inner.height;
    }
    known = apply_aspect_ratio(known, item.aspect_ratio);
    known.width = known
        .width
        .map(|value| clamp(value, item.min_size.width, item.max_size.width));
    known.height = known
        .height
        .map(|value| clamp(value, item.min_size.height, item.max_size.height));

    let axis_available = |axis: Axis| match item.intrinsic.preferred(axis) {
        IntrinsicTag::MinContent => AvailableSpace::MinContent,
        IntrinsicTag::MaxContent => AvailableSpace::MaxContent,
        IntrinsicTag::FitContent => axis
            .size(resolved_preferred)
            .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
        // css-grid-2 §6.2: an item that is not stretched is sized as for
        // `fit-content` within its area (css-sizing-3 §5.2.2), so its
        // stretch-fit size is the area's, not a max-content constraint.
        IntrinsicTag::None => axis
            .size(inner)
            .map_or(AvailableSpace::MaxContent, AvailableSpace::Definite),
    };
    (
        known,
        Size::new(
            axis_available(Axis::Horizontal),
            axis_available(Axis::Vertical),
        ),
    )
}

/// The independence a committing container can vouch for on one item's input.
/// The item's area is what its percentages resolve against and what a stretch
/// fills, so independence chains off the area rather than the container's own
/// box. A fixed-function track cannot move at all; an intrinsic or flexible
/// one moves only with the contributions it collects, which this item's
/// content cannot touch as long as its own contributions are content-free.
fn item_commit_independence<T>(
    tree: &T,
    item: &GridItem<T::NodeId>,
    known: Size<Option<f32>>,
    area_is_fixed: Size<bool>,
    container: Size<bool>,
) -> Size<bool>
where
    T: LayoutTree,
{
    let fixed_area = Size::new(
        container.width && area_is_fixed.width,
        container.height && area_is_fixed.height,
    );
    let style = tree.style(item.key.node);
    let (values_stable, edges_stable) = item_value_stability(&style, item.aspect_ratio, fixed_area);
    let axis_independent = |axis: Axis| {
        let area_stable = axis.size(fixed_area)
            || (axis.size(container) && contribution_is_content_free(item, axis));
        edges_stable
            && axis.size(values_stable)
            && !axis_has_intrinsic_style(item, axis)
            && area_stable
            && axis.size(known).is_some()
    };
    Size::new(
        axis_independent(Axis::Horizontal),
        axis_independent(Axis::Vertical),
    )
}

/// Whether the item's contributions to its tracks are decided without ever
/// measuring it: a size the style pins, a minimum that is not the automatic
/// content-based one, and no baseline shim measured from its content.
fn contribution_is_content_free<N>(item: &GridItem<N>, axis: Axis) -> bool {
    let automatic_minimum_measures = axis.size(item.minimum_is_auto)
        && axis.size(item.min_size).is_none()
        && !axis.point(item.overflow).is_scrollable();
    let baseline_shim_measures =
        axis == Axis::Vertical && BaselineSet::of_alignment(item.align_self).is_some();
    axis.size(item.preferred_size).is_some()
        && !axis.size(item.preferred_behaves_auto_or_depends)
        && !automatic_minimum_measures
        && !baseline_shim_measures
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::fn_params_excessive_bools
)]
fn layout_in_flow_items<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [GridItem<T::NodeId>],
    columns: &TrackSet,
    rows: &TrackSet,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    // The goal the container hands its items: a measurement passes through;
    // a commit carries the independence the container can vouch for, which
    // each item narrows per axis.
    goal: LayoutGoal,
    // The container's own measurement was asked for its first baseline
    // (`LayoutInput::reads_first_baseline`); never set on a commit.
    first_baseline_requested: bool,
    // The container's own measurement was asked for an exact last baseline
    // (`LayoutInput::reads_last_baseline`); never set on a commit.
    last_baseline_requested: bool,
    rtl: bool,
    scroll_container: bool,
) -> (Size<f32>, [Option<f32>; 2])
where
    T: LayoutTree,
{
    let baseline_item_count = items
        .iter()
        .filter(|item| item.baseline_participation().is_some())
        .count();
    // The item §10.6 steps 3 and 4 read the container's last baseline from
    // (`GridOrderCandidates::last`): the last in grid order of the items whose
    // spans end in the last non-empty row. Placement already decided it.
    let last_candidate = last_baseline_requested
        .then(|| {
            items
                .iter()
                .enumerate()
                .max_by_key(|(index, item)| {
                    (
                        item.area.row.end,
                        item.area.row.start,
                        item.area.column.start,
                        *index,
                    )
                })
                .map(|(index, _)| index)
        })
        .flatten();
    let mut pending = Vec::with_capacity(baseline_item_count);
    let mut content_size = Size::ZERO;
    let mut candidates = GridOrderCandidates::default();

    for (index, item) in items.iter_mut().enumerate() {
        let (area_offset, area_size) = physical_area(item, columns, rows, inner_size, rtl);
        refresh_item_basis(
            tree,
            item,
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        item.clear_contribution_cache(Axis::Horizontal);
        item.clear_contribution_cache(Axis::Vertical);
        resolve_item_intrinsic_dimensions(
            tree,
            state,
            item,
            Axis::Horizontal,
            Some(CrossAxisTracks::resolved(rows)),
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        resolve_item_intrinsic_dimensions(
            tree,
            state,
            item,
            Axis::Vertical,
            Some(CrossAxisTracks::resolved(columns)),
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        let area = area_size.map(Some);
        let (known, available) = item_area_geometry(item, area);
        let item_goal = match goal.independence() {
            None => goal,
            Some(container) => LayoutGoal::Commit {
                content_independent: item_commit_independence(
                    tree,
                    item,
                    known,
                    Size::new(
                        span_is_fixed(columns, item.area.column),
                        span_is_fixed(rows, item.area.row),
                    ),
                    container,
                ),
            },
        };
        // A measurement reads the last baseline of a row's last-baseline group
        // member, and of the item a requested container last baseline comes
        // from; a commit is exact without asking.
        let reads_last_baseline = !goal.commits()
            && (item.baseline_participation() == Some(BaselineSet::Last)
                || last_candidate == Some(index));
        // It reads the first baseline of a row's first-baseline group member,
        // and of every item when the container's own first baseline, which
        // any of them may give, is read.
        let reads_first_baseline = !goal.commits()
            && (item.baseline_participation() == Some(BaselineSet::First)
                || first_baseline_requested);
        let input = LayoutInput::new(item_goal, known, area, available)
            .reading_last_baseline(reads_last_baseline)
            .reading_first_baseline(reads_first_baseline);
        let output = tree.compute_layout(state, item.key.node, input);

        let mut margin = item.margin;
        for axis in Axis::ALL {
            let area = axis.size(area_size);
            let child = axis.size(output.size);
            let auto_start = item.margin_auto.start(axis);
            let auto_end = item.margin_auto.end(axis);
            let fixed_start = axis.start(margin);
            let fixed_end = axis.end(margin);
            let auto_count = usize::from(auto_start) + usize::from(auto_end);
            if auto_count > 0 {
                let share = ((area - child - fixed_start - fixed_end).max(0.0)) / auto_count as f32;
                if auto_start {
                    axis.set_start(&mut margin, share);
                }
                if auto_end {
                    axis.set_end(&mut margin, share);
                }
            }
        }

        let free_x = area_size.width - output.size.width - margin.horizontal_sum();
        let free_y = area_size.height - output.size.height - margin.vertical_sum();
        let item_rtl = item.direction == direction::T::Rtl;
        let offset_x = item_alignment_offset(free_x, item.justify_self, rtl, item_rtl);
        let offset_y = item_alignment_offset(free_y, item.align_self, false, false);
        let relative = relative_item_offset(
            tree,
            item,
            Size::new(Some(area_size.width), Some(area_size.height)),
        );
        // Aligned in its area; a relative offset moves it afterwards without
        // moving anything aligned against it.
        let aligned = Point::new(
            content_origin.x + area_offset.x + margin.left + offset_x,
            content_origin.y + area_offset.y + margin.top + offset_y,
        );
        let mut layout = Layout::with_order(item.key.layout_order);
        layout.location = Point::new(aligned.x + relative.x, aligned.y + relative.y);
        layout.size = output.size;
        layout.content_size = output.content_size;
        layout.border = item.border;
        layout.padding = item.padding;
        layout.margin = margin;
        // A sticky item's insets resolve against its grid area, not the
        // grid container (css-position-3 §3.4): handed to the host as the
        // rare record it is rather than carried by every `Layout`.
        let sticky_area = (item.position == PositionProperty::Sticky).then_some(Edges {
            left: content_origin.x + area_offset.x,
            right: content_origin.x + area_offset.x + area_size.width,
            top: content_origin.y + area_offset.y,
            bottom: content_origin.y + area_offset.y + area_size.height,
        });
        let participation = item.baseline_participation();
        let item_baseline = output
            .first_baselines
            .y
            .or_else(|| (item.align_self == AlignFlags::BASELINE).then_some(output.size.height));
        let baselines = [item_baseline, output.last_baselines.y];
        let Some(set) = participation else {
            candidates.offer(index, item.area, aligned.y, output.size.height, baselines);
            accumulate_scrollable_overflow(
                &mut content_size,
                layout.location,
                layout.size,
                layout.content_size,
                item.overflow,
                item_end_margin(scroll_container, layout.margin),
            );
            if goal.commits() {
                tree.set_unrounded_layout(state, item.key.node, layout);
                tree.set_sticky_containing_block(state, item.key.node, sticky_area);
            }
            continue;
        };
        layout.location = aligned;
        pending.push(PendingBaselineItem {
            node: item.key.node,
            index,
            set,
            area: item.area,
            area_top: content_origin.y + area_offset.y,
            area_bottom: content_origin.y + area_offset.y + area_size.height,
            layout,
            relative,
            baselines,
            overflow: item.overflow,
            sticky_area,
        });
    }

    if pending.is_empty() {
        return (
            content_size,
            [
                candidates.first.map(|(_, baseline)| baseline),
                candidates.last.map(|(_, baseline)| baseline),
            ],
        );
    }

    align_baseline_groups(&mut pending);
    for item in &pending {
        candidates.offer(
            item.index,
            item.area,
            item.layout.location.y,
            item.layout.size.height,
            item.baselines,
        );
    }
    let baselines = grid_container_baselines(&pending, &candidates);
    for mut item in pending {
        item.layout.location.x += item.relative.x;
        item.layout.location.y += item.relative.y;
        accumulate_scrollable_overflow(
            &mut content_size,
            item.layout.location,
            item.layout.size,
            item.layout.content_size,
            item.overflow,
            item_end_margin(scroll_container, item.layout.margin),
        );
        if goal.commits() {
            tree.set_unrounded_layout(state, item.node, item.layout);
            tree.set_sticky_containing_block(state, item.node, item.sticky_area);
        }
    }
    (content_size, baselines)
}

fn absolute_axis_lines(
    placement: Line<GridPlacement>,
    explicit_tracks: usize,
) -> (Option<i32>, Option<i32>) {
    let both_line = matches!(placement.start, GridPlacement::Line(_))
        && matches!(placement.end, GridPlacement::Line(_));
    if !both_line
        && !matches!(placement.start, GridPlacement::Auto)
        && !matches!(placement.end, GridPlacement::Auto)
    {
        return match resolve_axis_placement(placement, explicit_tracks) {
            AxisPlacement::Definite(span) => (Some(span.start), Some(span.end)),
            AxisPlacement::Indefinite { .. } => (None, None),
        };
    }

    let resolve_edge = |line: Line<GridPlacement>, end: bool| match resolve_axis_placement(
        line,
        explicit_tracks,
    ) {
        AxisPlacement::Definite(span) => Some(if end { span.end } else { span.start }),
        AxisPlacement::Indefinite { .. } => None,
    };
    let start = matches!(placement.start, GridPlacement::Line(_))
        .then(|| resolve_edge(Line::new(placement.start, GridPlacement::Span(1)), false))
        .flatten();
    let end = matches!(placement.end, GridPlacement::Line(_))
        .then(|| resolve_edge(Line::new(GridPlacement::Span(1), placement.end), true))
        .flatten();
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

fn absolute_static_offset<T>(
    tree: &T,
    state: &mut T::State,
    item: &GridItem<T::NodeId>,
    containing_size: Size<f32>,
    rtl: bool,
) -> Point<f32>
where
    T: LayoutTree,
{
    let basis = Size::new(Some(containing_size.width), Some(containing_size.height));
    let axis_available = |dimension: IntrinsicTag, available: f32| match dimension {
        IntrinsicTag::MinContent => AvailableSpace::MinContent,
        IntrinsicTag::MaxContent => AvailableSpace::MaxContent,
        IntrinsicTag::FitContent | IntrinsicTag::None => AvailableSpace::Definite(available),
    };
    let intrinsic_available = Size::new(
        axis_available(
            item.intrinsic.preferred(Axis::Horizontal),
            containing_size.width,
        ),
        axis_available(
            item.intrinsic.preferred(Axis::Vertical),
            containing_size.height,
        ),
    );
    let output = tree.compute_layout(
        state,
        item.key.node,
        LayoutInput::measure(
            item.preferred_size,
            basis,
            intrinsic_available,
            RequestedAxis::Both,
        ),
    );
    let item_floor = box_inset_size(item.padding, item.border);
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
        let start_auto = item.margin_auto.start(axis);
        let end_auto = item.margin_auto.end(axis);
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
                item.direction == direction::T::Rtl,
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
#[allow(
    clippy::too_many_lines,
    reason = "one ordered pass; the hoisted boxes interleave with it in tree order"
)]
#[inline(never)]
fn layout_absolute_items<'tree, T>(
    tree: &'tree T,
    state: &mut T::State,
    node: T::NodeId,
    items: &[PendingItem<T::NodeId>],
    columns: &TrackSet,
    rows: &TrackSet,
    explicit_columns: usize,
    explicit_rows: usize,
    inner_size: Size<f32>,
    outer_size: Size<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
    rtl: bool,
    defaults: ItemDefaults,
) -> Size<f32>
where
    T: LayoutTree + 'tree,
    T::Style<'tree>: GridStyle,
{
    let mut content_size = Size::ZERO;
    let padding_box_origin = Point::new(border.left, border.top);
    let padding_box_size = Size::new(
        (outer_size.width - border.horizontal_sum()).max(0.0),
        (outer_size.height - border.vertical_sum()).max(0.0),
    );
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let logical_content_start =
        Point::new(if rtl { padding.right } else { padding.left }, padding.top);
    crate::compute::util::debug_assert_tree_order(
        items.iter().map(|item| item.ordered.document_index),
    );
    let mut hoisted = crate::compute::HoistedPass::new(padding_box_size, border, rtl);
    for pending in items {
        hoisted.before(tree, state, node, pending.ordered.document_index);
        let key = pending.key();
        // `absolute_layout` decides whether it reads the static position.
        let static_axes =
            crate::compute::util::needs_static_position(tree, key.node, &tree.style(key.node));
        let needs_static_measurement = static_axes.width || static_axes.height;
        let content_static_offset = if needs_static_measurement {
            let item = resolve_grid_item(
                &tree.style(key.node),
                key,
                GridArea::default(),
                Size::new(Some(inner_size.width), Some(inner_size.height)),
                defaults,
            );
            absolute_static_offset(tree, state, &item, inner_size, rtl)
        } else {
            Point::ZERO
        };
        match pending.position {
            PositionProperty::Absolute => {
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
                let static_offset = Point::new(
                    padding.left + content_static_offset.x - x,
                    padding.top + content_static_offset.y - y,
                );
                let mut layout = compute_absolute_layout_in(
                    tree,
                    state,
                    key.node,
                    AbsoluteContainingBlock {
                        origin: Point::new(x, y),
                        size: containing_size,
                        padding_box_size,
                        is_padding_box: x == 0.0 && y == 0.0 && containing_size == padding_box_size,
                        rtl,
                        honors_self_alignment: true,
                    },
                    move |_, _| static_offset,
                );
                layout.location.x += origin.x;
                layout.location.y += origin.y;
                layout.order = key.layout_order;
                accumulate_scrollable_overflow(
                    &mut content_size,
                    layout.location,
                    layout.size,
                    layout.content_size,
                    tree.style(key.node).overflow(),
                    Size::ZERO,
                );
                tree.set_unrounded_layout(state, key.node, layout);
                hoisted.inside(tree, state, node, pending.ordered.document_index);
            }
            PositionProperty::Fixed => {
                tree.set_static_position(
                    state,
                    key.node,
                    Point::new(
                        content_origin.x + content_static_offset.x,
                        content_origin.y + content_static_offset.y,
                    ),
                );
            }
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {}
        }
    }
    hoisted.rest(tree, state, node);
    content_size
}

#[allow(clippy::too_many_lines)]
pub fn compute_grid_layout<'tree, T>(
    tree: &'tree T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    T: LayoutTree + 'tree,
    T::Style<'tree>: GridStyle,
{
    let style = tree.style(node);
    let contained_size = contained_axes(&style);
    let layout_contained = style.containment().contains(Contain::LAYOUT);
    let gap_value = style.gap();
    let auto_flow = style.grid_auto_flow();
    let direction = style.direction();
    let rtl = direction == direction::T::Rtl;
    let align_content = normalize_content_alignment(style.align_content().primary(), false, rtl)
        .unwrap_or(AlignFlags::STRETCH);
    let justify_content = normalize_content_alignment(style.justify_content().primary(), true, rtl)
        .unwrap_or(AlignFlags::STRETCH);
    let align_items =
        normalize_item_alignment_with_last_baseline(style.align_items().0, false, rtl);
    let justify_items =
        normalize_item_alignment_with_last_baseline(style.justify_items().computed.0.0, true, rtl);
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
        percentage_basis: initial_percentage_basis,
        gap: initial_gap,
        repeat_max_basis,
        repeat_min_basis,
        repeat_count_gap,
    } = container_prologue(&style, input);
    let (explicit_columns, explicit_rows) =
        expand_explicit_tracks(&style, repeat_max_basis, repeat_min_basis, repeat_count_gap);

    let commits_layout = input.goal.commits();
    let children = tree.flattened_children(node);
    let mut in_flow = Vec::with_capacity(children.capacity_hint());
    let mut absolute = commits_layout.then(Vec::new);
    let mut hidden = commits_layout.then(Vec::new);
    for (document_index, (child, style, display)) in children.enumerate() {
        let Some(child_style) = classify_item(child, &style, display, document_index) else {
            if let Some(hidden) = &mut hidden {
                hidden.push((document_index, child));
            }
            continue;
        };
        if matches!(
            child_style.position,
            PositionProperty::Absolute | PositionProperty::Fixed
        ) {
            if let Some(absolute) = &mut absolute {
                absolute.push(child_style);
            }
        } else {
            in_flow.push(child_style);
        }
    }
    if let Some(absolute) = &mut absolute {
        sort_and_assign_layout_order(&mut in_flow, absolute);
    } else if in_flow.iter().any(|item| item.ordered.css_order != 0) {
        in_flow.sort_unstable_by_key(|item| (item.ordered.css_order, item.ordered.document_index));
    }

    let placement = place_items(
        &in_flow,
        explicit_columns.tracks.len(),
        explicit_rows.tracks.len(),
        auto_flow,
    );
    // A `grid-auto-*` list is read only for coordinates outside the explicit
    // grid.
    let auto_columns = &style.grid_auto_columns().0;
    let auto_rows = &style.grid_auto_rows().0;
    // Only an `auto-fit` track reads the occupancy, to know whether it
    // collapses.
    let occupied_columns = if explicit_columns.auto_fit.is_some() {
        occupied_tracks(&placement.areas, placement.column_range, |area| area.column)
    } else {
        Vec::new()
    };
    let occupied_rows = if explicit_rows.auto_fit.is_some() {
        occupied_tracks(&placement.areas, placement.row_range, |area| area.row)
    } else {
        Vec::new()
    };
    let (first_column, column_specs) = build_axis_tracks(
        &explicit_columns,
        &auto_columns[..auto_columns.len().min(MAX_MATERIALIZED_TRACKS)],
        placement.column_range,
        &occupied_columns,
    );
    let (first_row, row_specs) = build_axis_tracks(
        &explicit_rows,
        &auto_rows[..auto_rows.len().min(MAX_MATERIALIZED_TRACKS)],
        placement.row_range,
        &occupied_rows,
    );
    let mut items = in_flow
        .into_iter()
        .zip(placement.areas)
        .map(|(item, area)| {
            let key = item.key();
            resolve_grid_item(&tree.style(key.node), key, area, Size::NONE, item_defaults)
        })
        .collect::<Vec<_>>();

    let mut columns = TrackSet {
        first_coordinate: first_column,
        ..TrackSet::default()
    };
    let mut rows = TrackSet {
        first_coordinate: first_row,
        ..TrackSet::default()
    };
    let mut intrinsic_scratch = IntrinsicSizingScratch::default();
    let mut items_fresh = true;
    // The available grid space (css-grid-2 §12.1; see `ContainerPrologue`).
    // A width the container does not have under a definite available width
    // is its fit-content width, from track-sizing runs under the two intrinsic
    // constraints — the run a min- or max-content measurement of this
    // container starts with, whose item probes are the same ones — or the
    // substituted extent of a size-contained inline axis; either is then the
    // definite size the tracks are sized against.
    let mut grid_space = metrics.available_inner;
    let decided_width = fit_content_width.map(|stretch_fit| {
        let content = contained_size.width().unwrap_or_else(|| {
            fit_content_inner_size(stretch_fit, |constraint| {
                run_track_sizing(
                    tree,
                    state,
                    &mut columns,
                    &mut rows,
                    &column_specs,
                    &row_specs,
                    &mut items,
                    initial_percentage_basis,
                    Size::new(constraint, grid_space.height),
                    initial_gap,
                    justify_content,
                    align_content,
                    &mut intrinsic_scratch,
                    core::mem::take(&mut items_fresh),
                );
                columns.used_size()
            })
        });
        clamped_inner_axis(&metrics, Axis::Horizontal, content)
    });
    if let Some(width) = decided_width {
        grid_space.width = AvailableSpace::Definite(width);
    }
    // This run decides the block size, from rows sized at the used column
    // widths. Once both sizes are decided without it, it is skipped: the
    // width came from the runs above, so it is not definite as a percentage
    // basis, and the definite rerun below sizes the tracks at the final size
    // anyway.
    let height_decided = metrics.outer.height.is_some() || contained_size.height().is_some();
    let provisional_ran = decided_width.is_none() || !height_decided;
    if provisional_ran {
        run_track_sizing(
            tree,
            state,
            &mut columns,
            &mut rows,
            &column_specs,
            &row_specs,
            &mut items,
            initial_percentage_basis,
            grid_space,
            initial_gap,
            justify_content,
            align_content,
            &mut intrinsic_scratch,
            core::mem::take(&mut items_fresh),
        );
    }
    let provisional_track_size = Size::new(columns.used_size(), rows.used_size());
    // A decided width is the content width; a contained axis reports the
    // substituted extent instead of the tracks it just sized; an uncontained
    // one reports the tracks.
    let container_track_size = Size::new(
        decided_width
            .or(contained_size.width())
            .unwrap_or(provisional_track_size.width),
        contained_size
            .height()
            .unwrap_or(provisional_track_size.height),
    );
    let outer_size = final_outer_size(&metrics, container_track_size);
    let final_inner = Size::new(
        (outer_size.width - metrics.box_inset.width).max(0.0),
        (outer_size.height - metrics.box_inset.height).max(0.0),
    );

    let final_gap = resolve_gap(
        gap_value,
        Size::new(Some(final_inner.width), Some(final_inner.height)),
    );
    let needs_definite_rerun = !provisional_ran
        || final_gap != initial_gap
        || DefiniteRerun {
            specs: &column_specs,
            tracks: &columns,
            basis: initial_percentage_basis.width,
            available: grid_space.width,
            used: provisional_track_size.width,
            final_inner: final_inner.width,
            inset: metrics.box_inset.width,
            spacing_reads_basis: false,
        }
        .moves_axis(&items, Axis::Horizontal)
        || DefiniteRerun {
            specs: &row_specs,
            tracks: &rows,
            basis: initial_percentage_basis.height,
            available: grid_space.height,
            used: provisional_track_size.height,
            final_inner: final_inner.height,
            inset: metrics.box_inset.height,
            // Column sizing spaces fixed rows by `align-content` against the
            // height basis (`run_track_sizing`'s `distributed_gap`).
            spacing_reads_basis: [
                AlignFlags::SPACE_BETWEEN,
                AlignFlags::SPACE_AROUND,
                AlignFlags::SPACE_EVENLY,
            ]
            .contains(&align_content),
        }
        .moves_axis(&items, Axis::Vertical);
    if needs_definite_rerun {
        run_track_sizing(
            tree,
            state,
            &mut columns,
            &mut rows,
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
            &mut intrinsic_scratch,
            core::mem::take(&mut items_fresh),
        );
    }
    align_tracks(&mut columns, final_inner.width, justify_content);
    align_tracks(&mut rows, final_inner.height, align_content);

    let content_origin = Point::new(
        metrics.border.left + metrics.padding.left,
        metrics.border.top + metrics.padding.top,
    );
    let (mut content_size, baselines) = layout_in_flow_items(
        tree,
        state,
        &mut items,
        &columns,
        &rows,
        final_inner,
        content_origin,
        container_content_independence(input, style_definite).map_or(
            input.goal,
            |content_independent| LayoutGoal::Commit {
                content_independent,
            },
        ),
        input.reads_first_baseline && !commits_layout,
        input.reads_last_baseline && !commits_layout,
        rtl,
        is_scroll_container(style.overflow()),
    );
    if commits_layout {
        for (document_index, child) in hidden.expect("commit keeps hidden grid items") {
            hide_subtree(tree, state, child);
            tree.set_unrounded_layout(
                state,
                child,
                Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
            );
        }
        crate::compute::record_scrollable_containing_block(
            tree,
            state,
            node,
            &style,
            outer_size,
            metrics.border,
            content_size,
        );
        let absolute = absolute.expect("commit keeps out-of-flow grid items");
        // With no out-of-flow box to lay out, the pass adds nothing.
        let absolute_content_size =
            if absolute.is_empty() && !tree.has_hoisted_children(state, node) {
                Size::ZERO
            } else {
                layout_absolute_items(
                    tree,
                    state,
                    node,
                    &absolute,
                    &columns,
                    &rows,
                    explicit_columns.tracks.len(),
                    explicit_rows.tracks.len(),
                    final_inner,
                    outer_size,
                    metrics.padding,
                    metrics.border,
                    rtl,
                    item_defaults,
                )
            };
        content_size = content_size.zip_map(absolute_content_size, f32::max);
    }
    let content_size = own_scrollable_overflow(&style, outer_size, metrics.border, content_size);
    // Layout containment exports no baseline (css-contain-2 §3.2).
    let [first, last] = if layout_contained {
        [None, None]
    } else {
        baselines
    };
    LayoutOutput::new(outer_size, content_size)
        .with_first_baselines(Point::new(None, first))
        .with_last_baselines(Point::new(None, last))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn line(value: i32) -> GridPlacement {
        GridPlacement::Line(value)
    }

    #[test]
    fn absolute_axis_lines_resolve_edges_independently() {
        assert_eq!(
            absolute_axis_lines(Line::new(line(3), line(2)), 4),
            (Some(2), Some(1))
        );
        assert_eq!(
            absolute_axis_lines(Line::new(line(0), line(2)), 4),
            (None, Some(1))
        );
        assert_eq!(
            absolute_axis_lines(Line::new(line(2), line(0)), 4),
            (Some(1), None)
        );
        assert_eq!(
            absolute_axis_lines(Line::new(GridPlacement::Auto, line(0)), 4),
            (None, None)
        );
        assert_eq!(
            absolute_axis_lines(Line::new(GridPlacement::Span(1), line(3)), 4),
            (Some(1), Some(2))
        );
        assert_eq!(
            absolute_axis_lines(Line::new(GridPlacement::Span(2), GridPlacement::Span(2)), 4),
            (None, None)
        );
    }

    #[test]
    fn container_floors_are_resolved() {
        let metrics = ResolvedContainerBox {
            preferred_definite: Size::new(false, false),
            aspect_ratio: None,
            box_sizing: stylo::computed_values::box_sizing::T::ContentBox,
            padding: Edges::ZERO,
            border: Edges::ZERO,
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
