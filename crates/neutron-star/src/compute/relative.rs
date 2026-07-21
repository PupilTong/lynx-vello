//! Starlight id-constrained relative layout.
//!
//! This is the standalone implementation of `display: relative`, not CSS
//! `position: relative`. Its direct in-flow children form a dependency graph
//! over integer ids and position physical margin edges relative to the parent
//! or sibling edges.

use stylo::computed_values::{box_sizing, direction, relative_center, relative_layout_once};
use stylo::values::computed::lynx_layout::RelativeReference;
use stylo::values::computed::{LengthPercentage, MaxSize, PositionProperty, Size as StyleSize};

use super::compute_absolute_layout;
use super::util::{
    ItemKey, OrderedItem, ResolvedContainerBox, ResolvedItemBox, box_inset_size, clamp_axis,
    preferred_size_definiteness, resolve_container_box, resolve_item_box_with_bases,
    resolve_length_percentage, sort_and_assign_layout_order, subtract_available_space,
    used_aspect_ratio,
};
use crate::geometry::{Edges, Line, Point, Size};
use crate::style::relative::{RELATIVE_REFERENCE_NONE, RELATIVE_REFERENCE_PARENT};
use crate::style::{CoreStyle, RelativeContainerStyle, RelativeItemStyle};
use crate::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis,
    SizingMode,
};

/// Whether a computed reference identifies another relative item (any value
/// other than the reserved `-1` none and `0` parent sentinels).
#[inline]
const fn reference_is_item(reference: RelativeReference) -> bool {
    reference != RELATIVE_REFERENCE_NONE && reference != RELATIVE_REFERENCE_PARENT
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    #[inline]
    fn size<T: Copy>(self, size: Size<T>) -> T {
        match self {
            Self::Horizontal => size.width,
            Self::Vertical => size.height,
        }
    }

    #[inline]
    fn set_size<T>(self, size: &mut Size<T>, value: T) {
        match self {
            Self::Horizontal => size.width = value,
            Self::Vertical => size.height = value,
        }
    }

    #[inline]
    fn position(self, positions: Size<Line<f32>>) -> Line<f32> {
        self.size(positions)
    }

    #[inline]
    fn set_position(self, positions: &mut Size<Line<f32>>, value: Line<f32>) {
        self.set_size(positions, value);
    }

    #[inline]
    fn margin_sum(self, margin: Edges<f32>) -> f32 {
        match self {
            Self::Horizontal => margin.horizontal_sum(),
            Self::Vertical => margin.vertical_sum(),
        }
    }

    #[inline]
    fn start_reference(self, edges: Edges<ResolvedReference>) -> ResolvedReference {
        match self {
            Self::Horizontal => edges.left,
            Self::Vertical => edges.top,
        }
    }

    #[inline]
    fn end_reference(self, edges: Edges<ResolvedReference>) -> ResolvedReference {
        match self {
            Self::Horizontal => edges.right,
            Self::Vertical => edges.bottom,
        }
    }

    #[inline]
    fn centers(self, center: relative_center::T) -> bool {
        match self {
            Self::Horizontal => matches!(
                center,
                relative_center::T::Horizontal | relative_center::T::Both
            ),
            Self::Vertical => matches!(
                center,
                relative_center::T::Vertical | relative_center::T::Both
            ),
        }
    }

    #[inline]
    const fn requested(self) -> RequestedAxis {
        match self {
            Self::Horizontal => RequestedAxis::Horizontal,
            Self::Vertical => RequestedAxis::Vertical,
        }
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
    key: ItemKey<N>,
    align: Edges<ResolvedReference>,
    adjacent: Edges<ResolvedReference>,
    center: relative_center::T,
    /// The item's positioning scheme. In-flow schemes lay out identically
    /// except that only `relative` applies the definite-inset visual nudge
    /// (`sticky` is nudged by the host at scroll time, `static` never).
    position: PositionProperty,
    preferred_size: Size<Option<f32>>,
    intrinsic_preferred_size: Size<Option<f32>>,
    intrinsic_sizes_ready: bool,
    preferred_size_is_definite: Size<bool>,
    min_size: Size<Option<f32>>,
    max_size: Size<Option<f32>>,
    box_sizing: box_sizing::T,
    margin: Edges<f32>,
    padding: Edges<f32>,
    border: Edges<f32>,
    inset: Edges<Option<f32>>,
    direction: direction::T,
    positions: Size<Line<f32>>,
    output: LayoutOutput,
    last_measure: Option<LayoutInput>,
    size_is_definite: Size<bool>,
    reuse_fixed_measurement: bool,
}

impl<N> RelativeItem<N> {
    #[inline]
    fn outer_size(&self, axis: Axis) -> f32 {
        axis.size(self.output.size) + axis.margin_sum(self.margin)
    }

    #[inline]
    fn box_floor(&self) -> Size<f32> {
        box_inset_size(self.padding, self.border)
    }

    #[inline]
    fn fixed_measurement_matches(&self, refreshed: &Self) -> bool {
        // Both sides resolve the same node's style within one flush, and
        // style is immutable for the whole flush, so the raw computed values
        // cannot differ — only the basis-dependent resolved fields can.
        self.preferred_size.width.is_some()
            && self.preferred_size.height.is_some()
            && self.preferred_size_is_definite.width
            && self.preferred_size_is_definite.height
            && self.preferred_size == refreshed.preferred_size
            && self.preferred_size_is_definite == refreshed.preferred_size_is_definite
            && self.min_size == refreshed.min_size
            && self.max_size == refreshed.max_size
            && self.box_sizing == refreshed.box_sizing
            && self.margin == refreshed.margin
            && self.padding == refreshed.padding
            && self.border == refreshed.border
    }
}

fn resolve_item<N>(
    key: ItemKey<N>,
    size_percentage_basis: Size<Option<f32>>,
    edge_inline_basis: Option<f32>,
    lookup: &IdLookup,
) -> RelativeItem<N>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let style = key.node.style();
    let ResolvedItemBox {
        raw_size,
        aspect_ratio,
        preferred_size,
        box_sizing,
        min_size,
        max_size,
        margin,
        padding,
        border,
        inset,
        ..
    } = resolve_item_box_with_bases(&style, size_percentage_basis, edge_inline_basis);
    let preferred_size_is_definite =
        preferred_size_definiteness(raw_size, size_percentage_basis, aspect_ratio);

    RelativeItem {
        key,
        align: style
            .relative_align()
            .map(|reference| ResolvedReference::resolve(reference, lookup)),
        adjacent: style
            .relative_adjacent()
            .map(|reference| ResolvedReference::resolve(reference, lookup)),
        center: style.relative_center(),
        position: style.position(),
        preferred_size,
        intrinsic_preferred_size: Size::NONE,
        intrinsic_sizes_ready: false,
        preferred_size_is_definite,
        min_size,
        max_size,
        box_sizing,
        margin,
        padding,
        border,
        inset,
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
    fn new<N>(items: &[OrderedItem<N>]) -> Self
    where
        N: LayoutNode,
        N::Style: RelativeContainerStyle + RelativeItemStyle,
    {
        let mut entries = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let relative_id = item.node.style().relative_id();
            if reference_is_item(relative_id) {
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
fn has_axis_dependencies<N>(item: &RelativeItem<N>, axis: Axis) -> bool
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let references = [
        axis.start_reference(item.align),
        axis.end_reference(item.align),
        axis.end_reference(item.adjacent),
        axis.start_reference(item.adjacent),
    ];
    references
        .into_iter()
        .any(|reference| reference.item_index_u32().is_some())
}

fn add_axis_dependencies<N>(item: &RelativeItem<N>, axis: Axis, dependencies: &mut Dependencies)
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let align_start = axis.start_reference(item.align);
    let align_end = axis.end_reference(item.align);
    // Adjacency is opposite-sided: right/bottom-of constrain start;
    // left/top-of constrain end.
    let after = axis.end_reference(item.adjacent);
    let before = axis.start_reference(item.adjacent);
    for reference in [align_start, align_end, after, before] {
        if let Some(index) = reference.item_index_u32() {
            dependencies.add(index);
        }
    }
}

/// Topological order with CSR reverse edges and a monotonic cycle cursor.
/// Every item has at most eight distinct dependencies, so graph construction
/// and sorting are `O(n + e)` after id lookup resolution.
fn dependency_order<N>(items: &[RelativeItem<N>], scope: DependencyScope) -> Vec<usize>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
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
        return (0..count).collect();
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
) -> Option<f32>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    if reference.is_parent() {
        return axis
            .size(parent_size)
            .map(|extent| if target_end { extent } else { 0.0 });
    }
    if !allow_item_references {
        return None;
    }
    reference.item_index().map(|index| {
        let position = axis.position(items[index].positions);
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
) -> Line<Option<f32>>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let align_start = axis.start_reference(item.align);
    let align_end = axis.end_reference(item.align);
    let after = axis.end_reference(item.adjacent);
    let before = axis.start_reference(item.adjacent);

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
) -> Size<Line<Option<f32>>>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
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
            let owner = axis.size(parent_size).or_else(|| available.into_option());
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

/// Whether resolving one axis of these raw sizing properties requires a
/// min-content probe (`min-content` or a `fit-content()` clamp).
#[inline]
fn needs_min_content(size: &StyleSize, min_size: &StyleSize, max_size: &MaxSize) -> bool {
    matches!(
        size,
        StyleSize::MinContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        min_size,
        StyleSize::MinContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        max_size,
        MaxSize::MinContent | MaxSize::FitContentFunction(_)
    )
}

/// Whether resolving one axis of these raw sizing properties requires a
/// max-content probe (`max-content` or a `fit-content()` clamp).
#[inline]
fn needs_max_content(size: &StyleSize, min_size: &StyleSize, max_size: &MaxSize) -> bool {
    matches!(
        size,
        StyleSize::MaxContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        min_size,
        StyleSize::MaxContent | StyleSize::FitContentFunction(_)
    ) || matches!(
        max_size,
        MaxSize::MaxContent | MaxSize::FitContentFunction(_)
    )
}

fn intrinsic_probe<N>(
    item: &RelativeItem<N>,
    axis: Axis,
    intrinsic_space: AvailableSpace,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) -> f32
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    // LayoutInput carries the containing space. The recursively dispatched
    // child owns its box model and removes its margins exactly once.
    let mut available = available_content;
    axis.set_size(&mut available, intrinsic_space);
    let mut input = LayoutInput::compute_size(Size::NONE, parent_size, available, axis.requested());
    input.sizing_mode = SizingMode::ContentSize;
    axis.size(item.key.node.compute_child_layout(input).size)
}

/// Resolves one `fit-content(limit)` axis against the probed contributions.
#[allow(clippy::too_many_arguments)]
fn fit_content_dimension(
    limit: &LengthPercentage,
    axis: Axis,
    min_content: Option<f32>,
    max_content: Option<f32>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
    box_sizing: box_sizing::T,
    box_floor: f32,
) -> f32 {
    let min_content = min_content.unwrap_or(0.0);
    let max_content = max_content.unwrap_or(min_content);
    let owner = axis
        .size(parent_size)
        .or_else(|| axis.size(available_content).into_option());
    let limit = resolve_length_percentage(limit, owner).map_or(max_content, |limit| {
        if box_sizing == box_sizing::T::ContentBox {
            limit + box_floor
        } else {
            limit
        }
    });
    max_content.min(limit.max(min_content))
}

/// Resolves one intrinsic preferred/minimum sizing axis; quantitative values
/// (auto, lengths, percentages, and the treated-as-auto keywords) yield
/// `None` and keep their already-resolved value.
#[allow(clippy::too_many_arguments)]
fn resolve_intrinsic_dimension(
    value: &StyleSize,
    axis: Axis,
    min_content: Option<f32>,
    max_content: Option<f32>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
    box_sizing: box_sizing::T,
    box_floor: f32,
) -> Option<f32> {
    match value {
        StyleSize::MinContent => min_content,
        StyleSize::MaxContent => max_content,
        StyleSize::FitContentFunction(limit) => Some(fit_content_dimension(
            &limit.0,
            axis,
            min_content,
            max_content,
            parent_size,
            available_content,
            box_sizing,
            box_floor,
        )),
        StyleSize::Auto
        | StyleSize::LengthPercentage(_)
        | StyleSize::FitContent
        | StyleSize::Stretch
        | StyleSize::WebkitFillAvailable => None,
        StyleSize::AnchorSizeFunction(_) | StyleSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

/// Resolves one intrinsic maximum sizing axis (`none` behaves as
/// quantitative).
#[allow(clippy::too_many_arguments)]
fn resolve_intrinsic_max_dimension(
    value: &MaxSize,
    axis: Axis,
    min_content: Option<f32>,
    max_content: Option<f32>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
    box_sizing: box_sizing::T,
    box_floor: f32,
) -> Option<f32> {
    match value {
        MaxSize::MinContent => min_content,
        MaxSize::MaxContent => max_content,
        MaxSize::FitContentFunction(limit) => Some(fit_content_dimension(
            &limit.0,
            axis,
            min_content,
            max_content,
            parent_size,
            available_content,
            box_sizing,
            box_floor,
        )),
        MaxSize::None
        | MaxSize::LengthPercentage(_)
        | MaxSize::FitContent
        | MaxSize::Stretch
        | MaxSize::WebkitFillAvailable => None,
        MaxSize::AnchorSizeFunction(_) | MaxSize::AnchorContainingCalcFunction(_) => {
            unreachable!("anchor sizing is pref-dead under the lynx feature")
        }
    }
}

fn prepare_intrinsic_sizes<N>(
    item: &mut RelativeItem<N>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    if item.intrinsic_sizes_ready {
        return;
    }

    // The borrowed raw values stay lent from this style view for the whole
    // resolution; recursive probes mutate only host-owned per-node slots.
    let style = item.key.node.style();
    let full_raw_size = style.size();
    let full_raw_min = style.min_size();
    let full_raw_max = style.max_size();
    for axis in Axis::ALL {
        let raw_size = axis.size(full_raw_size);
        let raw_min = axis.size(full_raw_min);
        let raw_max = axis.size(full_raw_max);
        let needs_min = needs_min_content(raw_size, raw_min, raw_max);
        let needs_max = needs_max_content(raw_size, raw_min, raw_max);
        if !needs_min && !needs_max {
            continue;
        }

        let min_content = needs_min.then(|| {
            intrinsic_probe(
                item,
                axis,
                AvailableSpace::MinContent,
                parent_size,
                available_content,
            )
        });
        let max_content = needs_max.then(|| {
            intrinsic_probe(
                item,
                axis,
                AvailableSpace::MaxContent,
                parent_size,
                available_content,
            )
        });
        let floor = axis.size(item.box_floor());
        let preferred = resolve_intrinsic_dimension(
            raw_size,
            axis,
            min_content,
            max_content,
            parent_size,
            available_content,
            item.box_sizing,
            floor,
        );
        let min = resolve_intrinsic_dimension(
            raw_min,
            axis,
            min_content,
            max_content,
            parent_size,
            available_content,
            item.box_sizing,
            floor,
        );
        let max = resolve_intrinsic_max_dimension(
            raw_max,
            axis,
            min_content,
            max_content,
            parent_size,
            available_content,
            item.box_sizing,
            floor,
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
            axis.set_size(
                &mut item.intrinsic_preferred_size,
                Some(clamp_axis(
                    preferred,
                    axis.size(item.min_size),
                    axis.size(item.max_size),
                    axis.size(floor),
                )),
            );
        }
    }
    item.intrinsic_sizes_ready = true;
}

/// Clamps one axis value by the item's resolved min/max and box floor.
#[inline]
fn clamped_item_axis<N>(item: &RelativeItem<N>, axis: Axis, value: f32, floor: Size<f32>) -> f32 {
    clamp_axis(
        value,
        axis.size(item.min_size),
        axis.size(item.max_size),
        axis.size(floor),
    )
}

fn measurement_input<N>(
    item: &RelativeItem<N>,
    constraints: Size<Line<Option<f32>>>,
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) -> LayoutInput
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let mut known_dimensions = Size::NONE;
    let mut constraint_definite = Size::new(false, false);
    // Keep this as containing space. Leaf and container entry points remove
    // their own margins when translating LayoutInput into content constraints.
    let mut available_space = available_content;
    let floor = item.box_floor();
    let style = item.key.node.style();
    let full_raw_size = style.size();

    for axis in Axis::ALL {
        let line = axis.size(constraints);
        let has_one_sided_constraint =
            matches!((line.start, line.end), (Some(_), None) | (None, Some(_)));
        let raw_size = axis.size(full_raw_size);
        let fit_content_needs_one_sided_measurement =
            matches!(raw_size, StyleSize::FitContentFunction(_)) && has_one_sided_constraint;
        let constrained = constrained_border_size(line, axis.margin_sum(item.margin))
            .map(|size| clamped_item_axis(item, axis, size, floor));
        let known_is_definite = constrained.is_some() || axis.size(item.preferred_size_is_definite);
        axis.set_size(&mut constraint_definite, known_is_definite);
        // Starlight clamps every incoming child constraint by min/max at
        // `LayoutObject::UpdateMeasure` before measuring, so a definite
        // preferred size never bypasses resolved bounds (intrinsic-keyword
        // bounds included) and content is measured at the clamped size.
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
            let margin_sum = axis.margin_sum(item.margin);
            let available = if fit_content_needs_one_sided_measurement {
                // Starlight resolves fit-content on the default
                // margin-stripped AtMost constraint before a one-sided
                // relative constraint changes it. Start subtracts from that
                // fitted limit, while end replaces it with the end
                // coordinate. Lift the result back to containing space so
                // the child can remove its own margins exactly once.
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
                        // Starlight replaces the default margin-stripped
                        // AtMost constraint with the physical end
                        // coordinate. Add the margins here because the
                        // child entry point will remove them while
                        // lowering LayoutInput.
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

    let mut input = LayoutInput::compute_size(
        known_dimensions,
        parent_size,
        available_space,
        RequestedAxis::Both,
    );
    // A dependency equation decides geometry, but an intrinsic keyword does
    // not become a definite percentage basis merely because its used size is
    // passed as a known dimension for this measurement.
    input.definite_dimensions = constraint_definite;
    input.sizing_mode = SizingMode::InherentSize;
    input
}

fn measure_item<N>(item: &mut RelativeItem<N>, input: LayoutInput)
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
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
    let mut output = item.key.node.compute_child_layout(input);
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
            // Intrinsic min/max values are resolved by Relative rather than
            // by the recursively-dispatched child. A horizontal clamp can
            // change wrapping and therefore height, so remeasure with that
            // width fixed while keeping any dependency-decided height.
            let mut refined = input;
            refined.known_dimensions.width = Some(clamped_width);
            refined.available_space.width = AvailableSpace::Definite(clamped_width);
            if !input.definite_dimensions.height {
                refined.known_dimensions.height = None;
                refined.available_space.height = input.available_space.height;
            }
            output = item.key.node.compute_child_layout(refined);
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
        item.preferred_size_is_definite.width || input.definite_dimensions.width,
        item.preferred_size_is_definite.height || input.definite_dimensions.height,
    );
}

fn position_from_constraints<N>(
    item: &RelativeItem<N>,
    axis: Axis,
    constraints: Line<Option<f32>>,
    bounds: Bounds,
) -> Line<f32>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let outer_size = item.outer_size(axis);
    match (constraints.start, constraints.end) {
        (Some(start), Some(end)) => Line::new(start, end.max(start)),
        (Some(start), None) => Line::new(start, start + outer_size),
        (None, Some(end)) => Line::new(end - outer_size, end),
        (None, None) => {
            let align_start = axis.start_reference(item.align);
            let align_end = axis.end_reference(item.align);
            if align_end.is_parent() {
                Line::new(bounds.max - outer_size, bounds.max)
            } else if align_start.is_parent() || !axis.centers(item.center) {
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
) -> Bounds
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let mut bounds = Bounds::new(axis.size(parent_size));
    for &index in order {
        let constraints = axis_constraints(&items[index], axis, parent_size, items, true);
        let position = position_from_constraints(&items[index], axis, constraints, bounds);
        axis.set_position(&mut items[index].positions, position);
        bounds.include(position);
    }
    bounds
}

fn measure_all<N>(
    items: &mut [RelativeItem<N>],
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
    allow_item_references: bool,
) where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    for index in 0..items.len() {
        prepare_intrinsic_sizes(&mut items[index], parent_size, available_content);
        let constraints = all_constraints(&items[index], parent_size, items, allow_item_references);
        let input = measurement_input(&items[index], constraints, parent_size, available_content);
        measure_item(&mut items[index], input);
    }
}

fn one_pass_layout<N>(
    items: &mut [RelativeItem<N>],
    order: &[usize],
    parent_size: Size<Option<f32>>,
    available_content: Size<AvailableSpace>,
) -> Size<Bounds>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let mut bounds = Size::new(
        Bounds::new(parent_size.width),
        Bounds::new(parent_size.height),
    );
    for &index in order {
        prepare_intrinsic_sizes(&mut items[index], parent_size, available_content);
        let constraints = all_constraints(&items[index], parent_size, items, true);
        let input = measurement_input(&items[index], constraints, parent_size, available_content);
        measure_item(&mut items[index], input);

        for axis in Axis::ALL {
            let axis_constraints = axis.size(constraints);
            let axis_bounds = axis.size(bounds);
            let position =
                position_from_constraints(&items[index], axis, axis_constraints, axis_bounds);
            axis.set_position(&mut items[index].positions, position);
            let mut updated = axis_bounds;
            updated.include(position);
            axis.set_size(&mut bounds, updated);
        }
    }
    bounds
}

fn refresh_item_bases<N>(
    items: &mut [RelativeItem<N>],
    size_percentage_basis: Size<Option<f32>>,
    edge_inline_basis: Option<f32>,
    lookup: &IdLookup,
) where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    for item in items {
        let positions = item.positions;
        let output = item.output;
        let last_measure = item.last_measure;
        let intrinsic_preferred_size = item.intrinsic_preferred_size;
        let intrinsic_sizes_ready = item.intrinsic_sizes_ready;
        let size_is_definite = item.size_is_definite;
        let mut refreshed =
            resolve_item(item.key, size_percentage_basis, edge_inline_basis, lookup);
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
fn two_pass_layout<N>(
    items: &mut [RelativeItem<N>],
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
) -> Size<f32>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    // Initial measurement only has parent-edge constraints. Sibling edges
    // become available as the separate axis orders are positioned.
    measure_all(items, initial_parent_size, available_content, false);
    let _ = position_axis(
        items,
        horizontal_order,
        Axis::Horizontal,
        initial_parent_size,
    );
    let _ = position_axis(items, vertical_order, Axis::Vertical, initial_parent_size);

    // Refine both-sided sibling constraints and selectively remeasure.
    measure_all(items, initial_parent_size, available_content, true);
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
        horizontal_bounds.extent(),
        box_inset.width,
        min_size.width,
        max_size.width,
    );
    let content_width = (outer_width - box_inset.width).max(0.0);
    let mut resolved_parent_size = initial_parent_size;

    if initial_parent_size.width.is_none() {
        resolved_parent_size.width = Some(content_width);
        available_content.width = AvailableSpace::Definite(content_width);

        // Percentages whose owner width was cyclic now resolve against the
        // content-sized width. Relative references and ids cannot change
        // during the immutable layout epoch, so the existing lookup remains
        // valid.
        refresh_item_bases(items, resolved_parent_size, Some(content_width), lookup);
        let _ = position_axis(
            items,
            horizontal_order,
            Axis::Horizontal,
            resolved_parent_size,
        );
        measure_all(items, resolved_parent_size, available_content, true);
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
        vertical_bounds.extent(),
        box_inset.height,
        min_size.height,
        max_size.height,
    );
    let content_height = (outer_height - box_inset.height).max(0.0);
    resolved_parent_size.height = Some(content_height);

    // Final positions see both final content extents. This intentionally does
    // not add another measurement round: relative-layout Level 1 only
    // repositions after final height determination.
    let _ = position_axis(
        items,
        horizontal_order,
        Axis::Horizontal,
        resolved_parent_size,
    );
    let _ = position_axis(items, vertical_order, Axis::Vertical, resolved_parent_size);

    Size::new(outer_width, outer_height)
}

#[inline]
fn relative_offset(inset: Edges<Option<f32>>, direction: direction::T) -> Point<f32> {
    let x = match (inset.left, inset.right) {
        (Some(left), Some(right)) => {
            if direction == direction::T::Rtl {
                -right
            } else {
                left
            }
        }
        (Some(left), None) => left,
        (None, Some(right)) => -right,
        (None, None) => 0.0,
    };
    Point::new(x, inset.top.unwrap_or_else(|| -inset.bottom.unwrap_or(0.0)))
}

fn commit_in_flow<N>(
    items: &mut [RelativeItem<N>],
    content_size: Size<f32>,
    content_origin: Point<f32>,
    container_size: Size<f32>,
) -> Size<f32>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let parent_size = content_size.map(Some);
    let available = content_size.map(AvailableSpace::Definite);
    let mut scrollable_size = container_size;

    for item in items {
        let mut input =
            LayoutInput::perform_layout(item.output.size.map(Some), parent_size, available);
        input.definite_dimensions = item.size_is_definite;
        input.sizing_mode = SizingMode::ContentSize;
        let output = item.key.node.compute_child_layout(input);
        item.output = output;

        // Only `relative` nudges at layout time; `static` has no offsets and
        // `sticky` is a host scroll-time post-pass.
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
        item.key.node.set_unrounded_layout(&layout);

        scrollable_size.width = scrollable_size
            .width
            .max(location.x + output.size.width.max(output.content_size.width));
        scrollable_size.height = scrollable_size
            .height
            .max(location.y + output.size.height.max(output.content_size.height));
    }
    scrollable_size
}

fn commit_out_of_flow<N>(
    items: &[OrderedItem<N>],
    container_size: Size<f32>,
    border: Edges<f32>,
) -> Size<f32>
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let padding_box_size = Size::new(
        (container_size.width - border.horizontal_sum()).max(0.0),
        (container_size.height - border.vertical_sum()).max(0.0),
    );
    let mut scrollable_size = container_size;
    for pending in items {
        let style = pending.node.style();
        match style.position() {
            PositionProperty::Absolute => {
                let mut layout =
                    compute_absolute_layout(pending.node, padding_box_size, Point::ZERO);
                layout.order = pending.layout_order;
                layout.location.x += border.left;
                layout.location.y += border.top;
                scrollable_size.width = scrollable_size
                    .width
                    .max(layout.location.x + layout.size.width.max(layout.content_size.width));
                scrollable_size.height = scrollable_size
                    .height
                    .max(layout.location.y + layout.size.height.max(layout.content_size.height));
                pending.node.set_unrounded_layout(&layout);
            }
            // The containing block is not the layout parent (CSS `fixed`):
            // record the static position; the host completes layout in its
            // positioned pass.
            PositionProperty::Fixed => {
                pending
                    .node
                    .set_static_position(Point::new(border.left, border.top));
            }
            PositionProperty::Static | PositionProperty::Relative | PositionProperty::Sticky => {}
        }
    }
    scrollable_size
}

/// Computes a Starlight relative-layout container.
///
/// Relative ids and physical-edge properties come from each handle's style
/// view (`N::Style: RelativeContainerStyle + RelativeItemStyle`); recursive
/// measurement and durable geometry writes flow through the [`LayoutNode`]
/// handles. The implementation uses compact sorted id lookup, fixed-width
/// dependency deduplication, CSR reverse edges, and a linear
/// Kahn/cycle-fallback traversal. Child layouts are stored only for
/// [`LayoutGoal::Commit`], and the container exports no baseline.
#[allow(clippy::too_many_lines)]
pub fn compute_relative_layout<N>(node: N, input: LayoutInput) -> LayoutOutput
where
    N: LayoutNode,
    N::Style: RelativeContainerStyle + RelativeItemStyle,
{
    let style = node.style();
    let layout_once = style.relative_layout_once() == relative_layout_once::T::True;
    let style_definite = if input.sizing_mode == SizingMode::ContentSize {
        Size::new(false, false)
    } else {
        preferred_size_definiteness(
            style.size(),
            input.parent_size,
            used_aspect_ratio(style.aspect_ratio()),
        )
    };
    let outer_definite = Size::new(
        input.definite_dimensions.width || style_definite.width,
        input.definite_dimensions.height || style_definite.height,
    );
    let ResolvedContainerBox {
        padding,
        border,
        box_inset,
        min: min_size,
        max: max_size,
        outer: initial_outer,
        inner: initial_inner,
        available_inner,
    } = resolve_container_box(&style, input);

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
    let edge_inline_basis = available_content.width.into_option();

    let child_count = node.child_count();
    let mut generated = Vec::with_capacity(child_count);
    let mut absolute_items = Vec::new();
    let mut hidden = Vec::new();
    for (document_index, child) in node.children().enumerate() {
        let child_style = child.style();
        if child_style.display().is_none() {
            hidden.push((document_index, child));
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
            absolute_items.push(pending);
        } else {
            generated.push(pending);
        }
    }

    sort_and_assign_layout_order(&mut generated, &mut absolute_items);

    let lookup = IdLookup::new(&generated);
    let mut items = generated
        .into_iter()
        .map(|item| resolve_item(item.key(), initial_parent_size, edge_inline_basis, &lookup))
        .collect::<Vec<_>>();

    let outer_size = if layout_once {
        let order = dependency_order(&items, DependencyScope::Combined);
        let bounds = one_pass_layout(&mut items, &order, initial_parent_size, available_content);
        Size::new(
            final_outer_axis(
                initial_outer.width,
                input.known_dimensions.width,
                bounds.width.extent(),
                box_inset.width,
                min_size.width,
                max_size.width,
            ),
            final_outer_axis(
                initial_outer.height,
                input.known_dimensions.height,
                bounds.height.extent(),
                box_inset.height,
                min_size.height,
                max_size.height,
            ),
        )
    } else {
        let horizontal_order = dependency_order(&items, DependencyScope::Horizontal);
        let vertical_order = dependency_order(&items, DependencyScope::Vertical);
        two_pass_layout(
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
        )
    };
    let content_size = Size::new(
        (outer_size.width - box_inset.width).max(0.0),
        (outer_size.height - box_inset.height).max(0.0),
    );
    if matches!(input.goal, LayoutGoal::Measure(_)) {
        return LayoutOutput::new(outer_size, outer_size);
    }

    let content_origin = Point::new(border.left + padding.left, border.top + padding.top);
    let mut scrollable_size = commit_in_flow(&mut items, content_size, content_origin, outer_size);
    for (document_index, child) in hidden {
        super::hide_subtree(child);
        child.set_unrounded_layout(&Layout::with_order(
            u32::try_from(document_index).unwrap_or(u32::MAX),
        ));
    }
    scrollable_size = scrollable_size.zip_map(
        commit_out_of_flow(&absolute_items, outer_size, border),
        f32::max,
    );

    LayoutOutput::new(outer_size, scrollable_size)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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
