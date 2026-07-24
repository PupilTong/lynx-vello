//! Starlight id-constrained relative layout.

use stylo::computed_values::{box_sizing, direction, relative_center, relative_layout_once};
use stylo::values::computed::lynx_layout::RelativeReference;
use stylo::values::computed::{PositionProperty, Size as StyleSize};

use super::compute_absolute_layout;
use super::util::{
    Axis, ItemGeometry, ItemKey, OrderedItem, ResolvedContainerBox, accumulate_scrollable_overflow,
    clamp_axis, own_scrollable_overflow, relative_offset, resolve_container_box, resolve_intrinsic,
    resolve_item_geometry_with_bases, resolve_length_percentage, sort_and_assign_layout_order,
    subtract_available_space,
};
use crate::geometry::{Edges, Line, Point, Size};
use crate::style::containment::size_containment;
use crate::style::{CoreStyle, RELATIVE_REFERENCE_NONE, RELATIVE_REFERENCE_PARENT};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, LayoutTree, RequestedAxis,
    SizingMode,
};

#[inline]
const fn reference_is_item(reference: RelativeReference) -> bool {
    reference != RELATIVE_REFERENCE_NONE && reference != RELATIVE_REFERENCE_PARENT
}

#[inline]
fn axis_centers(axis: Axis, center: relative_center::T) -> bool {
    match axis {
        Axis::Horizontal => matches!(
            center,
            relative_center::T::Horizontal | relative_center::T::Both
        ),
        Axis::Vertical => matches!(
            center,
            relative_center::T::Vertical | relative_center::T::Both
        ),
    }
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: f32,
    max: f32,
    definite: bool,
}

impl Bounds {
    #[inline]
    fn new(parent_extent: Option<f32>) -> Self {
        Self {
            min: 0.0,
            max: parent_extent.unwrap_or(0.0),
            definite: parent_extent.is_some(),
        }
    }

    #[inline]
    fn include(&mut self, position: Line<f32>) {
        if !self.definite {
            self.min = self.min.min(position.start);
            self.max = self.max.max(position.end);
        }
    }

    #[inline]
    fn extent(self) -> f32 {
        (self.max - self.min).max(0.0)
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedReference(u32);

impl ResolvedReference {
    const NONE: Self = Self(u32::MAX);
    const PARENT: Self = Self(u32::MAX - 1);

    #[inline]
    fn resolve(reference: RelativeReference, lookup: &IdLookup) -> Self {
        if reference == RELATIVE_REFERENCE_NONE {
            Self::NONE
        } else if reference == RELATIVE_REFERENCE_PARENT {
            Self::PARENT
        } else {
            lookup.get(reference).map_or(Self::NONE, |index| {
                Self(u32::try_from(index).expect("relative item count exceeds u32"))
            })
        }
    }

    #[inline]
    const fn is_parent(self) -> bool {
        self.0 == Self::PARENT.0
    }

    #[inline]
    const fn item_index_u32(self) -> Option<u32> {
        if self.0 < Self::PARENT.0 {
            Some(self.0)
        } else {
            None
        }
    }

    #[inline]
    fn item_index(self) -> Option<usize> {
        self.item_index_u32()
            .map(|index| usize::try_from(index).expect("relative item index does not fit usize"))
    }
}

#[derive(Debug)]
struct RelativeItem<N> {
    geometry: ItemGeometry,
    key: ItemKey<N>,
    align: Edges<ResolvedReference>,
    adjacent: Edges<ResolvedReference>,
    center: relative_center::T,
    position: PositionProperty,
    intrinsic_preferred_size: Size<Option<f32>>,
    intrinsic_sizes_ready: bool,
    inset: Edges<Option<f32>>,
    direction: direction::T,
    positions: Size<Line<f32>>,
    output: LayoutOutput,
    last_measure: Option<LayoutInput>,
    size_is_definite: Size<bool>,
    reuse_fixed_measurement: bool,
}
super::util::impl_item_geometry!(RelativeItem);

impl<N> RelativeItem<N> {
    #[inline]
    fn outer_size(&self, axis: Axis) -> f32 {
        axis.size(self.output.size) + axis.sum(self.margin)
    }

    #[inline]
    fn fixed_measurement_matches(&self, refreshed: &Self) -> bool {
        self.preferred_size.width.is_some()
            && self.preferred_size.height.is_some()
            && self.preferred_definite.width
            && self.preferred_definite.height
            && self.preferred_size == refreshed.preferred_size
            && self.preferred_definite == refreshed.preferred_definite
            && self.min_size == refreshed.min_size
            && self.max_size == refreshed.max_size
            && self.box_sizing == refreshed.box_sizing
            && self.margin == refreshed.margin
            && self.padding == refreshed.padding
            && self.border == refreshed.border
    }
}

fn resolve_item<T>(
    tree: &T,
    key: ItemKey<T::NodeId>,
    size_percentage_basis: Size<Option<f32>>,
    edge_inline_basis: Option<f32>,
    lookup: &IdLookup,
) -> RelativeItem<T::NodeId>
where
    T: LayoutTree,
{
    let style = tree.style(key.node);
    let geometry =
        resolve_item_geometry_with_bases(&style, size_percentage_basis, edge_inline_basis);
    RelativeItem {
        geometry,
        key,
        align: style
            .relative_align()
            .map(|reference| ResolvedReference::resolve(reference, lookup)),
        adjacent: style
            .relative_adjacent()
            .map(|reference| ResolvedReference::resolve(reference, lookup)),
        center: style.relative_center(),
        position: style.position(),
        intrinsic_preferred_size: Size::NONE,
        intrinsic_sizes_ready: false,
        inset: super::util::resolve_insets(style.inset(), size_percentage_basis),
        direction: style.direction(),
        positions: Size::new(Line::new(0.0, 0.0), Line::new(0.0, 0.0)),
        output: LayoutOutput::HIDDEN,
        last_measure: None,
        size_is_definite: Size::new(false, false),
        reuse_fixed_measurement: false,
    }
}

/// Compact, sorted id lookup. Sorting by `(id, ordered_index)` and retaining
/// the final pair implements duplicate-id last-wins semantics without a
/// randomized hash table in the hot path.
#[derive(Debug)]
struct IdLookup {
    entries: Vec<(i32, usize)>,
}

impl IdLookup {
    fn new<T>(tree: &T, items: &[OrderedItem<T::NodeId>]) -> Self
    where
        T: LayoutTree,
    {
        let mut entries = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let relative_id = tree.style(item.node).relative_id();
            if reference_is_item(relative_id) {
                if entries.is_empty() {
                    entries.reserve(items.len());
                }
                entries.push((relative_id, index));
            }
        }
        entries.sort_unstable();

        let mut write = 0;
        for read in 0..entries.len() {
            let entry = entries[read];
            if write > 0 && entries[write - 1].0 == entry.0 {
                entries[write - 1] = entry;
            } else {
                entries[write] = entry;
                write += 1;
            }
        }
        entries.truncate(write);
        Self { entries }
    }

    #[inline]
    fn get(&self, id: RelativeReference) -> Option<usize> {
        if !reference_is_item(id) {
            return None;
        }
        self.entries
            .binary_search_by_key(&id, |entry| entry.0)
            .ok()
            .map(|index| self.entries[index].1)
    }
}

#[derive(Debug, Clone, Copy)]
enum DependencyScope {
    Horizontal,
    Vertical,
    Combined,
}

#[derive(Debug, Clone, Copy)]
struct Dependencies {
    values: [u32; 8],
    len: u8,
}

impl Dependencies {
    const EMPTY: Self = Self {
        values: [0; 8],
        len: 0,
    };

    #[inline]
    fn add(&mut self, value: u32) {
        let len = usize::from(self.len);
        if self.values[..len].contains(&value) {
            return;
        }
        self.values[len] = value;
        self.len += 1;
    }

    #[inline]
    fn as_slice(&self) -> &[u32] {
        &self.values[..usize::from(self.len)]
    }
}

#[inline]
fn has_axis_dependencies<N>(item: &RelativeItem<N>, axis: Axis) -> bool {
    let references = [
        axis.start(item.align),
        axis.end(item.align),
        axis.end(item.adjacent),
        axis.start(item.adjacent),
    ];
    references
        .into_iter()
        .any(|reference| reference.item_index_u32().is_some())
}

fn add_axis_dependencies<N>(item: &RelativeItem<N>, axis: Axis, dependencies: &mut Dependencies) {
    let align_start = axis.start(item.align);
    let align_end = axis.end(item.align);
    let after = axis.end(item.adjacent);
    let before = axis.start(item.adjacent);
    for reference in [align_start, align_end, after, before] {
        if let Some(index) = reference.item_index_u32() {
            dependencies.add(index);
        }
    }
}

fn dependency_order<N>(items: &[RelativeItem<N>], scope: DependencyScope) -> Vec<usize> {
    let count = items.len();
    let has_dependencies = items.iter().any(|item| match scope {
        DependencyScope::Horizontal => has_axis_dependencies(item, Axis::Horizontal),
        DependencyScope::Vertical => has_axis_dependencies(item, Axis::Vertical),
        DependencyScope::Combined => {
            has_axis_dependencies(item, Axis::Horizontal)
                || has_axis_dependencies(item, Axis::Vertical)
        }
    });
    if !has_dependencies {
        return Vec::new();
    }

    let mut dependencies = vec![Dependencies::EMPTY; count];
    for (index, item) in items.iter().enumerate() {
        match scope {
            DependencyScope::Horizontal => {
                add_axis_dependencies(item, Axis::Horizontal, &mut dependencies[index]);
            }
            DependencyScope::Vertical => {
                add_axis_dependencies(item, Axis::Vertical, &mut dependencies[index]);
            }
            DependencyScope::Combined => {
                add_axis_dependencies(item, Axis::Horizontal, &mut dependencies[index]);
                add_axis_dependencies(item, Axis::Vertical, &mut dependencies[index]);
            }
        }
    }

    let mut outgoing_counts = vec![0_usize; count];
    let mut indegree = Vec::with_capacity(count);
    let mut edge_count = 0;
    for item_dependencies in &dependencies {
        let len = item_dependencies.as_slice().len();
        indegree.push(u8::try_from(len).expect("relative item has at most eight dependencies"));
        edge_count += len;
        for &dependency in item_dependencies.as_slice() {
            let dependency =
                usize::try_from(dependency).expect("relative dependency index does not fit usize");
            outgoing_counts[dependency] += 1;
        }
    }

    let mut offsets = Vec::with_capacity(count + 1);
    offsets.push(0);
    for &outgoing in &outgoing_counts {
        offsets.push(offsets.last().copied().unwrap_or(0) + outgoing);
    }
    outgoing_counts.fill(0);
    let mut dependents = vec![0_usize; edge_count];
    for (dependent, item_dependencies) in dependencies.iter().enumerate() {
        for &dependency in item_dependencies.as_slice() {
            let dependency =
                usize::try_from(dependency).expect("relative dependency index does not fit usize");
            let cursor = offsets[dependency] + outgoing_counts[dependency];
            dependents[cursor] = dependent;
            outgoing_counts[dependency] += 1;
        }
    }
    drop(dependencies);

    outgoing_counts.clear();
    let mut ready = outgoing_counts;
    for (index, &degree) in indegree.iter().enumerate() {
        if degree == 0 {
            ready.push(index);
        }
    }
    let mut ready_head = 0;
    let mut lowest_remaining = 0;
    let mut order = Vec::with_capacity(count);

    while order.len() < count {
        let current = if ready_head < ready.len() {
            let current = ready[ready_head];
            ready_head += 1;
            current
        } else {
            while indegree[lowest_remaining] == u8::MAX {
                lowest_remaining += 1;
            }
            let current = lowest_remaining;
            lowest_remaining += 1;
            current
        };
        if indegree[current] == u8::MAX {
            continue;
        }
        indegree[current] = u8::MAX;
        order.push(current);

        for &dependent in &dependents[offsets[current]..offsets[current + 1]] {
            if indegree[dependent] == u8::MAX {
                continue;
            }
            indegree[dependent] -= 1;
            if indegree[dependent] == 0 {
                ready.push(dependent);
            }
        }
    }

    order
}

#[inline]
fn reference_position<N>(
    reference: ResolvedReference,
    target_end: bool,
    axis: Axis,
    parent_size: Size<Option<f32>>,
    items: &[RelativeItem<N>],
    allow_item_references: bool,
) -> Option<f32> {
    if reference.is_parent() {
        return axis
            .size(parent_size)
            .map(|extent| if target_end { extent } else { 0.0 });
    }
    if !allow_item_references {
        return None;
    }
    reference.item_index().map(|index| {
        let position = axis.size(items[index].positions);
        if target_end {
            position.end
        } else {
            position.start
        }
    })
}

fn axis_constraints<N>(
    item: &RelativeItem<N>,
    axis: Axis,
    parent_size: Size<Option<f32>>,
    items: &[RelativeItem<N>],
    allow_item_references: bool,
) -> Line<Option<f32>> {
    let align_start = axis.start(item.align);
    let align_end = axis.end(item.align);
    let after = axis.end(item.adjacent);
    let before = axis.start(item.adjacent);

    let start = reference_position(
        align_start,
        false,
        axis,
        parent_size,
        items,
        allow_item_references,
    )
    .or_else(|| reference_position(after, true, axis, parent_size, items, allow_item_references));
    let end = reference_position(
        align_end,
        true,
        axis,
        parent_size,
        items,
        allow_item_references,
    )
    .or_else(|| {
        reference_position(
            before,
            false,
            axis,
            parent_size,
            items,
            allow_item_references,
        )
    });
    Line::new(start, end)
}

fn all_constraints<N>(
    item: &RelativeItem<N>,
    parent_size: Size<Option<f32>>,
    items: &[RelativeItem<N>],
    allow_item_references: bool,
) -> Size<Line<Option<f32>>> {
    Size::new(
        axis_constraints(
            item,
            Axis::Horizontal,
            parent_size,
            items,
            allow_item_references,
        ),
        axis_constraints(
            item,
            Axis::Vertical,
            parent_size,
            items,
            allow_item_references,
        ),
    )
}

#[inline]
fn constrained_border_size(constraints: Line<Option<f32>>, margin_sum: f32) -> Option<f32> {
    match (constraints.start, constraints.end) {
        (Some(start), Some(end)) => Some(((end.max(start) - start) - margin_sum).max(0.0)),
        _ => None,
    }
}

fn fit_content_available(
    value: &StyleSize,
    axis: Axis,
    parent_size: Size<Option<f32>>,
    available: AvailableSpace,
    box_sizing: box_sizing::T,
    box_floor: f32,
) -> AvailableSpace {
    match value {
        StyleSize::MinContent => AvailableSpace::MinContent,
        StyleSize::MaxContent => AvailableSpace::MaxContent,
        StyleSize::FitContentFunction(limit) => {
            let owner = axis
                .size(parent_size)
                .or_else(|| available.definite_value());
            let limit = resolve_length_percentage(&limit.0, owner).map(|limit| {
                if box_sizing == box_sizing::T::ContentBox {
                    limit + box_floor
                } else {
                    limit
                }
            });
            match (available, limit) {
                (AvailableSpace::Definite(available), Some(limit)) => {
                    AvailableSpace::Definite(available.min(limit).max(0.0))
                }
                (_, Some(limit)) => AvailableSpace::Definite(limit.max(0.0)),
                (available, None) => available,
            }
        }
        StyleSize::Auto
        | StyleSize::LengthPercentage(_)
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => available,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

fn intrinsic_probe<T>(
    tree: &T,
    state: &mut T::State,
    item: &RelativeItem<T::NodeId>,
    axis: Axis,
    intrinsic_space: AvailableSpace,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) -> f32
where
    T: LayoutTree,
{
    let mut available = available_content;
    axis.set_size(&mut available, intrinsic_space);
    let mut input = LayoutInput::measure(Size::NONE, parent_size, available, axis.requested());
    input.sizing_mode = SizingMode::IgnoreSizeStyles;
    axis.size(tree.compute_layout(state, item.key.node, input).size)
}

fn prepare_intrinsic_sizes<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut RelativeItem<T::NodeId>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) where
    T: LayoutTree,
{
    if item.intrinsic_sizes_ready {
        return;
    }

    let style = tree.style(item.key.node);
    let full_raw_size = style.size();
    let full_raw_min = style.min_size();
    let full_raw_max = style.max_size();
    for axis in Axis::ALL {
        let raw_size = axis.size(full_raw_size);
        let raw_min = axis.size(full_raw_min);
        let raw_max = axis.size(full_raw_max);
        let needs_min = item.intrinsic.needs_min_content(axis);
        let needs_max = item.intrinsic.needs_max_content(axis);
        if !needs_min && !needs_max {
            continue;
        }

        let min_content = needs_min.then(|| {
            intrinsic_probe(
                tree,
                state,
                item,
                axis,
                AvailableSpace::MinContent,
                parent_size,
                available_content,
            )
        });
        let max_content = needs_max.then(|| {
            intrinsic_probe(
                tree,
                state,
                item,
                axis,
                AvailableSpace::MaxContent,
                parent_size,
                available_content,
            )
        });
        let floor = axis.size(item.box_floor());
        let basis = axis
            .size(parent_size)
            .or_else(|| axis.size(available_content).definite_value());
        let preferred = resolve_intrinsic(
            item.intrinsic.preferred(axis),
            raw_size,
            None,
            min_content,
            max_content,
            basis,
            floor,
            item.box_sizing,
        );
        let min = resolve_intrinsic(
            item.intrinsic.minimum(axis),
            raw_min,
            None,
            min_content,
            max_content,
            basis,
            floor,
            item.box_sizing,
        );
        let max = resolve_intrinsic(
            item.intrinsic.maximum(axis),
            raw_max,
            None,
            min_content,
            max_content,
            basis,
            floor,
            item.box_sizing,
        );
        axis.set_size(&mut item.intrinsic_preferred_size, preferred);
        if let Some(min) = min {
            axis.set_size(&mut item.min_size, Some(min));
        }
        if let Some(max) = max {
            axis.set_size(&mut item.max_size, Some(max));
        }
    }
    let floor = item.box_floor();
    for axis in Axis::ALL {
        if let Some(preferred) = axis.size(item.intrinsic_preferred_size) {
            let min = axis.size(item.min_size);
            let max = axis.size(item.max_size);
            axis.set_size(
                &mut item.intrinsic_preferred_size,
                Some(clamp_axis(preferred, min, max, axis.size(floor))),
            );
        }
    }
    item.intrinsic_sizes_ready = true;
}

#[inline]
fn clamped_item_axis<N>(item: &RelativeItem<N>, axis: Axis, value: f32, floor: Size<f32>) -> f32 {
    clamp_axis(
        value,
        axis.size(item.min_size),
        axis.size(item.max_size),
        axis.size(floor),
    )
}

fn measurement_input<T>(
    tree: &T,
    item: &RelativeItem<T::NodeId>,
    constraints: Size<Line<Option<f32>>>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) -> LayoutInput
where
    T: LayoutTree,
{
    let mut known_dimensions = Size::NONE;
    let mut constraint_definite = Size::new(false, false);
    let mut available_space = available_content;
    let floor = item.box_floor();
    let style = tree.style(item.key.node);
    let full_raw_size = style.size();

    for axis in Axis::ALL {
        let line = axis.size(constraints);
        let has_one_sided_constraint =
            matches!((line.start, line.end), (Some(_), None) | (None, Some(_)));
        let raw_size = axis.size(full_raw_size);
        let fit_content_needs_one_sided_measurement = item.intrinsic.preferred(axis)
            == super::util::IntrinsicTag::FitContent
            && has_one_sided_constraint;
        let constrained = constrained_border_size(line, axis.sum(item.margin))
            .map(|size| clamped_item_axis(item, axis, size, floor));
        let known_is_definite = constrained.is_some() || axis.size(item.preferred_definite);
        axis.set_size(&mut constraint_definite, known_is_definite);
        let known = constrained
            .or_else(|| {
                axis.size(item.preferred_size)
                    .map(|preferred| clamped_item_axis(item, axis, preferred, floor))
            })
            .or_else(|| {
                (!fit_content_needs_one_sided_measurement)
                    .then(|| axis.size(item.intrinsic_preferred_size))
                    .flatten()
            });
        axis.set_size(&mut known_dimensions, known);
        if let Some(known) = known {
            axis.set_size(&mut available_space, AvailableSpace::Definite(known));
        } else {
            let margin_sum = axis.sum(item.margin);
            let available = if fit_content_needs_one_sided_measurement {
                let child_available =
                    subtract_available_space(axis.size(available_space), margin_sum);
                let fitted_child_available = fit_content_available(
                    raw_size,
                    axis,
                    parent_size,
                    child_available,
                    item.box_sizing,
                    axis.size(floor),
                );
                let constrained_child_available =
                    match (line.start, line.end, fitted_child_available) {
                        (Some(start), None, AvailableSpace::Definite(available)) => {
                            AvailableSpace::Definite((available - start).max(0.0))
                        }
                        (None, Some(end), AvailableSpace::Definite(_)) => {
                            AvailableSpace::Definite(end.max(0.0))
                        }
                        (_, _, available) => available,
                    };
                match constrained_child_available {
                    AvailableSpace::Definite(available) => {
                        AvailableSpace::Definite(available + margin_sum)
                    }
                    intrinsic => intrinsic,
                }
            } else {
                let one_sided_available = match (line.start, line.end, axis.size(available_space)) {
                    (Some(start), None, AvailableSpace::Definite(available)) => {
                        AvailableSpace::Definite((available - start).max(0.0))
                    }
                    (None, Some(end), AvailableSpace::Definite(_)) => {
                        AvailableSpace::Definite((end + margin_sum).max(0.0))
                    }
                    (_, _, available) => available,
                };
                fit_content_available(
                    raw_size,
                    axis,
                    parent_size,
                    one_sided_available,
                    item.box_sizing,
                    axis.size(floor),
                )
            };
            axis.set_size(&mut available_space, available);
        }
    }

    let mut input = LayoutInput::measure(
        known_dimensions,
        parent_size,
        available_space,
        RequestedAxis::Both,
    );
    input.definite_dimensions = constraint_definite;
    input.sizing_mode = SizingMode::ApplySizeStyles;
    input
}

fn measure_item<T>(
    tree: &T,
    state: &mut T::State,
    item: &mut RelativeItem<T::NodeId>,
    input: LayoutInput,
) where
    T: LayoutTree,
{
    if let Some(previous) = item.last_measure {
        if previous == input {
            return;
        }
        if item.reuse_fixed_measurement
            && previous.goal == input.goal
            && previous.sizing_mode == input.sizing_mode
            && previous.known_dimensions == input.known_dimensions
            && previous.definite_dimensions == input.definite_dimensions
            && previous.available_space == input.available_space
        {
            item.last_measure = Some(input);
            return;
        }
    }
    item.reuse_fixed_measurement = false;
    if input.known_dimensions.width.is_some() && input.known_dimensions.height.is_some() {
        let size = input.known_dimensions.unwrap_or(Size::ZERO);
        item.output = LayoutOutput::new(size, size);
        item.last_measure = Some(input);
        item.size_is_definite = Size::new(
            item.preferred_definite.width || input.definite_dimensions.width,
            item.preferred_definite.height || input.definite_dimensions.height,
        );
        return;
    }
    let mut output = tree.compute_layout(state, item.key.node, input);
    let floor = item.box_floor();
    if input.known_dimensions.width.is_none() {
        let clamped_width = clamp_axis(
            output.size.width,
            item.min_size.width,
            item.max_size.width,
            floor.width,
        );
        if clamped_width.total_cmp(&output.size.width).is_eq() {
            output.size.width = clamped_width;
        } else {
            let mut refined = input;
            refined.known_dimensions.width = Some(clamped_width);
            refined.available_space.width = AvailableSpace::Definite(clamped_width);
            if !input.definite_dimensions.height {
                refined.known_dimensions.height = None;
                refined.available_space.height = input.available_space.height;
            }
            output = tree.compute_layout(state, item.key.node, refined);
            output.size.width = clamped_width;
        }
    }
    if input.known_dimensions.height.is_none() {
        output.size.height = clamp_axis(
            output.size.height,
            item.min_size.height,
            item.max_size.height,
            floor.height,
        );
    }
    output.content_size.width = output.content_size.width.max(output.size.width);
    output.content_size.height = output.content_size.height.max(output.size.height);
    item.output = output;
    item.last_measure = Some(input);
    item.size_is_definite = Size::new(
        item.preferred_definite.width || input.definite_dimensions.width,
        item.preferred_definite.height || input.definite_dimensions.height,
    );
}

fn position_from_constraints<N>(
    item: &RelativeItem<N>,
    axis: Axis,
    constraints: Line<Option<f32>>,
    bounds: Bounds,
) -> Line<f32> {
    let outer_size = item.outer_size(axis);
    match (constraints.start, constraints.end) {
        (Some(start), Some(end)) => Line::new(start, end.max(start)),
        (Some(start), None) => Line::new(start, start + outer_size),
        (None, Some(end)) => Line::new(end - outer_size, end),
        (None, None) => {
            let align_start = axis.start(item.align);
            let align_end = axis.end(item.align);
            if align_end.is_parent() {
                Line::new(bounds.max - outer_size, bounds.max)
            } else if align_start.is_parent() || !axis_centers(axis, item.center) {
                Line::new(bounds.min, bounds.min + outer_size)
            } else {
                let start = bounds.min + (bounds.max - bounds.min - outer_size) / 2.0;
                Line::new(start, start + outer_size)
            }
        }
    }
}

fn position_axis<N>(
    items: &mut [RelativeItem<N>],
    order: &[usize],
    axis: Axis,
    parent_size: Size<Option<f32>>,
) -> Bounds {
    let mut bounds = Bounds::new(axis.size(parent_size));
    for ordinal in 0..items.len() {
        let index = order.get(ordinal).copied().unwrap_or(ordinal);
        let constraints = axis_constraints(&items[index], axis, parent_size, items, true);
        let position = position_from_constraints(&items[index], axis, constraints, bounds);
        axis.set_size(&mut items[index].positions, position);
        bounds.include(position);
    }
    bounds
}

fn measure_all<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [RelativeItem<T::NodeId>],
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
    allow_item_references: bool,
) where
    T: LayoutTree,
{
    for index in 0..items.len() {
        prepare_intrinsic_sizes(
            tree,
            state,
            &mut items[index],
            parent_size,
            available_content,
        );
        let constraints = all_constraints(&items[index], parent_size, items, allow_item_references);
        let input = measurement_input(
            tree,
            &items[index],
            constraints,
            parent_size,
            available_content,
        );
        measure_item(tree, state, &mut items[index], input);
    }
}

fn one_pass_layout<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [RelativeItem<T::NodeId>],
    order: &[usize],
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) -> Size<Bounds>
where
    T: LayoutTree,
{
    let mut bounds = Size::new(
        Bounds::new(parent_size.width),
        Bounds::new(parent_size.height),
    );
    for ordinal in 0..items.len() {
        let index = order.get(ordinal).copied().unwrap_or(ordinal);
        prepare_intrinsic_sizes(
            tree,
            state,
            &mut items[index],
            parent_size,
            available_content,
        );
        let constraints = all_constraints(&items[index], parent_size, items, true);
        let input = measurement_input(
            tree,
            &items[index],
            constraints,
            parent_size,
            available_content,
        );
        measure_item(tree, state, &mut items[index], input);

        for axis in Axis::ALL {
            let axis_constraints = axis.size(constraints);
            let axis_bounds = axis.size(bounds);
            let position =
                position_from_constraints(&items[index], axis, axis_constraints, axis_bounds);
            axis.set_size(&mut items[index].positions, position);
            let mut updated = axis_bounds;
            updated.include(position);
            axis.set_size(&mut bounds, updated);
        }
    }
    bounds
}

fn refresh_item_bases<T>(
    tree: &T,
    items: &mut [RelativeItem<T::NodeId>],
    size_percentage_basis: Size<Option<f32>>,
    edge_inline_basis: Option<f32>,
    lookup: &IdLookup,
) where
    T: LayoutTree,
{
    for item in items {
        let positions = item.positions;
        let output = item.output;
        let last_measure = item.last_measure;
        let intrinsic_preferred_size = item.intrinsic_preferred_size;
        let intrinsic_sizes_ready = item.intrinsic_sizes_ready;
        let size_is_definite = item.size_is_definite;
        let mut refreshed = resolve_item(
            tree,
            item.key,
            size_percentage_basis,
            edge_inline_basis,
            lookup,
        );
        let reuse_fixed_measurement = item.fixed_measurement_matches(&refreshed);
        refreshed.positions = positions;
        refreshed.output = output;
        if reuse_fixed_measurement {
            refreshed.intrinsic_preferred_size = intrinsic_preferred_size;
            refreshed.intrinsic_sizes_ready = intrinsic_sizes_ready;
            refreshed.last_measure = last_measure;
            refreshed.size_is_definite = size_is_definite;
            refreshed.reuse_fixed_measurement = true;
        }
        *item = refreshed;
    }
}

#[inline]
fn contained_extent(size_containment: Option<Size<Option<f32>>>, axis: Axis) -> Option<f32> {
    size_containment.map(|intrinsic| axis.size(intrinsic).unwrap_or(0.0))
}

#[inline]
fn final_outer_axis(
    initial_outer: Option<f32>,
    caller_known: Option<f32>,
    content_extent: f32,
    inset: f32,
    min: Option<f32>,
    max: Option<f32>,
) -> f32 {
    if let Some(known) = caller_known {
        return known.max(0.0);
    }
    let candidate = initial_outer.unwrap_or(content_extent + inset);
    clamp_axis(candidate, min, max, inset).max(0.0)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn two_pass_layout<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [RelativeItem<T::NodeId>],
    horizontal_order: &[usize],
    vertical_order: &[usize],
    initial_parent_size: Size<Option<f32>>,
    mut available_content: Size<AvailableSpace>,
    lookup: &IdLookup,
    initial_outer: Size<Option<f32>>,
    caller_known: Size<Option<f32>>,
    box_inset: Size<f32>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    size_containment: Option<Size<Option<f32>>>,
) -> Size<f32>
where
    T: LayoutTree,
{
    measure_all(
        tree,
        state,
        items,
        initial_parent_size,
        available_content,
        false,
    );
    let _ = position_axis(
        items,
        horizontal_order,
        Axis::Horizontal,
        initial_parent_size,
    );
    let _ = position_axis(items, vertical_order, Axis::Vertical, initial_parent_size);

    measure_all(
        tree,
        state,
        items,
        initial_parent_size,
        available_content,
        true,
    );
    let horizontal_bounds = position_axis(
        items,
        horizontal_order,
        Axis::Horizontal,
        initial_parent_size,
    );
    let mut vertical_bounds =
        position_axis(items, vertical_order, Axis::Vertical, initial_parent_size);

    let outer_width = final_outer_axis(
        initial_outer.width,
        caller_known.width,
        contained_extent(size_containment, Axis::Horizontal)
            .unwrap_or_else(|| horizontal_bounds.extent()),
        box_inset.width,
        min_size.width,
        max_size.width,
    );
    let content_width = (outer_width - box_inset.width).max(0.0);
    let mut resolved_parent_size = initial_parent_size;

    if initial_parent_size.width.is_none() {
        resolved_parent_size.width = Some(content_width);
        available_content.width = AvailableSpace::Definite(content_width);

        refresh_item_bases(
            tree,
            items,
            resolved_parent_size,
            Some(content_width),
            lookup,
        );
        let _ = position_axis(
            items,
            horizontal_order,
            Axis::Horizontal,
            resolved_parent_size,
        );
        measure_all(
            tree,
            state,
            items,
            resolved_parent_size,
            available_content,
            true,
        );
        let _ = position_axis(
            items,
            horizontal_order,
            Axis::Horizontal,
            resolved_parent_size,
        );
        vertical_bounds =
            position_axis(items, vertical_order, Axis::Vertical, resolved_parent_size);
    }

    let outer_height = final_outer_axis(
        initial_outer.height,
        caller_known.height,
        contained_extent(size_containment, Axis::Vertical)
            .unwrap_or_else(|| vertical_bounds.extent()),
        box_inset.height,
        min_size.height,
        max_size.height,
    );
    let content_height = (outer_height - box_inset.height).max(0.0);
    resolved_parent_size.height = Some(content_height);

    let _ = position_axis(
        items,
        horizontal_order,
        Axis::Horizontal,
        resolved_parent_size,
    );
    let _ = position_axis(items, vertical_order, Axis::Vertical, resolved_parent_size);

    Size::new(outer_width, outer_height)
}

fn commit_in_flow<T>(
    tree: &T,
    state: &mut T::State,
    items: &mut [RelativeItem<T::NodeId>],
    content_size: Size<f32>,
    content_origin: Point<f32>,
    container_size: Size<f32>,
) -> Size<f32>
where
    T: LayoutTree,
{
    let parent_size = content_size.map(Some);
    let available = content_size.map(AvailableSpace::Definite);
    let mut scrollable_size = container_size;

    for item in items {
        let mut input = LayoutInput::commit(item.output.size.map(Some), parent_size, available);
        input.definite_dimensions = item.size_is_definite;
        input.sizing_mode = SizingMode::IgnoreSizeStyles;
        let output = tree.compute_layout(state, item.key.node, input);
        item.output = output;

        let offset = if item.position == PositionProperty::Relative {
            relative_offset(item.inset, item.direction)
        } else {
            Point::ZERO
        };
        let horizontal = item.positions.width;
        let vertical = item.positions.height;
        let location = Point::new(
            content_origin.x + horizontal.start + item.margin.left + offset.x,
            content_origin.y + vertical.start + item.margin.top + offset.y,
        );

        let mut layout = Layout::with_order(item.key.layout_order);
        layout.location = location;
        layout.size = output.size;
        layout.content_size = output.content_size;
        layout.border = item.border;
        layout.padding = item.padding;
        layout.margin = item.margin;
        tree.layout_mut(state, item.key.node).set_unrounded(layout);

        accumulate_scrollable_overflow(
            &mut scrollable_size,
            location,
            output.size,
            output.content_size,
            item.overflow,
        );
    }
    scrollable_size
}

fn commit_out_of_flow<T>(
    tree: &T,
    state: &mut T::State,
    items: &[OrderedItem<T::NodeId>],
    container_size: Size<f32>,
    border: Edges<f32>,
) -> Size<f32>
where
    T: LayoutTree,
{
    let padding_box_size = Size::new(
        (container_size.width - border.horizontal_sum()).max(0.0),
        (container_size.height - border.vertical_sum()).max(0.0),
    );
    let mut scrollable_size = container_size;
    for pending in items {
        let style = tree.style(pending.node);
        match style.position() {
            PositionProperty::Absolute => {
                let mut layout = compute_absolute_layout(
                    tree,
                    state,
                    pending.node,
                    padding_box_size,
                    Point::ZERO,
                );
                layout.order = pending.layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                accumulate_scrollable_overflow(
                    &mut scrollable_size,
                    layout.location,
                    layout.size,
                    layout.content_size,
                    style.overflow(),
                );
                tree.layout_mut(state, pending.node).set_unrounded(layout);
            }
            PositionProperty::Fixed => {
                tree.layout_mut(state, pending.node)
                    .set_static_position(Point::new(border.left, border.top));
            }
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {}
        }
    }
    scrollable_size
}

#[allow(clippy::too_many_lines)]
pub fn compute_relative_layout<T>(
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
    let layout_once = style.relative_layout_once() == relative_layout_once::T::True;
    let ResolvedContainerBox {
        preferred_definite: style_definite,
        padding,
        border,
        box_inset,
        min: min_size,
        max: max_size,
        outer: initial_outer,
        inner: initial_inner,
        available_inner,
        ..
    } = resolve_container_box(&style, input);
    let outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );

    if matches!(input.goal, LayoutGoal::Measure(_))
        && (size_containment.is_some()
            || (initial_outer.width.is_some() && initial_outer.height.is_some()))
    {
        let outer_size = Size::new(
            final_outer_axis(
                initial_outer.width,
                input.known_dimensions.width,
                contained_extent(size_containment, Axis::Horizontal).unwrap_or(0.0),
                box_inset.width,
                min_size.width,
                max_size.width,
            ),
            final_outer_axis(
                initial_outer.height,
                input.known_dimensions.height,
                contained_extent(size_containment, Axis::Vertical).unwrap_or(0.0),
                box_inset.height,
                min_size.height,
                max_size.height,
            ),
        );
        return LayoutOutput::new(outer_size, outer_size);
    }

    let initial_parent_size = Size::new(
        outer_definite
            .width
            .then_some(initial_inner.width)
            .flatten(),
        outer_definite
            .height
            .then_some(initial_inner.height)
            .flatten(),
    );
    let available_content = Size::new(
        initial_inner
            .width
            .map_or(available_inner.width, AvailableSpace::Definite),
        initial_inner
            .height
            .map_or(available_inner.height, AvailableSpace::Definite),
    );
    let edge_inline_basis = available_content.width.definite_value();

    let commits_layout = input.goal == LayoutGoal::Commit;
    let children = tree.children(node);
    let (lower, upper) = children.size_hint();
    let mut generated = Vec::with_capacity(upper.unwrap_or(lower));
    let mut absolute_items = Vec::new();
    let mut hidden = Vec::new();
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
            layout_order: u32::try_from(document_index).unwrap_or(u32::MAX),
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

    let lookup = IdLookup::new(tree, &generated);
    let mut items = generated
        .into_iter()
        .map(|item| {
            resolve_item(
                tree,
                item.key(),
                initial_parent_size,
                edge_inline_basis,
                &lookup,
            )
        })
        .collect::<Vec<_>>();

    let outer_size = if layout_once {
        let order = dependency_order(&items, DependencyScope::Combined);
        let bounds = one_pass_layout(
            tree,
            state,
            &mut items,
            &order,
            initial_parent_size,
            available_content,
        );
        Size::new(
            final_outer_axis(
                initial_outer.width,
                input.known_dimensions.width,
                contained_extent(size_containment, Axis::Horizontal)
                    .unwrap_or_else(|| bounds.width.extent()),
                box_inset.width,
                min_size.width,
                max_size.width,
            ),
            final_outer_axis(
                initial_outer.height,
                input.known_dimensions.height,
                contained_extent(size_containment, Axis::Vertical)
                    .unwrap_or_else(|| bounds.height.extent()),
                box_inset.height,
                min_size.height,
                max_size.height,
            ),
        )
    } else {
        let horizontal_order = dependency_order(&items, DependencyScope::Horizontal);
        let vertical_order = dependency_order(&items, DependencyScope::Vertical);
        two_pass_layout(
            tree,
            state,
            &mut items,
            &horizontal_order,
            &vertical_order,
            initial_parent_size,
            available_content,
            &lookup,
            initial_outer,
            input.known_dimensions,
            box_inset,
            min_size,
            max_size,
            size_containment,
        )
    };
    let content_size = Size::new(
        (outer_size.width - box_inset.width).max(0.0),
        (outer_size.height - box_inset.height).max(0.0),
    );
    if !commits_layout {
        return LayoutOutput::new(outer_size, outer_size);
    }

    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let mut scrollable_size = commit_in_flow(
        tree,
        state,
        &mut items,
        content_size,
        content_origin,
        outer_size,
    );
    for (document_index, child) in hidden {
        super::hide_subtree(tree, state, child);
        tree.layout_mut(state, child)
            .set_unrounded(Layout::with_order(
                u32::try_from(document_index).unwrap_or(u32::MAX),
            ));
    }
    scrollable_size = scrollable_size.zip_map(
        commit_out_of_flow(tree, state, &absolute_items, outer_size, border),
        f32::max,
    );

    let scrollable_size = own_scrollable_overflow(&style, outer_size, scrollable_size);
    LayoutOutput::new(outer_size, scrollable_size)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::cell::Cell;

    use stylo::values::computed::{
        Contain, ContainIntrinsicSize, Display, Length, LengthPercentage, Size as StyleSize,
    };
    use stylo::values::generics::NonNegative;

    use super::*;

    fn size_px(value: f32) -> StyleSize {
        StyleSize::LengthPercentage(NonNegative(LengthPercentage::new_length(Length::new(
            value,
        ))))
    }

    #[derive(Debug)]
    struct TestStyle {
        size: Size<StyleSize>,
        containment: Contain,
        contain_intrinsic_size: Size<ContainIntrinsicSize>,
    }

    impl TestStyle {
        fn auto() -> Self {
            Self {
                size: Size::new(StyleSize::auto(), StyleSize::auto()),
                containment: Contain::empty(),
                contain_intrinsic_size: Size::new(
                    ContainIntrinsicSize::None,
                    ContainIntrinsicSize::None,
                ),
            }
        }

        fn fixed(width: f32, height: f32) -> Self {
            Self {
                size: Size::new(size_px(width), size_px(height)),
                ..Self::auto()
            }
        }

        fn size_contained(width: Option<f32>, height: Option<f32>) -> Self {
            let intrinsic = |value: Option<f32>| {
                value.map_or(ContainIntrinsicSize::None, |value| {
                    ContainIntrinsicSize::Length(NonNegative(Length::new(value)))
                })
            };
            Self {
                containment: Contain::SIZE,
                contain_intrinsic_size: Size::new(intrinsic(width), intrinsic(height)),
                ..Self::auto()
            }
        }
    }

    impl CoreStyle for TestStyle {
        fn display(&self) -> Display {
            Display::LynxRelative
        }

        fn size(&self) -> Size<&StyleSize> {
            self.size.as_ref()
        }

        fn containment(&self) -> Contain {
            self.containment
        }

        fn contain_intrinsic_width(&self) -> ContainIntrinsicSize {
            self.contain_intrinsic_size.width.clone()
        }

        fn contain_intrinsic_height(&self) -> ContainIntrinsicSize {
            self.contain_intrinsic_size.height.clone()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestKind {
        Root,
        Child,
    }

    struct TestState {
        root_style: TestStyle,
        child_style: TestStyle,
        children_calls: Cell<usize>,
        child_measure_calls: Cell<usize>,
        child_commit_calls: Cell<usize>,
        committed_artifact: Cell<bool>,
    }

    impl TestState {
        fn new(root_style: TestStyle, child_style: TestStyle) -> Self {
            Self {
                root_style,
                child_style,
                children_calls: Cell::new(0),
                child_measure_calls: Cell::new(0),
                child_commit_calls: Cell::new(0),
                committed_artifact: Cell::new(false),
            }
        }

        const fn root() -> TestKind {
            TestKind::Root
        }
    }

    impl LayoutTree for TestState {
        type NodeId = TestKind;
        type State = [crate::tree::LayoutSlot; 2];
        type Style<'tree>
            = &'tree TestStyle
        where
            Self: 'tree;
        type ChildIter<'tree>
            = core::option::IntoIter<Self::NodeId>
        where
            Self: 'tree;

        fn children(&self, node: Self::NodeId) -> Self::ChildIter<'_> {
            self.children_calls.set(self.children_calls.get() + 1);
            (node == TestKind::Root)
                .then_some(TestKind::Child)
                .into_iter()
        }

        fn style(&self, node: Self::NodeId) -> Self::Style<'_> {
            match node {
                TestKind::Root => &self.root_style,
                TestKind::Child => &self.child_style,
            }
        }

        fn layout<'state>(
            &self,
            state: &'state Self::State,
            node: Self::NodeId,
        ) -> &'state crate::tree::LayoutSlot {
            &state[node as usize]
        }

        fn layout_mut<'state>(
            &self,
            state: &'state mut Self::State,
            node: Self::NodeId,
        ) -> &'state mut crate::tree::LayoutSlot {
            &mut state[node as usize]
        }

        fn compute_layout(
            &self,
            _state: &mut Self::State,
            node: Self::NodeId,
            input: LayoutInput,
        ) -> LayoutOutput {
            assert_eq!(node, TestKind::Child);
            match input.goal {
                LayoutGoal::Measure(_) => self
                    .child_measure_calls
                    .set(self.child_measure_calls.get() + 1),
                LayoutGoal::Commit => {
                    self.child_commit_calls
                        .set(self.child_commit_calls.get() + 1);
                    self.committed_artifact.set(true);
                }
            }
            let size = input.known_dimensions.unwrap_or(Size::new(200.0, 100.0));
            LayoutOutput::new(size, size)
        }
    }

    fn measure_input(known_dimensions: Size<Option<f32>>) -> LayoutInput {
        LayoutInput::measure(
            known_dimensions,
            Size::NONE,
            Size::MAX_CONTENT,
            RequestedAxis::Both,
        )
    }

    #[test]
    fn content_independent_measure_skips_the_entire_child_tree() {
        let tree = TestState::new(TestStyle::auto(), TestStyle::fixed(20.0, 10.0));
        let mut state = Default::default();

        let output = compute_relative_layout(
            &tree,
            &mut state,
            TestState::root(),
            measure_input(Size::new(Some(120.0), Some(80.0))),
        );

        assert_eq!(
            output,
            LayoutOutput::new(Size::new(120.0, 80.0), Size::new(120.0, 80.0))
        );
        assert_eq!(tree.children_calls.get(), 0);
        assert_eq!(tree.child_measure_calls.get(), 0);
        assert_eq!(tree.child_commit_calls.get(), 0);
    }

    #[test]
    fn size_contained_measure_skips_children_with_indefinite_outer_size() {
        let tree = TestState::new(
            TestStyle::size_contained(Some(42.0), None),
            TestStyle::fixed(20.0, 10.0),
        );
        let mut state = Default::default();

        let output = compute_relative_layout(
            &tree,
            &mut state,
            TestState::root(),
            measure_input(Size::NONE),
        );

        assert_eq!(
            output,
            LayoutOutput::new(Size::new(42.0, 0.0), Size::new(42.0, 0.0))
        );
        assert_eq!(tree.children_calls.get(), 0);
        assert_eq!(tree.child_measure_calls.get(), 0);
    }

    #[test]
    fn fully_known_item_skips_measure_but_still_commits_its_artifact() {
        let tree = TestState::new(TestStyle::auto(), TestStyle::fixed(20.0, 10.0));
        let mut state = Default::default();

        let measured = compute_relative_layout(
            &tree,
            &mut state,
            TestState::root(),
            measure_input(Size::NONE),
        );
        assert_eq!(measured.size, Size::new(20.0, 10.0));
        assert_eq!(tree.child_measure_calls.get(), 0);
        assert_eq!(tree.child_commit_calls.get(), 0);

        let committed = compute_relative_layout(
            &tree,
            &mut state,
            TestState::root(),
            LayoutInput::commit(
                Size::new(Some(100.0), Some(50.0)),
                Size::NONE,
                Size::new(
                    AvailableSpace::Definite(100.0),
                    AvailableSpace::Definite(50.0),
                ),
            ),
        );
        assert_eq!(committed.size, Size::new(100.0, 50.0));
        assert_eq!(tree.child_measure_calls.get(), 0);
        assert_eq!(tree.child_commit_calls.get(), 1);
        assert!(tree.committed_artifact.get());
        assert_eq!(
            state[TestKind::Child as usize].unrounded().size,
            Size::new(20.0, 10.0)
        );
    }

    #[test]
    fn fixed_dependencies_deduplicate_without_heap_sets() {
        let mut dependencies = Dependencies::EMPTY;
        dependencies.add(4);
        dependencies.add(4);
        dependencies.add(2);
        assert_eq!(dependencies.as_slice(), &[4, 2]);
    }

    #[test]
    fn contradictory_constraints_collapse_at_the_start_edge() {
        assert_eq!(
            constrained_border_size(Line::new(Some(20.0), Some(10.0)), 3.0),
            Some(0.0)
        );
    }
}
