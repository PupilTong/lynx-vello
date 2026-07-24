//! CSS Flexible Box Layout Module Level 1 layout algorithm.

#![allow(clippy::cast_precision_loss)]

use smallvec::SmallVec;
use stylo::computed_values::{box_sizing, direction, flex_direction, flex_wrap};
use stylo::values::computed::{
    FlexBasis, LengthPercentage, MaxSize, PositionProperty, Size as StyleSize,
};
use stylo::values::specified::align::AlignFlags;

use super::compute_absolute_layout;
use super::single_axis::{
    BaseReversals, FlowAxes, flow_end, flow_start, flow_to_physical, measure_child, set_flow_end,
    set_flow_start,
};
use super::util::{
    Axis, ItemGeometry, ItemKey, OrderedItem, ResolvedContainerBox, accumulate_scrollable_overflow,
    box_inset_size, clamp_axis, normalize_content_alignment, normalize_item_alignment,
    own_scrollable_overflow, relative_offset, resolve_container_box, resolve_gap, resolve_gap_axis,
    resolve_insets, resolve_item_geometry, resolve_length_percentage, resolve_style_size,
    sort_and_assign_layout_order, style_size_behaves_auto,
};
use crate::geometry::{Edges, Point, Size};
use crate::style::containment::size_containment;
use crate::style::{Contain, CoreStyle};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
    SizingMode,
};

type Axes = FlowAxes<BaseReversals>;

#[inline]
const fn direction_is_row(direction: flex_direction::T) -> bool {
    matches!(
        direction,
        flex_direction::T::Row | flex_direction::T::RowReverse
    )
}

#[inline]
const fn direction_is_reverse(direction: flex_direction::T) -> bool {
    matches!(
        direction,
        flex_direction::T::RowReverse | flex_direction::T::ColumnReverse
    )
}

fn flex_axes(
    direction: flex_direction::T,
    wrap: flex_wrap::T,
    inline_direction: direction::T,
) -> Axes {
    let is_row = direction_is_row(direction);
    let main = if is_row {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    let rtl = inline_direction == direction::T::Rtl;
    let main_base_reverse = is_row && rtl;
    let main_reverse = main_base_reverse ^ direction_is_reverse(direction);
    let cross_base_reverse = !is_row && rtl;
    let cross_reverse = cross_base_reverse ^ (wrap == flex_wrap::T::WrapReverse);
    FlowAxes {
        main,
        cross: main.other(),
        main_reverse,
        cross_reverse,
        base: BaseReversals {
            main: main_base_reverse,
            cross: cross_base_reverse,
        },
    }
}

/// Transient per-item state accumulated across the Flexbox §9 sizing,
/// flexing, cross-size, and alignment passes. It stores only resolved values
/// and compact hot style fields; the raw sizing properties the base-size
/// pass classifies are re-borrowed from the style view (borrowed accessors
/// make a re-fetch a pointer projection, never a clone).
#[derive(Debug)]
struct FlexItem<N> {
    geometry: ItemGeometry,
    key: ItemKey<N>,
    direction: direction::T,
    position: PositionProperty,
    align_self: AlignFlags,
    inset: Edges<Option<f32>>,
    flex_grow: f32,
    flex_shrink: f32,
    flex_basis: f32,
    inner_flex_basis: f32,
    min_content_contribution: f32,
    max_content_contribution: f32,
    resolved_min_main: f32,
    hypothetical_main: f32,
    target_main: f32,
    hypothetical_cross: f32,
    target_cross: f32,
    baseline: f32,
    measured_baselines: Point<Option<f32>>,
    frozen: bool,
    violation: f32,
    main_position: f32,
    cross_position: f32,
    main_size_is_definite: bool,
}
super::util::impl_item_geometry!(FlexItem);

/// One consecutive range in the order-modified item array plus its resolved
/// cross-axis size and position.
#[derive(Debug, Clone, Copy)]
struct FlexLine {
    start: usize,
    end: usize,
    cross_size: f32,
    cross_position: f32,
}

/// The common single-line case stays inline; wrapping spills as needed.
type FlexLines = SmallVec<[FlexLine; 1]>;

fn alignment_distribution(
    value: AlignFlags,
    free_space: f32,
    count: usize,
    flow_reverse: bool,
    base_reverse: bool,
) -> (f32, f32) {
    if count == 0 {
        return (0.0, 0.0);
    }

    let value = if free_space < 0.0 {
        if value == AlignFlags::SPACE_BETWEEN || value == AlignFlags::STRETCH {
            AlignFlags::FLEX_START
        } else if value == AlignFlags::SPACE_AROUND || value == AlignFlags::SPACE_EVENLY {
            AlignFlags::CENTER
        } else {
            value
        }
    } else {
        value
    };

    if value == AlignFlags::START {
        if flow_reverse == base_reverse {
            (0.0, 0.0)
        } else {
            (free_space, 0.0)
        }
    } else if value == AlignFlags::END {
        if flow_reverse == base_reverse {
            (free_space, 0.0)
        } else {
            (0.0, 0.0)
        }
    } else if value == AlignFlags::FLEX_END {
        (free_space, 0.0)
    } else if value == AlignFlags::CENTER {
        (free_space / 2.0, 0.0)
    } else if value == AlignFlags::SPACE_BETWEEN && count > 1 {
        (0.0, free_space / (count - 1) as f32)
    } else if value == AlignFlags::SPACE_AROUND {
        let between = free_space / count as f32;
        (between / 2.0, between)
    } else if value == AlignFlags::SPACE_EVENLY {
        let between = free_space / (count + 1) as f32;
        (between, between)
    } else {
        (0.0, 0.0)
    }
}

#[inline]
fn item_alignment_offset(
    value: AlignFlags,
    free_space: f32,
    flow_reverse: bool,
    base_reverse: bool,
) -> f32 {
    let (start, end) = if flow_reverse == base_reverse {
        (0.0, free_space)
    } else {
        (free_space, 0.0)
    };
    if value == AlignFlags::START {
        start
    } else if value == AlignFlags::END {
        end
    } else if value == AlignFlags::FLEX_END {
        free_space
    } else if value == AlignFlags::CENTER {
        free_space / 2.0
    } else {
        0.0
    }
}

#[inline]
fn flex_basis_behaves_auto(value: &FlexBasis) -> bool {
    match value {
        FlexBasis::Content => true,
        FlexBasis::Size(size) => style_size_behaves_auto(size),
    }
}

#[inline]
fn resolve_flex_basis(value: &FlexBasis, basis: Option<f32>) -> Option<f32> {
    match value {
        FlexBasis::Content => None,
        FlexBasis::Size(size) => resolve_style_size(size, basis),
    }
}

fn resolve_item<T>(
    tree: &T,
    key: ItemKey<T::NodeId>,
    container_inner_size: Size<Option<f32>>,
    axes: Axes,
    rtl: bool,
    default_alignment: AlignFlags,
) -> FlexItem<T::NodeId>
where
    T: LayoutTree,
{
    let style = tree.style(key.node);
    let flex_grow = style.flex_grow().0;
    let flex_shrink = style.flex_shrink().0;
    debug_assert!(
        flex_grow.is_finite() && flex_grow >= 0.0,
        "flex-grow must be finite and non-negative"
    );
    debug_assert!(
        flex_shrink.is_finite() && flex_shrink >= 0.0,
        "flex-shrink must be finite and non-negative"
    );
    let geometry = resolve_item_geometry(&style, container_inner_size);
    FlexItem {
        geometry,
        key,
        direction: style.direction(),
        position: style.position(),
        inset: resolve_insets(style.inset(), container_inner_size),
        align_self: normalize_item_alignment(
            style.align_self().0,
            axes.cross == Axis::Horizontal,
            rtl,
        )
        .unwrap_or(default_alignment),
        flex_grow,
        flex_shrink,
        flex_basis: 0.0,
        inner_flex_basis: 0.0,
        min_content_contribution: 0.0,
        max_content_contribution: 0.0,
        resolved_min_main: 0.0,
        hypothetical_main: 0.0,
        target_main: 0.0,
        hypothetical_cross: 0.0,
        target_cross: 0.0,
        baseline: 0.0,
        measured_baselines: Point::NONE,
        frozen: false,
        violation: 0.0,
        main_position: 0.0,
        cross_position: 0.0,
        main_size_is_definite: false,
    }
}

/// Lazily memoized main-axis measurements for one flex base-size pass.
///
/// Intrinsic text measurement is intentionally still performed whenever its
/// result participates in sizing; this only avoids probes whose result has no
/// consumer in the current branch of the flex algorithm.
struct MainAxisProbes<'tree, 'state, T>
where
    T: LayoutTree,
{
    tree: &'tree T,
    state: &'state mut T::State,
    node: T::NodeId,
    axes: Axes,
    known_dimensions: Size<Option<f32>>,
    definite_dimensions: Size<bool>,
    parent_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    values: [f32; 3],
    measured: u8,
}

impl<T> MainAxisProbes<'_, '_, T>
where
    T: LayoutTree,
{
    fn measure(&mut self, available_main: AvailableSpace) -> f32 {
        let available_space = self
            .axes
            .main
            .pack(available_main, self.axes.cross.size(self.available_space));
        self.axes.main.size(
            measure_child(
                self.tree,
                self.state,
                self.node,
                self.known_dimensions,
                self.definite_dimensions,
                self.parent_size,
                available_space,
                SizingMode::IgnoreSizeStyles,
                self.axes.main.requested(),
            )
            .size,
        )
    }

    fn probe(&mut self, slot: usize, available_main: AvailableSpace) -> f32 {
        let expected = match slot {
            0 => AvailableSpace::MinContent,
            1 => AvailableSpace::MaxContent,
            2 => self.axes.main.size(self.available_space),
            _ => unreachable!("the main-axis probe cache has exactly three slots"),
        };
        debug_assert_eq!(
            available_main, expected,
            "a main-axis probe slot must not be reused for another constraint"
        );
        let bit = 1_u8 << slot;
        if self.measured & bit != 0 {
            return self.values[slot];
        }
        let value = self.measure(available_main);
        self.values[slot] = value;
        self.measured |= bit;
        value
    }

    fn min_content(&mut self) -> f32 {
        self.probe(0, AvailableSpace::MinContent)
    }

    fn max_content(&mut self) -> f32 {
        self.probe(1, AvailableSpace::MaxContent)
    }

    fn available_content(&mut self) -> f32 {
        let available_main = self.axes.main.size(self.available_space);
        if !available_main.is_definite() {
            return self.max_content();
        }
        self.probe(2, available_main)
    }

    fn fit_content(&mut self, limit: &LengthPercentage, percentage_basis: Option<f32>) -> f32 {
        let min_content = self.min_content();
        let max_content = self.max_content();
        let limit = resolve_length_percentage(limit, percentage_basis).unwrap_or(max_content);
        max_content.min(limit.max(min_content))
    }

    fn resolve_size(&mut self, value: &StyleSize, percentage_basis: Option<f32>) -> Option<f32> {
        match value {
            StyleSize::MinContent => Some(self.min_content()),
            StyleSize::MaxContent => Some(self.max_content()),
            StyleSize::FitContentFunction(limit) => {
                Some(self.fit_content(&limit.0, percentage_basis))
            }
            _ => None,
        }
    }

    fn resolve_max_size(&mut self, value: &MaxSize, percentage_basis: Option<f32>) -> Option<f32> {
        match value {
            MaxSize::MinContent => Some(self.min_content()),
            MaxSize::MaxContent => Some(self.max_content()),
            MaxSize::FitContentFunction(limit) => {
                Some(self.fit_content(&limit.0, percentage_basis))
            }
            _ => None,
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn determine_flex_base_sizes<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [FlexItem<T::NodeId>],
    axes: Axes,
    container_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
    flex_basis_percentage_basis: Option<f32>,
    container_main_is_definite: bool,
    needs_intrinsic_main_contributions: bool,
) where
    T: LayoutTree,
{
    let container_main = axes.main.size(container_inner_size);
    let available_main = axes.main.size(available_space);
    let needs_min_content_contribution =
        needs_intrinsic_main_contributions && available_main == AvailableSpace::MinContent;
    let needs_max_content_contribution =
        needs_intrinsic_main_contributions && available_main == AvailableSpace::MaxContent;

    for item in items {
        let node = item.key.node;
        let style = tree.style(node);
        let raw_size = style.size();
        let raw_min_size = style.min_size();
        let raw_max_size = style.max_size();
        let raw_flex_basis = style.flex_basis();
        let inset_size = box_inset_size(item.padding, item.border);
        let main_floor = axes.main.size(inset_size);
        let cross_preferred = axes.cross.size(item.preferred_size);
        let mut known = Size::NONE;
        axes.cross.set_size(&mut known, cross_preferred);
        let mut known_is_definite = Size::new(false, false);
        axes.cross.set_size(
            &mut known_is_definite,
            axes.cross.size(item.preferred_definite),
        );

        let contribution_parent_size = axes.main.pack(None, axes.cross.size(container_inner_size));
        let mut probes = MainAxisProbes {
            tree,
            state: &mut *state,
            node,
            axes,
            known_dimensions: known,
            definite_dimensions: known_is_definite,
            parent_size: contribution_parent_size,
            available_space,
            values: [0.0; 3],
            measured: 0,
        };

        if axes.main.size(item.preferred_size).is_none()
            && let Some(value) = probes.resolve_size(axes.main.size(raw_size), container_main)
        {
            axes.main.set_size(&mut item.preferred_size, Some(value));
        }
        if axes.main.size(item.min_size).is_none()
            && let Some(value) = probes.resolve_size(axes.main.size(raw_min_size), container_main)
        {
            axes.main.set_size(&mut item.min_size, Some(value));
        }
        if axes.main.size(item.max_size).is_none()
            && let Some(value) =
                probes.resolve_max_size(axes.main.size(raw_max_size), container_main)
        {
            axes.main.set_size(&mut item.max_size, Some(value));
        }

        let flex_basis_is_auto = flex_basis_behaves_auto(raw_flex_basis);
        let resolved_basis =
            resolve_flex_basis(raw_flex_basis, flex_basis_percentage_basis).map(|basis| {
                if item.box_sizing == box_sizing::T::ContentBox {
                    basis + main_floor
                } else {
                    basis
                }
            });

        let preferred_main = axes.main.size(item.preferred_size);
        let preferred_flex_basis = if flex_basis_is_auto {
            preferred_main
        } else {
            None
        };
        let definite_basis = resolved_basis.or(preferred_flex_basis);
        item.main_size_is_definite = container_main_is_definite || definite_basis.is_some();
        item.flex_basis = if let Some(basis) = definite_basis {
            basis
        } else {
            let content_basis: &StyleSize = if flex_basis_is_auto {
                axes.main.size(raw_size)
            } else {
                match raw_flex_basis {
                    FlexBasis::Size(size) => size,
                    FlexBasis::Content => {
                        unreachable!("flex-basis: content behaves as auto and was handled above")
                    }
                }
            };
            match content_basis {
                StyleSize::MinContent => probes.min_content(),
                StyleSize::MaxContent => probes.max_content(),
                StyleSize::FitContentFunction(limit) => {
                    probes.fit_content(&limit.0, flex_basis_percentage_basis)
                }
                StyleSize::LengthPercentage(lp) if lp.0.to_percentage().is_none() => {
                    probes.max_content()
                }
                StyleSize::Auto
                | StyleSize::LengthPercentage(_)
                | StyleSize::FitContent
                | StyleSize::Stretch
                | StyleSize::WebkitFillAvailable => {
                    if available_main == AvailableSpace::MinContent {
                        probes.min_content()
                    } else {
                        probes.available_content()
                    }
                }
                StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
                    unreachable!("anchor sizing is pref-dead under the lynx feature")
                }
            }
        };

        item.inner_flex_basis = item.flex_basis - main_floor;

        let explicit_min = axes.main.size(item.min_size);
        item.resolved_min_main = if let Some(minimum) = explicit_min {
            minimum.max(main_floor)
        } else if item.overflow.x.is_scrollable() || item.overflow.y.is_scrollable() {
            main_floor
        } else {
            let main_is_auto = style_size_behaves_auto(axes.main.size(raw_size));
            let cross_is_auto = style_size_behaves_auto(axes.cross.size(raw_size));
            let specified_suggestion = (!main_is_auto).then_some(preferred_main).flatten();
            let transferred_suggestion = (item.aspect_ratio.is_some() && !cross_is_auto)
                .then_some(preferred_main)
                .flatten();
            let mut content_suggestion = probes.min_content();
            if let Some(transferred) = transferred_suggestion {
                content_suggestion = content_suggestion.max(transferred);
            }
            if let Some(specified) = specified_suggestion {
                content_suggestion = content_suggestion.min(specified);
            }
            content_suggestion =
                content_suggestion.min(axes.main.size(item.max_size).unwrap_or(f32::INFINITY));
            content_suggestion.max(main_floor)
        };

        item.hypothetical_main = clamp_axis(
            item.flex_basis,
            Some(item.resolved_min_main),
            axes.main.size(item.max_size),
            main_floor,
        );
        let margin_main = axes.main.sum(item.margin);
        let preferred_contribution = preferred_main.unwrap_or(0.0);
        let contribution = |content: f32| {
            let definite_basis = (!flex_basis_is_auto).then_some(item.flex_basis);
            let mut value = content
                .max(preferred_contribution)
                .max(definite_basis.unwrap_or(0.0));
            if item.flex_grow == 0.0 {
                value = value.min(item.flex_basis);
            }
            if item.flex_shrink == 0.0 {
                value = value.max(item.flex_basis);
            }
            clamp_axis(
                value,
                Some(item.resolved_min_main),
                axes.main.size(item.max_size),
                main_floor,
            ) + margin_main
        };
        let min_content_contribution = if needs_min_content_contribution {
            contribution(probes.min_content())
        } else {
            0.0
        };
        let max_content_contribution = if needs_max_content_contribution {
            contribution(probes.max_content())
        } else {
            0.0
        };
        item.min_content_contribution = min_content_contribution;
        item.max_content_contribution = max_content_contribution;
        item.target_main = item.hypothetical_main;
    }
}

#[inline]
fn item_outer_hypothetical_main<N>(item: &FlexItem<N>, axes: Axes) -> f32 {
    item.hypothetical_main + axes.main.sum(item.margin)
}

#[inline]
fn item_outer_target_main<N>(item: &FlexItem<N>, axes: Axes) -> f32 {
    item.target_main + axes.main.sum(item.margin)
}

fn collect_flex_lines<N>(
    items: &[FlexItem<N>],
    wrap: flex_wrap::T,
    available_main: AvailableSpace,
    gap: f32,
    axes: Axes,
) -> FlexLines {
    if wrap == flex_wrap::T::Nowrap || available_main == AvailableSpace::MaxContent {
        return SmallVec::from_buf([FlexLine {
            start: 0,
            end: items.len(),
            cross_size: 0.0,
            cross_position: 0.0,
        }]);
    }
    if items.is_empty() {
        return SmallVec::new();
    }
    if available_main == AvailableSpace::MinContent {
        return (0..items.len())
            .map(|start| FlexLine {
                start,
                end: start + 1,
                cross_size: 0.0,
                cross_position: 0.0,
            })
            .collect();
    }

    let AvailableSpace::Definite(limit) = available_main else {
        unreachable!("intrinsic available-space variants handled above")
    };
    let mut lines = SmallVec::new();
    let mut start = 0;
    while start < items.len() {
        let mut end = start;
        let mut occupied = 0.0;
        let mut prior_participants = 0;
        while end < items.len() {
            let item_size = item_outer_hypothetical_main(&items[end], axes);
            let candidate_gap = if prior_participants == 0 { 0.0 } else { gap };
            let candidate = occupied + candidate_gap + item_size;
            if prior_participants > 0
                && candidate > limit
                && !(item_size == 0.0 && candidate_gap == 0.0)
            {
                break;
            }
            occupied = candidate;
            prior_participants += 1;
            end += 1;
        }
        lines.push(FlexLine {
            start,
            end,
            cross_size: 0.0,
            cross_position: 0.0,
        });
        start = end;
    }
    lines
}

fn line_intrinsic_main<N>(items: &[FlexItem<N>], line: FlexLine, gap: f32, axes: Axes) -> f32 {
    let line_items = &items[line.start..line.end];
    let item_sum = line_items
        .iter()
        .map(|item| item.flex_basis.max(item.resolved_min_main) + axes.main.sum(item.margin))
        .sum::<f32>();
    item_sum + gap * line_items.len().saturating_sub(1) as f32
}

fn line_content_contribution<N>(
    items: &[FlexItem<N>],
    line: FlexLine,
    gap: f32,
    maximum: bool,
) -> f32 {
    let line_items = &items[line.start..line.end];
    let item_sum = line_items
        .iter()
        .map(|item| {
            if maximum {
                item.max_content_contribution
            } else {
                item.min_content_contribution
            }
        })
        .sum::<f32>();
    item_sum + gap * line_items.len().saturating_sub(1) as f32
}

#[allow(clippy::too_many_arguments)]
fn determine_auto_main_size<N>(
    items: &[FlexItem<N>],
    lines: &[FlexLine],
    gap: f32,
    axes: Axes,
    available_main: AvailableSpace,
    inset_main: f32,
    min_outer: Option<f32>,
    max_outer: Option<f32>,
) -> f32 {
    let content = match available_main {
        AvailableSpace::MaxContent => lines
            .iter()
            .copied()
            .map(|line| line_content_contribution(items, line, gap, true))
            .max_by(f32::total_cmp)
            .unwrap_or(0.0),
        AvailableSpace::MinContent => lines
            .iter()
            .copied()
            .map(|line| line_content_contribution(items, line, gap, false))
            .max_by(f32::total_cmp)
            .unwrap_or(0.0),
        AvailableSpace::Definite(_) => lines
            .iter()
            .copied()
            .map(|line| line_intrinsic_main(items, line, gap, axes))
            .max_by(f32::total_cmp)
            .unwrap_or(0.0),
    };
    clamp_axis(content + inset_main, min_outer, max_outer, inset_main)
}

#[allow(clippy::too_many_lines)]
fn resolve_flexible_lengths<N>(
    items: &mut [FlexItem<N>],
    line: FlexLine,
    inner_main_size: f32,
    gap: f32,
    axes: Axes,
) {
    let line_items = &mut items[line.start..line.end];
    if line_items.is_empty() {
        return;
    }
    let total_gap = gap * line_items.len().saturating_sub(1) as f32;
    let hypothetical_sum = total_gap
        + line_items
            .iter()
            .map(|item| item_outer_hypothetical_main(item, axes))
            .sum::<f32>();
    let growing = hypothetical_sum < inner_main_size;
    let initial_delta = inner_main_size - hypothetical_sum;

    for item in line_items.iter_mut() {
        item.frozen = false;
        item.violation = 0.0;
        item.target_main = item.flex_basis;
        let factor_is_zero = if growing {
            item.flex_grow == 0.0
        } else {
            item.flex_shrink == 0.0
        };
        let clamp_requires_freeze = if growing {
            item.flex_basis > item.hypothetical_main
        } else {
            item.flex_basis < item.hypothetical_main
        };
        if initial_delta.abs() <= f32::EPSILON || factor_is_zero || clamp_requires_freeze {
            item.target_main = item.hypothetical_main;
            item.frozen = true;
        }
    }

    let initial_used = total_gap
        + line_items
            .iter()
            .map(|item| {
                let main = if item.frozen {
                    item.target_main
                } else {
                    item.flex_basis
                };
                main + axes.main.sum(item.margin)
            })
            .sum::<f32>();
    let initial_free_space = inner_main_size - initial_used;

    for _ in 0..=line_items.len() {
        if line_items.iter().all(|item| item.frozen) {
            return;
        }

        let used = total_gap
            + line_items
                .iter()
                .map(|item| {
                    let main = if item.frozen {
                        item.target_main
                    } else {
                        item.flex_basis
                    };
                    main + axes.main.sum(item.margin)
                })
                .sum::<f32>();
        let mut remaining = inner_main_size - used;
        let factor_sum = line_items
            .iter()
            .filter(|item| !item.frozen)
            .map(|item| {
                if growing {
                    item.flex_grow
                } else {
                    item.flex_shrink
                }
            })
            .sum::<f32>();
        if factor_sum < 1.0 {
            let scaled = initial_free_space * factor_sum;
            if scaled.abs() < remaining.abs() {
                remaining = scaled;
            }
        }

        if growing {
            if factor_sum > 0.0 {
                for item in line_items.iter_mut().filter(|item| !item.frozen) {
                    item.target_main = item.flex_basis + remaining * item.flex_grow / factor_sum;
                }
            }
        } else {
            let scaled_sum = line_items
                .iter()
                .filter(|item| !item.frozen)
                .map(|item| item.flex_shrink * item.inner_flex_basis)
                .sum::<f32>();
            if scaled_sum > 0.0 {
                for item in line_items.iter_mut().filter(|item| !item.frozen) {
                    let scaled = item.flex_shrink * item.inner_flex_basis;
                    item.target_main = item.flex_basis + remaining * scaled / scaled_sum;
                }
            }
        }

        let mut total_violation = 0.0;
        for item in line_items.iter_mut().filter(|item| !item.frozen) {
            let floor = axes.main.size(box_inset_size(item.padding, item.border));
            let unclamped = item.target_main;
            item.target_main = clamp_axis(
                unclamped,
                Some(item.resolved_min_main),
                axes.main.size(item.max_size),
                floor,
            );
            item.violation = item.target_main - unclamped;
            total_violation += item.violation;
        }

        let mut froze_any = false;
        for item in line_items.iter_mut().filter(|item| !item.frozen) {
            let freeze = if total_violation > f32::EPSILON {
                item.violation > 0.0
            } else if total_violation < -f32::EPSILON {
                item.violation < 0.0
            } else {
                true
            };
            if freeze {
                item.frozen = true;
                froze_any = true;
            }
        }
        if !froze_any {
            for item in line_items.iter_mut() {
                item.frozen = true;
            }
            return;
        }
    }

    debug_assert!(false, "flex freeze loop exceeded the item-count bound");
}

#[inline]
fn is_first_baseline_candidate<N>(item: &FlexItem<N>, axes: Axes) -> bool {
    axes.main == Axis::Vertical || item.align_self == AlignFlags::BASELINE
}

#[allow(clippy::too_many_arguments)]
fn determine_hypothetical_cross_sizes<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [FlexItem<T::NodeId>],
    lines: &[FlexLine],
    axes: Axes,
    wrap: flex_wrap::T,
    container_inner_size: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) where
    T: LayoutTree,
{
    let exported_baseline_item = lines.first().and_then(|line| {
        let line_items = &items[line.start..line.end];
        line_items
            .iter()
            .position(|item| is_first_baseline_candidate(item, axes))
            .map(|offset| line.start + offset)
            .or_else(|| (!line_items.is_empty()).then_some(line.start))
    });
    let nowrap_cross = (wrap == flex_wrap::T::Nowrap)
        .then(|| axes.cross.size(container_inner_size))
        .flatten();

    for line in lines {
        for (offset, item) in items[line.start..line.end].iter_mut().enumerate() {
            let item_index = line.start + offset;
            let inset_size = box_inset_size(item.padding, item.border);
            let cross_floor = axes.cross.size(inset_size);
            let min_cross = axes.cross.size(item.min_size);
            let max_cross = axes.cross.size(item.max_size);
            let cross_start_auto = item.margin_auto.flow_start(axes.cross, axes.cross_reverse);
            let cross_end_auto = item.margin_auto.flow_end(axes.cross, axes.cross_reverse);
            let participates_in_baseline_alignment = axes.main == Axis::Horizontal
                && item.align_self == AlignFlags::BASELINE
                && !cross_start_auto
                && !cross_end_auto;
            let baseline_is_consumed =
                exported_baseline_item == Some(item_index) || participates_in_baseline_alignment;
            let stretched_cross = nowrap_cross.filter(|_| {
                item.align_self == AlignFlags::STRETCH
                    && axes.cross.size(item.size_is_auto)
                    && !cross_start_auto
                    && !cross_end_auto
            });
            let resolved_cross = axes
                .cross
                .size(item.preferred_size)
                .map(|preferred| clamp_axis(preferred, min_cross, max_cross, cross_floor))
                .or_else(|| {
                    stretched_cross.map(|line_cross| {
                        clamp_axis(
                            line_cross - axes.cross.sum(item.margin),
                            min_cross,
                            max_cross,
                            cross_floor,
                        )
                    })
                });
            let cross_has_intrinsic_style = item.intrinsic.preferred(axes.cross).is_intrinsic()
                || item.intrinsic.minimum(axes.cross).is_intrinsic()
                || item.intrinsic.maximum(axes.cross).is_intrinsic();

            if !baseline_is_consumed
                && !cross_has_intrinsic_style
                && let Some(cross) = resolved_cross
            {
                item.hypothetical_cross = cross;
                item.target_cross = cross;
                item.measured_baselines = Point::NONE;
                item.baseline = cross;
                continue;
            }

            let mut known = Size::NONE;
            axes.main.set_size(&mut known, Some(item.target_main));
            axes.cross
                .set_size(&mut known, axes.cross.size(item.preferred_size));
            let mut known_is_definite = item.preferred_definite;
            axes.main
                .set_size(&mut known_is_definite, item.main_size_is_definite);
            let child_available = axes.main.pack(
                AvailableSpace::Definite(item.target_main),
                axes.cross.size(available_space),
            );
            let output = measure_child(
                tree,
                state,
                item.key.node,
                known,
                known_is_definite,
                container_inner_size,
                child_available,
                SizingMode::ApplySizeStyles,
                RequestedAxis::Both,
            );
            item.hypothetical_cross = clamp_axis(
                axes.cross.size(output.size),
                min_cross,
                max_cross,
                cross_floor,
            );
            item.target_cross = item.hypothetical_cross;
            item.measured_baselines = output.first_baselines;
            item.baseline = if axes.main == Axis::Horizontal {
                output.first_baselines.y.unwrap_or(output.size.height)
            } else {
                output.first_baselines.x.unwrap_or(output.size.width)
            };
        }
    }
}

fn calculate_line_cross_sizes<N>(
    items: &[FlexItem<N>],
    lines: &mut [FlexLine],
    axes: Axes,
    wrap: flex_wrap::T,
    known_inner_cross: Option<f32>,
) {
    if wrap == flex_wrap::T::Nowrap
        && let (Some(line), Some(cross_size)) = (lines.first_mut(), known_inner_cross)
    {
        line.cross_size = cross_size.max(0.0);
        return;
    }

    for line in lines {
        let mut largest_outer = 0.0_f32;
        let mut largest_before_baseline = 0.0_f32;
        let mut largest_after_baseline = 0.0_f32;
        let mut has_baseline_item = false;
        for item in &items[line.start..line.end] {
            let outer_cross = item.hypothetical_cross + axes.cross.sum(item.margin);
            if axes.main == Axis::Horizontal
                && item.align_self == AlignFlags::BASELINE
                && !item.margin_auto.flow_start(axes.cross, axes.cross_reverse)
                && !item.margin_auto.flow_end(axes.cross, axes.cross_reverse)
            {
                let before = item.margin.top + item.baseline;
                let after = outer_cross - before;
                largest_before_baseline = largest_before_baseline.max(before);
                largest_after_baseline = largest_after_baseline.max(after);
                has_baseline_item = true;
            } else {
                largest_outer = largest_outer.max(outer_cross);
            }
        }
        let baseline_outer = if has_baseline_item {
            largest_before_baseline + largest_after_baseline
        } else {
            0.0
        };
        line.cross_size = largest_outer.max(baseline_outer).max(0.0);
    }
}

fn determine_auto_cross_size(
    lines: &[FlexLine],
    cross_gap: f32,
    inset_cross: f32,
    min_outer: Option<f32>,
    max_outer: Option<f32>,
    axes: Axes,
    cross_constraint: AvailableSpace,
) -> f32 {
    let lines_size =
        if axes.main == Axis::Vertical && cross_constraint == AvailableSpace::MinContent {
            lines
                .iter()
                .map(|line| line.cross_size)
                .max_by(f32::total_cmp)
                .unwrap_or(0.0)
        } else {
            lines.iter().map(|line| line.cross_size).sum::<f32>()
                + cross_gap * lines.len().saturating_sub(1) as f32
        };
    clamp_axis(lines_size + inset_cross, min_outer, max_outer, inset_cross)
}

fn stretch_lines(
    lines: &mut [FlexLine],
    wrap: flex_wrap::T,
    align_content: AlignFlags,
    inner_cross: f32,
    cross_gap: f32,
) {
    if wrap == flex_wrap::T::Nowrap || align_content != AlignFlags::STRETCH || lines.is_empty() {
        return;
    }
    let used = lines.iter().map(|line| line.cross_size).sum::<f32>()
        + cross_gap * lines.len().saturating_sub(1) as f32;
    if used < inner_cross {
        let addition = (inner_cross - used) / lines.len() as f32;
        for line in lines {
            line.cross_size += addition;
        }
    }
}

fn determine_used_cross_sizes<N>(items: &mut [FlexItem<N>], lines: &[FlexLine], axes: Axes) {
    for line in lines {
        for item in &mut items[line.start..line.end] {
            let inset_size = box_inset_size(item.padding, item.border);
            let cross_floor = axes.cross.size(inset_size);
            let should_stretch = item.align_self == AlignFlags::STRETCH
                && axes.cross.size(item.size_is_auto)
                && !item.margin_auto.flow_start(axes.cross, axes.cross_reverse)
                && !item.margin_auto.flow_end(axes.cross, axes.cross_reverse);
            item.target_cross = if should_stretch {
                clamp_axis(
                    line.cross_size - axes.cross.sum(item.margin),
                    axes.cross.size(item.min_size),
                    axes.cross.size(item.max_size),
                    cross_floor,
                )
            } else {
                item.hypothetical_cross
            };
        }
    }
}

fn distribute_main_axis<N>(
    items: &mut [FlexItem<N>],
    lines: &[FlexLine],
    axes: Axes,
    inner_main: f32,
    main_gap: f32,
    justify_content: AlignFlags,
) {
    for line in lines {
        let line_items = &mut items[line.start..line.end];
        let participant_count = line_items.len();
        let fixed_gap = main_gap * participant_count.saturating_sub(1) as f32;
        let used = fixed_gap
            + line_items
                .iter()
                .map(|item| item_outer_target_main(item, axes))
                .sum::<f32>();
        let free_space = inner_main - used;
        let auto_count = line_items
            .iter()
            .map(|item| {
                usize::from(item.margin_auto.flow_start(axes.main, axes.main_reverse))
                    + usize::from(item.margin_auto.flow_end(axes.main, axes.main_reverse))
            })
            .sum::<usize>();

        let (leading, distributed_gap) = if free_space > 0.0 && auto_count > 0 {
            let share = free_space / auto_count as f32;
            for item in line_items.iter_mut() {
                if item.margin_auto.flow_start(axes.main, axes.main_reverse) {
                    set_flow_start(&mut item.margin, axes.main, axes.main_reverse, share);
                }
                if item.margin_auto.flow_end(axes.main, axes.main_reverse) {
                    set_flow_end(&mut item.margin, axes.main, axes.main_reverse, share);
                }
            }
            (0.0, 0.0)
        } else {
            for item in line_items.iter_mut() {
                if item.margin_auto.flow_start(axes.main, axes.main_reverse) {
                    set_flow_start(&mut item.margin, axes.main, axes.main_reverse, 0.0);
                }
                if item.margin_auto.flow_end(axes.main, axes.main_reverse) {
                    set_flow_end(&mut item.margin, axes.main, axes.main_reverse, 0.0);
                }
            }
            alignment_distribution(
                justify_content,
                free_space,
                participant_count,
                axes.main_reverse,
                axes.base.main,
            )
        };

        let mut cursor = leading;
        let mut participant_index = 0;
        for item in line_items.iter_mut() {
            cursor += flow_start(item.margin, axes.main, axes.main_reverse);
            item.main_position = cursor;
            cursor += item.target_main + flow_end(item.margin, axes.main, axes.main_reverse);
            participant_index += 1;
            if participant_index < participant_count {
                cursor += main_gap + distributed_gap;
            }
        }
    }
}

fn align_lines(
    lines: &mut [FlexLine],
    axes: Axes,
    wrap: flex_wrap::T,
    align_content: AlignFlags,
    inner_cross: f32,
    cross_gap: f32,
) {
    let used = lines.iter().map(|line| line.cross_size).sum::<f32>()
        + cross_gap * lines.len().saturating_sub(1) as f32;
    let free_space = inner_cross - used;
    let effective_alignment = if wrap == flex_wrap::T::Nowrap {
        AlignFlags::FLEX_START
    } else {
        align_content
    };
    let (leading, distributed_gap) = alignment_distribution(
        effective_alignment,
        free_space,
        lines.len(),
        axes.cross_reverse,
        axes.base.cross,
    );
    let mut cursor = leading;
    let line_count = lines.len();
    for (index, line) in lines.iter_mut().enumerate() {
        line.cross_position = cursor;
        cursor += line.cross_size;
        if index + 1 < line_count {
            cursor += cross_gap + distributed_gap;
        }
    }
}

fn align_items_cross_axis<N>(items: &mut [FlexItem<N>], lines: &[FlexLine], axes: Axes) {
    for line in lines {
        let max_physical_baseline = if axes.main == Axis::Horizontal {
            items[line.start..line.end]
                .iter()
                .filter(|item| {
                    item.align_self == AlignFlags::BASELINE
                        && !item.margin_auto.flow_start(axes.cross, axes.cross_reverse)
                        && !item.margin_auto.flow_end(axes.cross, axes.cross_reverse)
                })
                .map(|item| item.margin.top + item.baseline)
                .fold(0.0_f32, f32::max)
        } else {
            0.0
        };

        for item in &mut items[line.start..line.end] {
            let start_auto = item.margin_auto.flow_start(axes.cross, axes.cross_reverse);
            let end_auto = item.margin_auto.flow_end(axes.cross, axes.cross_reverse);
            let free = line.cross_size - item.target_cross - axes.cross.sum(item.margin);
            if start_auto || end_auto {
                if free >= 0.0 {
                    let count = usize::from(start_auto) + usize::from(end_auto);
                    let share = free / count as f32;
                    if start_auto {
                        set_flow_start(&mut item.margin, axes.cross, axes.cross_reverse, share);
                    }
                    if end_auto {
                        set_flow_end(&mut item.margin, axes.cross, axes.cross_reverse, share);
                    }
                } else {
                    let logical_start_auto =
                        item.margin_auto.flow_start(axes.cross, axes.base.cross);
                    let logical_end_auto = item.margin_auto.flow_end(axes.cross, axes.base.cross);
                    if logical_start_auto {
                        set_flow_start(&mut item.margin, axes.cross, axes.base.cross, 0.0);
                        if logical_end_auto {
                            set_flow_end(&mut item.margin, axes.cross, axes.base.cross, free);
                        }
                    } else if logical_end_auto {
                        set_flow_end(&mut item.margin, axes.cross, axes.base.cross, free);
                    }
                }
                let physical_position = axes.cross.start(item.margin);
                item.cross_position = if axes.cross_reverse {
                    line.cross_size - physical_position - item.target_cross
                } else {
                    physical_position
                };
                continue;
            }

            if item.align_self == AlignFlags::BASELINE && axes.main == Axis::Horizontal {
                let physical_top = max_physical_baseline - item.baseline;
                item.cross_position = if axes.cross_reverse {
                    line.cross_size - physical_top - item.target_cross
                } else {
                    physical_top
                };
                continue;
            }

            let alignment_offset =
                item_alignment_offset(item.align_self, free, axes.cross_reverse, axes.base.cross);
            item.cross_position =
                alignment_offset + flow_start(item.margin, axes.cross, axes.cross_reverse);
        }
    }
}

fn item_border_box_location<N>(
    item: &FlexItem<N>,
    line: FlexLine,
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Point<f32> {
    let main = flow_to_physical(
        item.main_position,
        item.target_main,
        axes.main.size(inner_size),
        axes.main_reverse,
    ) + axes.main.point(content_origin);
    let cross_flow = line.cross_position + item.cross_position;
    let cross = flow_to_physical(
        cross_flow,
        item.target_cross,
        axes.cross.size(inner_size),
        axes.cross_reverse,
    ) + axes.cross.point(content_origin);
    let mut point = Point::ZERO;
    axes.main.set_point(&mut point, main);
    axes.cross.set_point(&mut point, cross);
    point
}

fn first_container_baseline<N>(
    items: &[FlexItem<N>],
    lines: &[FlexLine],
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
) -> Option<f32> {
    let line = *lines.first()?;
    let first = items[line.start..line.end]
        .iter()
        .find(|item| is_first_baseline_candidate(item, axes))
        .or_else(|| items[line.start..line.end].first())?;
    let location = item_border_box_location(first, line, axes, inner_size, content_origin);
    Some(
        location.y
            + first
                .measured_baselines
                .y
                .unwrap_or_else(|| axes.main.pack(first.target_main, first.target_cross).height),
    )
}

#[allow(clippy::too_many_arguments)]
fn perform_in_flow_layout<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [FlexItem<T::NodeId>],
    lines: &[FlexLine],
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    container_size: Size<f32>,
) -> (Size<f32>, Option<f32>)
where
    T: LayoutTree,
{
    let parent_size = inner_size.map(Some);
    let mut content_size = container_size;
    let mut first_baseline = None;

    for line in lines {
        for item in &mut items[line.start..line.end] {
            let target_size = axes.main.pack(item.target_main, item.target_cross);
            let mut input = LayoutInput::commit(
                target_size.map(Some),
                parent_size,
                target_size.map(AvailableSpace::Definite),
            );
            axes.main
                .set_size(&mut input.definite_dimensions, item.main_size_is_definite);
            input.sizing_mode = SizingMode::IgnoreSizeStyles;
            let output = tree.compute_layout(state, item.key.node, input);

            let offset = if item.position == PositionProperty::Relative {
                relative_offset(item.inset, item.direction)
            } else {
                Point::ZERO
            };
            let mut location =
                item_border_box_location(item, *line, axes, inner_size, content_origin);
            location.x += offset.x;
            location.y += offset.y;

            let mut layout = Layout::with_order(item.key.layout_order);
            layout.location = location;
            layout.size = output.size;
            layout.content_size = output.content_size;
            layout.border = item.border;
            layout.padding = item.padding;
            layout.margin = item.margin;
            tree.set_unrounded_layout(state, item.key.node, layout);

            accumulate_scrollable_overflow(
                &mut content_size,
                location,
                output.size,
                output.content_size,
                item.overflow,
            );

            if first_baseline.is_none()
                && (axes.main == Axis::Vertical || item.align_self == AlignFlags::BASELINE)
            {
                first_baseline =
                    Some(location.y + output.first_baselines.y.unwrap_or(output.size.height));
            }
        }
    }

    if first_baseline.is_none() {
        first_baseline = first_container_baseline(items, lines, axes, inner_size, content_origin);
    }
    (content_size, first_baseline)
}

fn static_position_for_absolute<N>(
    item: &FlexItem<N>,
    axes: Axes,
    inner_size: Size<f32>,
    content_origin: Point<f32>,
    justify_content: AlignFlags,
) -> Point<f32> {
    let free_main = axes.main.size(inner_size) - item.target_main - axes.main.sum(item.margin);
    let (leading_main, _) = alignment_distribution(
        justify_content,
        free_main,
        1,
        axes.main_reverse,
        axes.base.main,
    );
    let main_flow = leading_main + flow_start(item.margin, axes.main, axes.main_reverse);

    let free_cross = axes.cross.size(inner_size) - item.target_cross - axes.cross.sum(item.margin);
    let cross_alignment = item_alignment_offset(
        item.align_self,
        free_cross,
        axes.cross_reverse,
        axes.base.cross,
    );
    let cross_flow = cross_alignment + flow_start(item.margin, axes.cross, axes.cross_reverse);

    let main_border = flow_to_physical(
        main_flow,
        item.target_main,
        axes.main.size(inner_size),
        axes.main_reverse,
    ) + axes.main.point(content_origin);
    let cross_border = flow_to_physical(
        cross_flow,
        item.target_cross,
        axes.cross.size(inner_size),
        axes.cross_reverse,
    ) + axes.cross.point(content_origin);
    let mut border_origin = Point::ZERO;
    axes.main.set_point(&mut border_origin, main_border);
    axes.cross.set_point(&mut border_origin, cross_border);

    Point::new(
        border_origin.x - item.margin.left,
        border_origin.y - item.margin.top,
    )
}

#[allow(clippy::too_many_arguments)]
fn perform_absolute_children<T>(
    tree: &T,
    state: &mut T::State,
    absolute_items: &[OrderedItem<T::NodeId>],
    axes: Axes,
    rtl: bool,
    inner_size: Size<f32>,
    container_size: Size<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
    justify_content: AlignFlags,
    default_alignment: AlignFlags,
) -> Size<f32>
where
    T: LayoutTree,
{
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let parent_size = inner_size.map(Some);
    let mut content_size = container_size;
    let padding_box_size = Size::new(
        (container_size.width - border.horizontal_sum()).max(0.0),
        (container_size.height - border.vertical_sum()).max(0.0),
    );

    for pending in absolute_items {
        let key = pending.key();
        let style = tree.style(key.node);
        let mut item = resolve_item(tree, key, parent_size, axes, rtl, default_alignment);
        let mut known = item.preferred_size;
        let available = inner_size.map(AvailableSpace::Definite);
        let output = measure_child(
            tree,
            state,
            key.node,
            known,
            item.preferred_definite,
            parent_size,
            available,
            SizingMode::ApplySizeStyles,
            RequestedAxis::Both,
        );
        let inset_size = box_inset_size(item.padding, item.border);
        known.width = Some(clamp_axis(
            output.size.width,
            item.min_size.width,
            item.max_size.width,
            inset_size.width,
        ));
        known.height = Some(clamp_axis(
            output.size.height,
            item.min_size.height,
            item.max_size.height,
            inset_size.height,
        ));
        item.target_main = axes.main.size(known).unwrap_or(0.0);
        item.target_cross = axes.cross.size(known).unwrap_or(0.0);
        let static_position =
            static_position_for_absolute(&item, axes, inner_size, content_origin, justify_content);

        match style.position() {
            PositionProperty::Absolute => {
                let static_in_padding_space = Point::new(
                    static_position.x - border.left,
                    static_position.y - border.top,
                );
                let mut layout = compute_absolute_layout(
                    tree,
                    state,
                    key.node,
                    padding_box_size,
                    static_in_padding_space,
                );
                layout.order = key.layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                accumulate_scrollable_overflow(
                    &mut content_size,
                    layout.location,
                    layout.size,
                    layout.content_size,
                    item.overflow,
                );
                tree.set_unrounded_layout(state, key.node, layout);
            }
            PositionProperty::Fixed => {
                tree.set_static_position(state, key.node, static_position);
            }
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {}
        }
    }
    content_size
}

struct CollectedFlexItems<N> {
    generated: Vec<OrderedItem<N>>,
    absolute_items: SmallVec<[OrderedItem<N>; 1]>,
    hidden: SmallVec<[(usize, N); 1]>,
}

fn collect_flex_items<T>(
    tree: &T,
    node: T::NodeId,
    goal: LayoutGoal,
) -> CollectedFlexItems<T::NodeId>
where
    T: LayoutTree,
{
    let commits_layout = goal == LayoutGoal::Commit;
    let children = tree.children(node);
    let (lower, upper) = children.size_hint();
    let child_capacity = match upper {
        Some(exact) if exact == lower => exact,
        _ => lower,
    };
    let mut generated = Vec::with_capacity(child_capacity);
    let mut absolute_items = SmallVec::new();
    let mut hidden = SmallVec::new();

    for (document_index, child) in children.enumerate() {
        let child_style = tree.style(child);
        if child_style.display().is_none() {
            if commits_layout {
                hidden.push((document_index, child));
            }
            continue;
        }
        let pending = OrderedItem {
            node: child,
            document_index,
            css_order: child_style.order(),
            layout_order: if commits_layout {
                u32::try_from(document_index).unwrap_or(u32::MAX)
            } else {
                0
            },
        };
        if matches!(
            child_style.position(),
            PositionProperty::Absolute | PositionProperty::Fixed
        ) {
            if commits_layout {
                absolute_items.push(pending);
            }
        } else {
            generated.push(pending);
        }
    }

    if commits_layout {
        sort_and_assign_layout_order(&mut generated, &mut absolute_items);
    } else if generated.iter().any(|item| item.css_order != 0) {
        generated.sort_unstable_by_key(|item| (item.css_order, item.document_index));
    }

    CollectedFlexItems {
        generated,
        absolute_items,
        hidden,
    }
}

#[allow(clippy::too_many_lines)]
pub fn compute_flexbox_layout<T>(
    tree: &T,
    state: &mut T::State,
    node: T::NodeId,
    input: LayoutInput,
) -> LayoutOutput
where
    T: LayoutTree,
{
    let style = tree.style(node);
    let size_containment = size_containment(&style);
    let layout_contained = style.containment().contains(Contain::LAYOUT);
    let flex_wrap = style.flex_wrap();
    let axes = flex_axes(style.flex_direction(), flex_wrap, style.direction());
    let rtl = style.direction() == direction::T::Rtl;
    let main_horizontal = axes.main == Axis::Horizontal;
    let cross_horizontal = axes.cross == Axis::Horizontal;
    let align_content =
        normalize_content_alignment(style.align_content().primary(), cross_horizontal, rtl)
            .unwrap_or(AlignFlags::STRETCH);
    let align_items = normalize_item_alignment(style.align_items().0, cross_horizontal, rtl)
        .unwrap_or(AlignFlags::STRETCH);
    let justify_content =
        normalize_content_alignment(style.justify_content().primary(), main_horizontal, rtl)
            .unwrap_or(AlignFlags::FLEX_START);
    let ResolvedContainerBox {
        preferred_definite: style_definite,
        padding,
        border,
        box_inset: container_inset_size,
        min: min_size,
        max: max_size,
        outer: mut outer_size,
        inner: mut inner_size,
        available_inner: inner_available_space,
        ..
    } = resolve_container_box(&style, input);
    let outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );
    let item_inline_basis_was_indefinite = !outer_definite.width;
    let main_percentage_basis_was_indefinite = !axes.main.size(outer_definite);
    let gap_value = style.gap();
    let mut gap = resolve_gap(gap_value, inner_size);
    let CollectedFlexItems {
        generated,
        absolute_items,
        hidden,
    } = collect_flex_items(tree, node, input.goal);
    let mut items = generated
        .into_iter()
        .map(|item| {
            let mut percentage_basis = inner_size;
            if !outer_definite.width {
                percentage_basis.width = None;
            }
            if !outer_definite.height {
                percentage_basis.height = None;
            }
            resolve_item(tree, item.key(), percentage_basis, axes, rtl, align_items)
        })
        .collect::<Vec<_>>();
    determine_flex_base_sizes(
        tree,
        state,
        &mut items,
        axes,
        inner_size,
        inner_available_space,
        (!main_percentage_basis_was_indefinite)
            .then(|| axes.main.size(inner_size))
            .flatten(),
        !main_percentage_basis_was_indefinite,
        axes.main.size(outer_size).is_none() && size_containment.is_none(),
    );

    let main_gap = axes.main.size(gap);
    let line_available_main = axes.main.size(inner_size).map_or_else(
        || axes.main.size(inner_available_space),
        AvailableSpace::Definite,
    );
    let mut lines = collect_flex_lines(&items, flex_wrap, line_available_main, main_gap, axes);

    let inset_main = axes.main.size(container_inset_size);
    if axes.main.size(outer_size).is_none() {
        let outer_main = if let Some(intrinsic) = size_containment {
            clamp_axis(
                axes.main.size(intrinsic).unwrap_or(0.0) + inset_main,
                axes.main.size(min_size),
                axes.main.size(max_size),
                inset_main,
            )
        } else {
            determine_auto_main_size(
                &items,
                &lines,
                main_gap,
                axes,
                line_available_main,
                inset_main,
                axes.main.size(min_size),
                axes.main.size(max_size),
            )
        };
        axes.main.set_size(&mut outer_size, Some(outer_main));
        axes.main
            .set_size(&mut inner_size, Some((outer_main - inset_main).max(0.0)));
        let resolved_main_gap =
            resolve_gap_axis(axes.main.size(gap_value), axes.main.size(inner_size));
        axes.main.set_size(&mut gap, resolved_main_gap);
    }
    let inner_main = axes.main.size(inner_size).unwrap_or(0.0);
    let mut main_gap = axes.main.size(gap);
    for line in lines.iter().copied() {
        resolve_flexible_lengths(&mut items, line, inner_main, main_gap, axes);
    }

    determine_hypothetical_cross_sizes(
        tree,
        state,
        &mut items,
        &lines,
        axes,
        flex_wrap,
        inner_size,
        inner_available_space,
    );
    calculate_line_cross_sizes(
        &items,
        &mut lines,
        axes,
        flex_wrap,
        axes.cross.size(inner_size),
    );

    let cross_was_definite = axes.cross.size(outer_size).is_some();
    let inset_cross = axes.cross.size(container_inset_size);
    if !cross_was_definite {
        let outer_cross = if let Some(intrinsic) = size_containment {
            clamp_axis(
                axes.cross.size(intrinsic).unwrap_or(0.0) + inset_cross,
                axes.cross.size(min_size),
                axes.cross.size(max_size),
                inset_cross,
            )
        } else {
            determine_auto_cross_size(
                &lines,
                axes.cross.size(gap),
                inset_cross,
                axes.cross.size(min_size),
                axes.cross.size(max_size),
                axes,
                axes.cross.size(inner_available_space),
            )
        };
        axes.cross.set_size(&mut outer_size, Some(outer_cross));
        axes.cross
            .set_size(&mut inner_size, Some((outer_cross - inset_cross).max(0.0)));
    }
    if cross_was_definite {
        let resolved_cross_gap =
            resolve_gap_axis(axes.cross.size(gap_value), axes.cross.size(inner_size));
        axes.cross.set_size(&mut gap, resolved_cross_gap);
    }
    let inner_cross = axes.cross.size(inner_size).unwrap_or(0.0);
    if item_inline_basis_was_indefinite {
        gap = resolve_gap(gap_value, inner_size);
        main_gap = axes.main.size(gap);
        for item in &mut items {
            let key = item.key;
            *item = resolve_item(tree, key, inner_size, axes, rtl, align_items);
        }
        let final_available_space = Size::new(
            AvailableSpace::Definite(inner_size.width.unwrap_or(0.0)),
            AvailableSpace::Definite(inner_size.height.unwrap_or(0.0)),
        );
        determine_flex_base_sizes(
            tree,
            state,
            &mut items,
            axes,
            inner_size,
            final_available_space,
            if main_percentage_basis_was_indefinite {
                None
            } else {
                axes.main.size(inner_size)
            },
            !main_percentage_basis_was_indefinite,
            false,
        );
        lines = collect_flex_lines(
            &items,
            flex_wrap,
            AvailableSpace::Definite(inner_main),
            main_gap,
            axes,
        );
        for line in lines.iter().copied() {
            resolve_flexible_lengths(&mut items, line, inner_main, main_gap, axes);
        }
        determine_hypothetical_cross_sizes(
            tree,
            state,
            &mut items,
            &lines,
            axes,
            flex_wrap,
            inner_size,
            final_available_space,
        );
        calculate_line_cross_sizes(&items, &mut lines, axes, flex_wrap, Some(inner_cross));
    }
    let cross_gap = axes.cross.size(gap);
    if flex_wrap == flex_wrap::T::Nowrap
        && let Some(line) = lines.first_mut()
    {
        line.cross_size = inner_cross;
    }
    stretch_lines(&mut lines, flex_wrap, align_content, inner_cross, cross_gap);
    determine_used_cross_sizes(&mut items, &lines, axes);
    distribute_main_axis(
        &mut items,
        &lines,
        axes,
        inner_main,
        main_gap,
        justify_content,
    );
    align_lines(
        &mut lines,
        axes,
        flex_wrap,
        align_content,
        inner_cross,
        cross_gap,
    );
    align_items_cross_axis(&mut items, &lines, axes);

    let outer_size = outer_size.unwrap_or(Size::ZERO);
    let inner_size = inner_size.unwrap_or(Size::ZERO);
    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let provisional_baseline =
        first_container_baseline(&items, &lines, axes, inner_size, content_origin);
    if matches!(input.goal, LayoutGoal::Measure(_)) {
        let baseline = if layout_contained {
            None
        } else {
            provisional_baseline
        };
        return LayoutOutput::new(outer_size, outer_size)
            .with_first_baselines(Point::new(None, baseline));
    }

    let (mut content_size, first_baseline) = perform_in_flow_layout(
        tree,
        state,
        &mut items,
        &lines,
        axes,
        inner_size,
        content_origin,
        outer_size,
    );
    for (document_index, child) in hidden {
        super::hide_subtree(tree, state, child);
        tree.set_unrounded_layout(
            state,
            child,
            Layout::with_order(u32::try_from(document_index).unwrap_or(u32::MAX)),
        );
    }
    let absolute_content_size = perform_absolute_children(
        tree,
        state,
        &absolute_items,
        axes,
        rtl,
        inner_size,
        outer_size,
        padding,
        border,
        justify_content,
        align_items,
    );
    content_size = content_size.zip_map(absolute_content_size, f32::max);
    let content_size = own_scrollable_overflow(&style, outer_size, content_size);

    let baseline = if layout_contained {
        None
    } else {
        first_baseline.or(provisional_baseline)
    };
    LayoutOutput::new(outer_size, content_size).with_first_baselines(Point::new(None, baseline))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::float_cmp)]
mod tests {
    use core::cell::Cell;

    use stylo::values::computed::{Display, Overflow};

    use super::*;
    use crate::tree::LayoutSlot;

    #[derive(Debug)]
    struct TestStyle(Display, PositionProperty, i32);

    static TEST_STYLES: [TestStyle; 5] = [
        TestStyle(Display::Flex, PositionProperty::Static, 0),
        TestStyle(Display::Flex, PositionProperty::Static, 2),
        TestStyle(Display::Flex, PositionProperty::Absolute, 0),
        TestStyle(Display::None, PositionProperty::Static, 0),
        TestStyle(Display::Flex, PositionProperty::Static, -1),
    ];

    impl CoreStyle for TestStyle {
        fn display(&self) -> Display {
            self.0
        }

        fn position(&self) -> PositionProperty {
            self.1
        }

        fn order(&self) -> i32 {
            self.2
        }
    }

    std::thread_local! {
        static TEST_MEASURE_CALLS: Cell<usize> = const { Cell::new(0) };
        static TEST_CROSS_SIZE: Cell<f32> = const { Cell::new(0.0) };
        static TEST_CROSS_BASELINE: Cell<Option<f32>> = const { Cell::new(None) };
    }

    /// Minimal indexed handle for line-math, probe-cache, and collection tests.
    #[derive(Debug, Clone, Copy)]
    struct TestRef(usize);

    #[derive(Debug, Default)]
    struct TestTree;

    #[derive(Debug)]
    struct TestState {
        layouts: [LayoutSlot; 5],
    }

    impl Default for TestState {
        fn default() -> Self {
            Self {
                layouts: core::array::from_fn(|_| LayoutSlot::default()),
            }
        }
    }

    impl LayoutTree for TestTree {
        type NodeId = TestRef;
        type State = TestState;
        type Style<'tree> = &'static TestStyle;
        type ChildIter<'tree> = core::array::IntoIter<TestRef, 4>;

        fn children(&self, _node: TestRef) -> Self::ChildIter<'_> {
            [TestRef(1), TestRef(2), TestRef(3), TestRef(4)].into_iter()
        }

        fn style(&self, node: TestRef) -> &'static TestStyle {
            &TEST_STYLES[node.0]
        }

        fn layout<'state>(&self, state: &'state Self::State, node: TestRef) -> &'state LayoutSlot {
            &state.layouts[node.0]
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            node: TestRef,
        ) -> &'state mut LayoutSlot {
            &mut state.layouts[node.0]
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            _node: TestRef,
            input: LayoutInput,
        ) -> LayoutOutput {
            TEST_MEASURE_CALLS.with(|calls| calls.set(calls.get() + 1));
            let width = match input.available_space.width {
                AvailableSpace::MinContent => 11.0,
                AvailableSpace::MaxContent => 23.0,
                AvailableSpace::Definite(value) => value,
            };
            let (height, baseline) = if input.goal == LayoutGoal::Measure(RequestedAxis::Both) {
                (TEST_CROSS_SIZE.get(), TEST_CROSS_BASELINE.get())
            } else {
                (0.0, None)
            };
            LayoutOutput::new(Size::new(width, height), Size::ZERO)
                .with_first_baselines(Point::new(None, baseline))
        }
    }

    fn item(main: f32, cross: f32) -> FlexItem<TestRef> {
        FlexItem {
            geometry: ItemGeometry {
                preferred_size: Size::NONE,
                min_size: Size::NONE,
                max_size: Size::NONE,
                margin: Edges::ZERO,
                padding: Edges::ZERO,
                border: Edges::ZERO,
                aspect_ratio: None,
                intrinsic: crate::compute::util::IntrinsicTags::default(),
                preferred_definite: Size::new(false, false),
                size_is_auto: Size::new(true, true),
                overflow: Point::new(Overflow::Visible, Overflow::Visible),
                box_sizing: box_sizing::T::ContentBox,
                margin_auto: crate::compute::util::EdgeMask::default(),
            },
            key: ItemKey {
                node: TestRef(0),
                layout_order: 0,
            },
            direction: direction::T::Ltr,
            position: PositionProperty::Relative,
            align_self: AlignFlags::STRETCH,
            inset: Edges::uniform(None),
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: main,
            inner_flex_basis: main,
            min_content_contribution: main,
            max_content_contribution: main,
            resolved_min_main: 0.0,
            hypothetical_main: main,
            target_main: main,
            hypothetical_cross: cross,
            target_cross: cross,
            baseline: cross,
            measured_baselines: Point::NONE,
            frozen: false,
            violation: 0.0,
            main_position: 0.0,
            cross_position: 0.0,
            main_size_is_definite: false,
        }
    }

    fn row_axes(wrap: flex_wrap::T) -> Axes {
        flex_axes(flex_direction::T::Row, wrap, direction::T::Ltr)
    }

    fn test_line(end: usize, cross_size: f32) -> FlexLine {
        FlexLine {
            start: 0,
            end,
            cross_size,
            cross_position: 0.0,
        }
    }

    fn align_one_cross(
        candidate: FlexItem<TestRef>,
        axes: Axes,
        line_cross_size: f32,
    ) -> FlexItem<TestRef> {
        let mut items = [candidate];
        align_items_cross_axis(&mut items, &[test_line(1, line_cross_size)], axes);
        items.into_iter().next().unwrap()
    }

    fn probes<'tree, 'state>(
        tree: &'tree TestTree,
        state: &'state mut TestState,
        available_main: AvailableSpace,
    ) -> MainAxisProbes<'tree, 'state, TestTree> {
        MainAxisProbes {
            tree,
            state,
            node: TestRef(0),
            axes: row_axes(flex_wrap::T::Nowrap),
            known_dimensions: Size::NONE,
            definite_dimensions: Size::new(false, false),
            parent_size: Size::NONE,
            available_space: Size::new(available_main, AvailableSpace::MaxContent),
            values: [0.0; 3],
            measured: 0,
        }
    }

    fn configure_measurement(cross: f32, baseline: Option<f32>) {
        TEST_MEASURE_CALLS.set(0);
        TEST_CROSS_SIZE.set(cross);
        TEST_CROSS_BASELINE.set(baseline);
    }

    fn measure_cross(items: &mut [FlexItem<TestRef>], wrap: flex_wrap::T, cross: f32) {
        let tree = TestTree;
        let mut state = TestState::default();
        determine_hypothetical_cross_sizes(
            &tree,
            &mut state,
            items,
            &[test_line(items.len(), 0.0)],
            row_axes(wrap),
            wrap,
            Size::new(Some(100.0), Some(cross)),
            Size::new(
                AvailableSpace::Definite(100.0),
                AvailableSpace::Definite(cross),
            ),
        );
    }

    macro_rules! assert_dist {
        ($alignment:expr, $space:expr, $count:expr, $reversed:expr, $expected:expr) => {
            assert_eq!(
                alignment_distribution($alignment, $space, $count, $reversed, false),
                $expected
            );
        };
    }

    fn static_cross(alignment: AlignFlags, wrap: flex_wrap::T) -> Point<f32> {
        let mut candidate = item(20.0, 10.0);
        candidate.align_self = alignment;
        static_position_for_absolute(
            &candidate,
            row_axes(wrap),
            Size::new(100.0, 50.0),
            Point::new(5.0, 7.0),
            AlignFlags::FLEX_START,
        )
    }

    #[test]
    fn measure_collection_keeps_only_ordered_in_flow_items() {
        let tree = TestTree;
        let measured =
            collect_flex_items(&tree, TestRef(0), LayoutGoal::Measure(RequestedAxis::Both));
        assert!(measured.generated.iter().map(|item| item.node.0).eq([4, 1]));
        assert!(measured.generated.iter().all(|item| item.layout_order == 0));
        assert!(measured.absolute_items.is_empty());
        assert!(measured.hidden.is_empty());
        assert!(measured.generated.capacity() >= 4);
        assert_eq!(
            (
                measured.absolute_items.inline_size(),
                measured.hidden.inline_size()
            ),
            (1, 1)
        );

        let committed = collect_flex_items(&tree, TestRef(0), LayoutGoal::Commit);
        assert!(
            committed
                .generated
                .iter()
                .map(|item| (item.node.0, item.layout_order))
                .eq([(4, 0), (1, 2)])
        );
        assert!(
            committed
                .absolute_items
                .iter()
                .map(|item| (item.node.0, item.layout_order))
                .eq([(2, 1)])
        );
        assert_eq!(committed.hidden[0].1.0, 3);
        assert!(committed.generated.capacity() >= 4);
        assert_eq!(committed.absolute_items.inline_size(), 1);
        assert_eq!(committed.hidden.inline_size(), 1);
    }

    #[test]
    fn nowrap_line_uses_inline_storage() {
        let lines = collect_flex_lines(
            &[item(10.0, 5.0)],
            flex_wrap::T::Nowrap,
            AvailableSpace::Definite(100.0),
            0.0,
            row_axes(flex_wrap::T::Nowrap),
        );

        assert!(!lines.spilled());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn main_axis_probes_are_lazy_memoized_and_reuse_max_content() {
        let tree = TestTree;
        let mut state = TestState::default();
        TEST_MEASURE_CALLS.set(0);
        let mut definite = probes(&tree, &mut state, AvailableSpace::Definite(37.0));
        assert_eq!(TEST_MEASURE_CALLS.get(), 0);
        assert_eq!(definite.min_content(), 11.0);
        assert_eq!(definite.min_content(), 11.0);
        assert_eq!(TEST_MEASURE_CALLS.get(), 1);
        assert_eq!(definite.max_content(), 23.0);
        assert_eq!(definite.max_content(), 23.0);
        assert_eq!(TEST_MEASURE_CALLS.get(), 2);
        assert_eq!(definite.available_content(), 37.0);
        assert_eq!(definite.available_content(), 37.0);
        assert_eq!(TEST_MEASURE_CALLS.get(), 3);

        TEST_MEASURE_CALLS.set(0);
        let mut intrinsic = probes(&tree, &mut state, AvailableSpace::MinContent);
        assert_eq!(intrinsic.available_content(), 23.0);
        assert_eq!(intrinsic.available_content(), 23.0);
        assert_eq!(TEST_MEASURE_CALLS.get(), 1);
    }

    #[test]
    fn flex_base_size_skips_unconsumed_intrinsic_probes() {
        let axes = row_axes(flex_wrap::T::Nowrap);
        let mut items = [item(0.0, 0.0)];
        items[0].min_size.width = Some(0.0);
        let tree = TestTree;
        let mut state = TestState::default();

        TEST_MEASURE_CALLS.set(0);
        determine_flex_base_sizes(
            &tree,
            &mut state,
            &mut items,
            axes,
            Size::new(Some(100.0), Some(20.0)),
            Size::new(
                AvailableSpace::Definite(37.0),
                AvailableSpace::Definite(20.0),
            ),
            Some(100.0),
            true,
            true,
        );

        assert_eq!(items[0].flex_basis, 37.0);
        assert_eq!(TEST_MEASURE_CALLS.get(), 1);
    }

    #[test]
    fn explicit_cross_elision_retains_alignment_and_exported_baselines() {
        let mut items = [item(10.0, 0.0), item(10.0, 0.0), item(10.0, 0.0)];
        for (item, cross) in items.iter_mut().zip([20.0, 22.0, 24.0]) {
            item.preferred_size.height = Some(cross);
            item.preferred_definite.height = true;
        }
        items[0].min_size.height = Some(21.0);
        items[1].align_self = AlignFlags::BASELINE;
        items[2].max_size.height = Some(23.0);

        configure_measurement(22.0, Some(7.0));
        measure_cross(&mut items, flex_wrap::T::Wrap, 50.0);

        assert_eq!(TEST_MEASURE_CALLS.get(), 1);
        assert_eq!(
            items.each_ref().map(|item| item.hypothetical_cross),
            [21.0, 22.0, 23.0]
        );
        assert_eq!(items[0].measured_baselines, Point::NONE);
        assert_eq!(items[1].measured_baselines.y, Some(7.0));
        assert_eq!(items[1].baseline, 7.0);
        assert_eq!(items[2].measured_baselines, Point::NONE);

        let mut fallback = [item(10.0, 0.0), item(10.0, 0.0)];
        fallback[0].preferred_size.height = Some(18.0);
        fallback[1].preferred_size.height = Some(19.0);
        configure_measurement(18.0, Some(6.0));
        measure_cross(&mut fallback, flex_wrap::T::Wrap, 50.0);
        assert_eq!(TEST_MEASURE_CALLS.get(), 1);
        assert_eq!(fallback[0].measured_baselines.y, Some(6.0));
        assert_eq!(fallback[1].measured_baselines, Point::NONE);
    }

    #[test]
    fn intrinsic_cross_constraints_keep_the_child_probe() {
        let mut items = [item(10.0, 0.0), item(10.0, 0.0), item(10.0, 0.0)];
        for item in &mut items {
            item.preferred_size.height = Some(10.0);
            item.preferred_definite.height = true;
        }
        items[1].intrinsic = crate::compute::util::IntrinsicTags::new(
            Size::new(&StyleSize::Auto, &StyleSize::Auto),
            Size::new(&StyleSize::Auto, &StyleSize::MinContent),
            Size::new(&MaxSize::none(), &MaxSize::none()),
        );
        items[2].intrinsic = crate::compute::util::IntrinsicTags::new(
            Size::new(&StyleSize::Auto, &StyleSize::Auto),
            Size::new(&StyleSize::Auto, &StyleSize::Auto),
            Size::new(&MaxSize::none(), &MaxSize::MaxContent),
        );

        configure_measurement(25.0, None);
        measure_cross(&mut items, flex_wrap::T::Wrap, 50.0);

        assert_eq!(TEST_MEASURE_CALLS.get(), 3);
        assert_eq!(
            items.each_ref().map(|item| item.hypothetical_cross),
            [25.0, 25.0, 25.0]
        );
    }

    #[test]
    fn definite_nowrap_stretch_elides_only_overwritten_cross_probes() {
        let axes = row_axes(flex_wrap::T::Nowrap);
        let mut items = [item(10.0, 0.0), item(10.0, 0.0), item(10.0, 0.0)];
        items[1].margin.top = 2.0;
        items[1].margin.bottom = 3.0;
        items[1].max_size.height = Some(30.0);
        let mut lines = [test_line(3, 0.0)];

        configure_measurement(13.0, Some(5.0));
        measure_cross(&mut items, flex_wrap::T::Nowrap, 40.0);

        assert_eq!(TEST_MEASURE_CALLS.get(), 1);
        assert_eq!(
            items.each_ref().map(|item| item.hypothetical_cross),
            [13.0, 30.0, 40.0]
        );
        assert_eq!(items[0].measured_baselines.y, Some(5.0));
        calculate_line_cross_sizes(&items, &mut lines, axes, flex_wrap::T::Nowrap, Some(40.0));
        determine_used_cross_sizes(&mut items, &lines, axes);
        assert_eq!(
            items.each_ref().map(|item| item.target_cross),
            [40.0, 30.0, 40.0]
        );
    }

    #[test]
    fn auto_cross_keeps_height_for_width_probes_when_lines_need_content_size() {
        let mut items = [item(10.0, 0.0), item(20.0, 0.0)];

        configure_measurement(17.0, None);
        measure_cross(&mut items, flex_wrap::T::Wrap, 40.0);

        assert_eq!(TEST_MEASURE_CALLS.get(), 2);
        assert_eq!(
            items.each_ref().map(|item| item.hypothetical_cross),
            [17.0, 17.0]
        );
    }

    #[test]
    fn physical_edge_helpers_cover_reversed_axes() {
        let mut edges = Edges::ZERO;
        set_flow_start(&mut edges, Axis::Horizontal, true, 1.0);
        set_flow_start(&mut edges, Axis::Vertical, true, 2.0);
        set_flow_end(&mut edges, Axis::Horizontal, true, 3.0);
        set_flow_end(&mut edges, Axis::Vertical, true, 4.0);
        assert_eq!(
            [edges.left, edges.right, edges.top, edges.bottom],
            [3.0, 1.0, 4.0, 2.0]
        );
    }

    #[test]
    fn alignment_distribution_covers_reverse_and_overflow_rules() {
        use AlignFlags as A;

        assert_dist!(A::START, 12.0, 2, false, (0.0, 0.0));
        assert_dist!(A::START, 12.0, 2, true, (12.0, 0.0));
        assert_dist!(A::END, 12.0, 2, false, (12.0, 0.0));
        assert_dist!(A::END, 12.0, 2, true, (0.0, 0.0));
        assert_dist!(A::FLEX_END, 12.0, 2, false, (12.0, 0.0));
        assert_dist!(A::CENTER, 12.0, 2, false, (6.0, 0.0));
        assert_dist!(A::SPACE_BETWEEN, 12.0, 3, false, (0.0, 6.0));
        assert_dist!(A::SPACE_AROUND, 12.0, 3, false, (2.0, 4.0));
        assert_dist!(A::SPACE_EVENLY, 12.0, 3, false, (3.0, 3.0));
        assert_dist!(A::SPACE_AROUND, -12.0, 3, false, (-6.0, 0.0));
        assert_dist!(A::SPACE_BETWEEN, -12.0, 3, false, (0.0, 0.0));
        assert_dist!(A::CENTER, 12.0, 0, false, (0.0, 0.0));
    }

    #[test]
    fn relative_offsets_respect_direction_and_opposing_edges() {
        let opposing = Edges {
            left: Some(7.0),
            right: Some(11.0),
            top: None,
            bottom: Some(5.0),
        };
        assert_eq!(
            [
                relative_offset(opposing, direction::T::Ltr),
                relative_offset(opposing, direction::T::Rtl)
            ],
            [Point::new(7.0, -5.0), Point::new(-11.0, -5.0)]
        );
    }

    #[test]
    fn intrinsic_line_helpers_cover_empty_min_max_and_definite_constraints() {
        let axes = row_axes(flex_wrap::T::Wrap);
        assert!(
            collect_flex_lines::<TestRef>(
                &[],
                flex_wrap::T::Wrap,
                AvailableSpace::Definite(10.0),
                0.0,
                axes
            )
            .is_empty()
        );

        let mut items = vec![item(10.0, 5.0), item(20.0, 7.0)];
        items[0].min_content_contribution = 8.0;
        items[1].min_content_contribution = 15.0;
        items[0].max_content_contribution = 12.0;
        items[1].max_content_contribution = 24.0;
        let lines = collect_flex_lines(
            &items,
            flex_wrap::T::Wrap,
            AvailableSpace::MinContent,
            2.0,
            axes,
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(
            line_intrinsic_main(&items, test_line(2, 0.0), 2.0, axes),
            32.0
        );
        for (name, available_main, expected) in [
            ("min content", AvailableSpace::MinContent, 16.0),
            ("max content", AvailableSpace::MaxContent, 25.0),
            ("definite", AvailableSpace::Definite(100.0), 21.0),
        ] {
            assert_eq!(
                determine_auto_main_size(
                    &items,
                    &lines,
                    2.0,
                    axes,
                    available_main,
                    1.0,
                    None,
                    None
                ),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn baseline_and_cross_alignment_cover_auto_margin_overflow_and_reversal() {
        let normal = row_axes(flex_wrap::T::Wrap);
        let reversed = row_axes(flex_wrap::T::WrapReverse);
        let mut baseline_items = vec![item(10.0, 20.0), item(10.0, 15.0)];
        baseline_items[0].align_self = AlignFlags::BASELINE;
        baseline_items[0].baseline = 12.0;
        baseline_items[0].margin = Edges {
            top: 2.0,
            bottom: 1.0,
            ..Edges::ZERO
        };
        baseline_items[1].align_self = AlignFlags::BASELINE;
        baseline_items[1].baseline = 5.0;
        baseline_items[1].margin = Edges {
            top: 1.0,
            bottom: 3.0,
            ..Edges::ZERO
        };
        let mut lines = [test_line(2, 0.0)];
        calculate_line_cross_sizes(
            &baseline_items,
            &mut lines,
            normal,
            flex_wrap::T::Wrap,
            None,
        );
        assert_eq!(lines[0].cross_size, 27.0);

        let mut positive_auto = item(10.0, 10.0);
        positive_auto.margin_auto = crate::compute::util::EdgeMask::from_edges(Edges {
            left: false,
            right: false,
            top: false,
            bottom: true,
        });
        let positive_auto = align_one_cross(positive_auto, normal, 20.0);
        assert_eq!(positive_auto.margin.bottom, 10.0);

        let mut overflowing_auto = item(10.0, 30.0);
        overflowing_auto.margin_auto = crate::compute::util::EdgeMask::from_edges(Edges {
            left: false,
            right: false,
            top: false,
            bottom: true,
        });
        let overflowing_auto = align_one_cross(overflowing_auto, reversed, 20.0);
        assert_eq!(overflowing_auto.margin.bottom, -10.0);

        let mut baseline = item(10.0, 20.0);
        baseline.align_self = AlignFlags::BASELINE;
        baseline.baseline = 8.0;
        let baseline = align_one_cross(baseline, reversed, 40.0);
        assert_eq!(baseline.cross_position, 20.0);

        let mut end = item(10.0, 20.0);
        end.align_self = AlignFlags::END;
        let normal_end = align_one_cross(end, normal, 40.0);
        assert_eq!(normal_end.cross_position, 20.0);

        let mut end = item(10.0, 20.0);
        end.align_self = AlignFlags::END;
        let reversed_end = align_one_cross(end, reversed, 40.0);
        assert_eq!(reversed_end.cross_position, 0.0);
    }

    #[test]
    fn absolute_static_cross_alignment_uses_logical_start_and_end() {
        use flex_wrap::T::{Wrap, WrapReverse};

        let origin = Point::new(5.0, 7.0);
        let end = Point::new(5.0, 47.0);
        assert_eq!(static_cross(AlignFlags::START, Wrap), origin);
        assert_eq!(static_cross(AlignFlags::START, WrapReverse), origin);
        assert_eq!(static_cross(AlignFlags::END, Wrap), end);
        assert_eq!(static_cross(AlignFlags::END, WrapReverse), end);
        assert_eq!(static_cross(AlignFlags::FLEX_END, Wrap), end);
    }
}
