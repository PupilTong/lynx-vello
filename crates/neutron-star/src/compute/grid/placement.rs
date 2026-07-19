//! Numeric CSS Grid placement (Grid §8).
//!
//! Line names carried by the stylo placement values are ignored (the engine
//! is numeric-lines-only), so this pass only has to handle numeric lines,
//! spans, and automatic positions. Coordinates are
//! zero-based track coordinates: explicit line `1` is coordinate `0`, and a
//! [`TrackSpan`] is half-open. Negative coordinates represent leading
//! implicit tracks.

use stylo::values::computed::{GridAutoFlow, GridLine};

use crate::geometry::Line;

/// One normalized side of a `grid-row` / `grid-column` placement.
///
/// Engine-private scratch decoded from stylo's [`GridLine`] by
/// [`grid_placement`]: `line_num == 0` means the number is absent, `is_span`
/// selects the span form, and the line name is ignored (the engine is
/// numeric-lines-only; hosts lower names before layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum GridPlacement {
    /// `auto`: placed by the auto-placement algorithm.
    #[default]
    Auto,
    /// A specific 1-based (possibly negative) line number. `0` is invalid
    /// per the CSS grammar and is decoded as [`GridPlacement::Auto`].
    Line(i32),
    /// `span <n>` relative to the opposite side. Values below one are
    /// clamped when the span is normalized (CSS treats `span 0` as invalid).
    Span(i32),
}

/// Decodes one stylo grid line into the engine's placement scratch.
///
/// A `span` carrying only an ident (no number) spans one track: the engine
/// cannot search named lines, and the parsed default span count is one.
pub(super) fn grid_placement(line: &GridLine) -> GridPlacement {
    if line.is_span {
        GridPlacement::Span(if line.line_num == 0 { 1 } else { line.line_num })
    } else if line.line_num == 0 {
        GridPlacement::Auto
    } else {
        GridPlacement::Line(line.line_num)
    }
}

const MIN_GRID_LINE: i32 = -10_000;
const MAX_GRID_LINE: i32 = 10_000;
const MAX_GRID_TRACKS: usize = (MAX_GRID_LINE - MIN_GRID_LINE) as usize;

/// A half-open sequence of tracks bounded by two resolved grid lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct TrackSpan {
    pub(super) start: i32,
    pub(super) end: i32,
}

impl TrackSpan {
    #[inline]
    fn len(self) -> usize {
        usize::try_from(self.end - self.start).unwrap_or(0)
    }

    #[inline]
    fn include(&mut self, other: Self) {
        self.start = self.start.min(other.start);
        self.end = self.end.max(other.end);
    }
}

/// The numeric placement styles needed for one grid item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PlacementInput {
    pub(super) column: Line<GridPlacement>,
    pub(super) row: Line<GridPlacement>,
}

/// A resolved two-dimensional grid area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct GridArea {
    pub(super) column: TrackSpan,
    pub(super) row: TrackSpan,
}

/// Result of resolving and auto-placing all in-flow items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlacementResult {
    /// Parallel to the input slice.
    pub(super) areas: Vec<GridArea>,
    /// Final explicit + implicit column-track coordinate range.
    pub(super) column_range: TrackSpan,
    /// Final explicit + implicit row-track coordinate range.
    pub(super) row_range: TrackSpan,
    /// Whether each track in `column_range` is covered by an item.
    pub(super) occupied_columns: Vec<bool>,
    /// Whether each track in `row_range` is covered by an item.
    pub(super) occupied_rows: Vec<bool>,
}

/// One axis after Grid §8.3 conflict handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AxisPlacement {
    /// Both bounding lines are known.
    Definite(TrackSpan),
    /// The position is automatic, with the given (numeric) span.
    Indefinite { span: usize },
}

impl AxisPlacement {
    #[inline]
    fn definite(self) -> Option<TrackSpan> {
        match self {
            Self::Definite(span) => Some(span),
            Self::Indefinite { .. } => None,
        }
    }

    #[inline]
    fn span(self) -> usize {
        match self {
            Self::Definite(tracks) => tracks.len().max(1),
            Self::Indefinite { span } => span,
        }
    }
}

/// Applies Grid §8.3 conflict handling and resolves numeric line indexes.
///
/// The returned definite area is already clamped to the supported grid-line
/// interval from Grid §5.4. An indefinite span is capped to the maximum
/// number of representable tracks; its final edge is clamped after automatic
/// placement chooses a start line.
pub(super) fn resolve_axis_placement(
    line: Line<GridPlacement>,
    explicit_tracks: usize,
) -> AxisPlacement {
    let start = normalized(line.start);
    let end = normalized(line.end);

    match (start, end) {
        (GridPlacement::Line(start), GridPlacement::Line(end)) => {
            let mut start = resolve_line(start, explicit_tracks);
            let mut end = resolve_line(end, explicit_tracks);
            if start > end {
                core::mem::swap(&mut start, &mut end);
            } else if start == end {
                // The end line is discarded, so the default span is one.
                end = end.saturating_add(1);
            }
            AxisPlacement::Definite(clamp_area(start, end))
        }
        (GridPlacement::Line(start), GridPlacement::Span(span)) => {
            let start = resolve_line(start, explicit_tracks);
            AxisPlacement::Definite(clamp_area(start, start + normalized_span_i64(span)))
        }
        (GridPlacement::Span(span), GridPlacement::Line(end)) => {
            let end = resolve_line(end, explicit_tracks);
            AxisPlacement::Definite(clamp_area(end - normalized_span_i64(span), end))
        }
        (GridPlacement::Line(start), GridPlacement::Auto) => {
            let start = resolve_line(start, explicit_tracks);
            AxisPlacement::Definite(clamp_area(start, start.saturating_add(1)))
        }
        (GridPlacement::Auto, GridPlacement::Line(end)) => {
            let end = resolve_line(end, explicit_tracks);
            AxisPlacement::Definite(clamp_area(end.saturating_sub(1), end))
        }
        // Two spans conflict: the end-side span is discarded. A lone
        // numeric span remains the span used by auto-placement.
        (GridPlacement::Span(span), GridPlacement::Span(_) | GridPlacement::Auto)
        | (GridPlacement::Auto, GridPlacement::Span(span)) => AxisPlacement::Indefinite {
            span: normalized_span(span),
        },
        (GridPlacement::Auto, GridPlacement::Auto) => AxisPlacement::Indefinite { span: 1 },
    }
}

/// Places items in order-modified document order using Grid §8.5.
// Keeping the four numbered spec passes together makes the cursor state and
// occupancy mutations auditable against §8.5.
#[allow(clippy::too_many_lines)]
pub(super) fn place_items(
    inputs: &[PlacementInput],
    explicit_columns: usize,
    explicit_rows: usize,
    flow: GridAutoFlow,
) -> PlacementResult {
    let row_flow = flow.contains(GridAutoFlow::ROW);
    let dense = flow.contains(GridAutoFlow::DENSE);
    let explicit_column_range = explicit_range(explicit_columns);
    let explicit_row_range = explicit_range(explicit_rows);

    if inputs.is_empty() {
        return finish_result(Vec::new(), explicit_column_range, explicit_row_range);
    }

    let mut items = Vec::with_capacity(inputs.len());
    for input in inputs {
        let column = resolve_axis_placement(input.column, explicit_columns);
        let row = resolve_axis_placement(input.row, explicit_rows);
        items.push(if row_flow {
            LogicalPlacement {
                primary: row,
                cross: column,
            }
        } else {
            LogicalPlacement {
                primary: column,
                cross: row,
            }
        });
    }

    let explicit_primary = if row_flow {
        explicit_row_range
    } else {
        explicit_column_range
    };
    let explicit_cross = if row_flow {
        explicit_column_range
    } else {
        explicit_row_range
    };

    // The primary range is fully known before step 4: auto-placement only
    // grows it endward. The final cross range additionally accounts for all
    // definite cross positions in step 3.
    let mut primary_range = explicit_primary;
    let mut all_cross_range = explicit_cross;
    let mut step_two_cross_range = explicit_cross;
    let mut locked_cross_span_sum = 0usize;
    let mut remaining_primary_span_sum = 0usize;
    let mut remaining_cross_span_max = 0usize;

    for item in &items {
        if let Some(primary) = item.primary.definite() {
            primary_range.include(primary);
        } else {
            remaining_primary_span_sum =
                remaining_primary_span_sum.saturating_add(item.primary.span());
        }

        if let Some(cross) = item.cross.definite() {
            all_cross_range.include(cross);
            if item.primary.definite().is_some() {
                // These are the items occupied by step 1 and therefore the
                // implicit-grid extent visible while step 2 is running.
                step_two_cross_range.include(cross);
            }
        } else if item.primary.definite().is_some() {
            locked_cross_span_sum = locked_cross_span_sum.saturating_add(item.cross.span());
        } else {
            remaining_cross_span_max = remaining_cross_span_max.max(item.cross.span());
        }
    }

    // Fully explicit grids permit overlap and need no occupancy search at
    // all. Besides accelerating a common case, this keeps far-apart §5.4
    // coordinates from materializing a mostly-empty bit matrix.
    if items
        .iter()
        .all(|item| item.primary.definite().is_some() && item.cross.definite().is_some())
    {
        let areas = items
            .iter()
            .map(|item| LogicalArea {
                primary: item.primary.definite().unwrap(),
                cross: item.cross.definite().unwrap(),
            })
            .map(|area| area.to_physical(row_flow))
            .collect();
        let (column_range, row_range) = if row_flow {
            (all_cross_range, primary_range)
        } else {
            (primary_range, all_cross_range)
        };
        return finish_result(areas, column_range, row_range);
    }

    // Allocate the occupancy matrix once. These are tight worst-case bounds:
    // definite occupied tracks can force each later item past their end, and
    // thereafter every item can add at most its own span.
    let occupancy_cross_min = all_cross_range.start.min(step_two_cross_range.start);
    let cross_after_locked = bounded_add(
        all_cross_range.end.max(step_two_cross_range.end),
        locked_cross_span_sum,
    );
    let cross_for_largest_span = bounded_add(occupancy_cross_min, remaining_cross_span_max);
    let occupancy_cross_max = all_cross_range
        .end
        .max(step_two_cross_range.end)
        .max(cross_after_locked)
        .max(cross_for_largest_span);
    let occupancy_primary_max = bounded_add(primary_range.end, remaining_primary_span_sum);

    debug_assert!(occupancy_cross_max > occupancy_cross_min);
    debug_assert!(occupancy_primary_max > primary_range.start);

    let mut occupancy = Occupancy::new(
        TrackSpan {
            start: primary_range.start,
            end: occupancy_primary_max,
        },
        TrackSpan {
            start: occupancy_cross_min,
            end: occupancy_cross_max,
        },
    );
    let mut logical_areas = vec![LogicalArea::default(); items.len()];
    let mut placed = vec![false; items.len()];

    // Step 1: fully definite items are allowed to overlap.
    for (index, item) in items.iter().copied().enumerate() {
        let (Some(primary), Some(cross)) = (item.primary.definite(), item.cross.definite()) else {
            continue;
        };
        let area = LogicalArea { primary, cross };
        occupancy.occupy(area);
        logical_areas[index] = area;
        placed[index] = true;
    }

    // Step 2: items locked to a primary track search in the cross axis. In
    // sparse mode, a range-max tree tracks the end cursor of every covered
    // primary track without repeatedly scanning wide spans.
    let has_locked_cross_item = items.iter().any(|item| {
        item.primary.definite().is_some() && matches!(item.cross, AxisPlacement::Indefinite { .. })
    });
    let mut sparse_locked_cursors = (!dense && has_locked_cross_item)
        .then(|| RangeMax::new(primary_range.len(), step_two_cross_range.start));
    let mut placed_by_step_two_end = step_two_cross_range.end;

    for (index, item) in items.iter().copied().enumerate() {
        if placed[index] {
            continue;
        }
        let (Some(primary), AxisPlacement::Indefinite { span }) =
            (item.primary.definite(), item.cross)
        else {
            continue;
        };

        // Step 2 runs before step 3 can add leading tracks for an unplaced
        // cross-definite item. Such later tracks must not make this item's
        // span grow startward when §5.4 truncates it at the end limit.
        let step_two_available =
            usize::try_from(MAX_GRID_LINE - step_two_cross_range.start).unwrap();
        let cross_span = span.min(step_two_available).max(1);
        let start = if let Some(cursors) = sparse_locked_cursors.as_mut() {
            let cursor_range = occupancy.primary_indices(primary);
            cursors.query(cursor_range.clone())
        } else {
            step_two_cross_range.start
        };
        let cross_start = occupancy
            .find_cross(primary, start, cross_span, occupancy.cross_range.end)
            .unwrap_or_else(|| occupancy.cross_range.end - usize_to_i32(cross_span));
        let area = LogicalArea {
            primary,
            cross: TrackSpan {
                start: cross_start,
                end: cross_start + usize_to_i32(cross_span),
            },
        };
        occupancy.occupy(area);
        if let Some(cursors) = sparse_locked_cursors.as_mut() {
            cursors.raise(occupancy.primary_indices(primary), area.cross.end);
        }
        placed_by_step_two_end = placed_by_step_two_end.max(area.cross.end);
        logical_areas[index] = area;
        placed[index] = true;
    }

    // Step 3: finish the fixed cross-axis implicit grid.
    let mut cross_range = all_cross_range;
    cross_range.end = cross_range.end.max(placed_by_step_two_end);
    if remaining_cross_span_max > cross_range.len() {
        cross_range.end = cross_range
            .end
            .max(bounded_add(cross_range.start, remaining_cross_span_max));
    }
    debug_assert!(cross_range.end <= occupancy.cross_range.end);

    // Step 4: place the remaining items. The cursor is persistent in sparse
    // mode and reset for every item in dense mode.
    let mut cursor_primary = primary_range.start;
    let mut cursor_cross = cross_range.start;

    for (index, item) in items.iter().copied().enumerate() {
        if placed[index] {
            continue;
        }

        let primary_span = item
            .primary
            .span()
            .min(occupancy.primary_range.len())
            .max(1);
        let area = if let Some(cross) = item.cross.definite() {
            let mut primary_start = if dense {
                primary_range.start
            } else {
                if cross.start < cursor_cross {
                    cursor_primary = cursor_primary.saturating_add(1);
                }
                cursor_cross = cross.start;
                cursor_primary
            };

            primary_start = occupancy
                .find_primary(primary_start, primary_span, cross)
                .unwrap_or_else(|| occupancy.primary_range.end - usize_to_i32(primary_span));
            if !dense {
                cursor_primary = primary_start;
            }
            LogicalArea {
                primary: TrackSpan {
                    start: primary_start,
                    end: primary_start + usize_to_i32(primary_span),
                },
                cross,
            }
        } else {
            let cross_span = item.cross.span().min(cross_range.len()).max(1);
            let mut primary_start = if dense {
                primary_range.start
            } else {
                cursor_primary
            };
            let mut cross_start = if dense {
                cross_range.start
            } else {
                cursor_cross
            };

            let found = loop {
                let primary_end = primary_start.saturating_add(usize_to_i32(primary_span));
                if primary_end > occupancy.primary_range.end {
                    break None;
                }
                let primary = TrackSpan {
                    start: primary_start,
                    end: primary_end,
                };
                if let Some(candidate) =
                    occupancy.find_cross(primary, cross_start, cross_span, cross_range.end)
                {
                    break Some((primary, candidate));
                }
                primary_start = primary_start.saturating_add(1);
                cross_start = cross_range.start;
            };

            let (primary, cross_start) = found.unwrap_or_else(|| {
                (
                    TrackSpan {
                        start: occupancy.primary_range.end - usize_to_i32(primary_span),
                        end: occupancy.primary_range.end,
                    },
                    cross_range.end - usize_to_i32(cross_span),
                )
            });
            if !dense {
                cursor_primary = primary.start;
                cursor_cross = cross_start;
            }
            LogicalArea {
                primary,
                cross: TrackSpan {
                    start: cross_start,
                    end: cross_start + usize_to_i32(cross_span),
                },
            }
        };

        occupancy.occupy(area);
        primary_range.end = primary_range.end.max(area.primary.end);
        logical_areas[index] = area;
        placed[index] = true;
    }

    debug_assert!(placed.iter().all(|placed| *placed));

    let areas = logical_areas
        .into_iter()
        .map(|area| area.to_physical(row_flow))
        .collect();
    let (column_range, row_range) = if row_flow {
        (cross_range, primary_range)
    } else {
        (primary_range, cross_range)
    };
    finish_result(areas, column_range, row_range)
}

#[derive(Debug, Clone, Copy)]
struct LogicalPlacement {
    primary: AxisPlacement,
    cross: AxisPlacement,
}

#[derive(Debug, Clone, Copy, Default)]
struct LogicalArea {
    primary: TrackSpan,
    cross: TrackSpan,
}

impl LogicalArea {
    #[inline]
    fn to_physical(self, row_flow: bool) -> GridArea {
        if row_flow {
            GridArea {
                column: self.cross,
                row: self.primary,
            }
        } else {
            GridArea {
                column: self.primary,
                row: self.cross,
            }
        }
    }
}

/// Flow-normalized, row-major occupancy matrix. Each primary stripe stores
/// its cross-axis cells contiguously, so the hot auto-placement search uses
/// word masks instead of walking previously placed items. Very large sparse
/// grids switch to sorted row intervals, avoiding a 50 MB eager bit matrix at
/// the §5.4 line limits.
#[derive(Debug)]
struct Occupancy {
    storage: OccupancyStorage,
    primary_range: TrackSpan,
    cross_range: TrackSpan,
    cross_len: usize,
}

const DENSE_OCCUPANCY_CELL_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
enum OccupancyStorage {
    Dense(Vec<u64>),
    /// Per-primary-track sorted, disjoint half-open cross-axis intervals.
    Sparse(Vec<Vec<(usize, usize)>>),
}

impl Occupancy {
    fn new(primary_range: TrackSpan, cross_range: TrackSpan) -> Self {
        let cross_len = cross_range.len();
        let cells = primary_range
            .len()
            .checked_mul(cross_len)
            .expect("clamped grid occupancy size fits usize");
        let storage = if cells <= DENSE_OCCUPANCY_CELL_LIMIT {
            let word_count = cells.div_ceil(u64::BITS as usize);
            OccupancyStorage::Dense(vec![0; word_count])
        } else {
            OccupancyStorage::Sparse(
                core::iter::repeat_with(Vec::new)
                    .take(primary_range.len())
                    .collect(),
            )
        };
        Self {
            storage,
            primary_range,
            cross_range,
            cross_len,
        }
    }

    #[inline]
    fn primary_indices(&self, span: TrackSpan) -> core::ops::Range<usize> {
        debug_assert!(span.start >= self.primary_range.start);
        debug_assert!(span.end <= self.primary_range.end);
        usize::try_from(span.start - self.primary_range.start).unwrap()
            ..usize::try_from(span.end - self.primary_range.start).unwrap()
    }

    #[inline]
    fn cross_offset(&self, coordinate: i32) -> usize {
        debug_assert!(coordinate >= self.cross_range.start);
        debug_assert!(coordinate <= self.cross_range.end);
        usize::try_from(coordinate - self.cross_range.start).unwrap()
    }

    fn occupy(&mut self, area: LogicalArea) {
        let cross_start = self.cross_offset(area.cross.start);
        let cross_end = self.cross_offset(area.cross.end);
        let primary = self.primary_indices(area.primary);
        match &mut self.storage {
            OccupancyStorage::Dense(words) => {
                for primary in primary {
                    let base = primary * self.cross_len;
                    set_bit_range(words, base + cross_start, base + cross_end);
                }
            }
            OccupancyStorage::Sparse(rows) => {
                for row in &mut rows[primary] {
                    insert_sparse_interval(row, cross_start, cross_end);
                }
            }
        }
    }

    /// Finds the first free cross-axis position and skips directly beyond the
    /// last colliding cell of each rejected candidate.
    fn find_cross(
        &self,
        primary: TrackSpan,
        start: i32,
        span: usize,
        cross_end: i32,
    ) -> Option<i32> {
        let span = usize_to_i32(span);
        let last_start = cross_end.checked_sub(span)?;
        let mut candidate = start.max(self.cross_range.start);
        while candidate <= last_start {
            let area_end = candidate + span;
            let Some(collision) = self.last_cross_collision(primary, candidate, area_end) else {
                return Some(candidate);
            };
            candidate = collision.saturating_add(1);
        }
        None
    }

    fn last_cross_collision(
        &self,
        primary: TrackSpan,
        cross_start: i32,
        cross_end: i32,
    ) -> Option<i32> {
        let start = self.cross_offset(cross_start);
        let end = self.cross_offset(cross_end);
        let mut last = None;
        match &self.storage {
            OccupancyStorage::Dense(words) => {
                for primary in self.primary_indices(primary) {
                    let base = primary * self.cross_len;
                    if let Some(bit) = last_set_bit(words, base + start, base + end) {
                        let coordinate = self.cross_range.start + usize_to_i32(bit - base);
                        last =
                            Some(last.map_or(coordinate, |current: i32| current.max(coordinate)));
                        if coordinate == cross_end - 1 {
                            break;
                        }
                    }
                }
            }
            OccupancyStorage::Sparse(rows) => {
                for primary in self.primary_indices(primary) {
                    if let Some(bit) = sparse_last_set(&rows[primary], start, end) {
                        let coordinate = self.cross_range.start + usize_to_i32(bit);
                        last =
                            Some(last.map_or(coordinate, |current: i32| current.max(coordinate)));
                        if coordinate == cross_end - 1 {
                            break;
                        }
                    }
                }
            }
        }
        last
    }

    /// Finds the first free primary position for a fixed cross-axis area.
    fn find_primary(&self, start: i32, span: usize, cross: TrackSpan) -> Option<i32> {
        let span = usize_to_i32(span);
        let last_start = self.primary_range.end.checked_sub(span)?;
        let mut candidate = start.max(self.primary_range.start);
        while candidate <= last_start {
            let area = TrackSpan {
                start: candidate,
                end: candidate + span,
            };
            let Some(collision) = self.last_primary_collision(area, cross) else {
                return Some(candidate);
            };
            candidate = collision.saturating_add(1);
        }
        None
    }

    fn last_primary_collision(&self, primary: TrackSpan, cross: TrackSpan) -> Option<i32> {
        let cross_start = self.cross_offset(cross.start);
        let cross_end = self.cross_offset(cross.end);
        match &self.storage {
            OccupancyStorage::Dense(words) => {
                for primary_index in self.primary_indices(primary).rev() {
                    let base = primary_index * self.cross_len;
                    if bit_range_any(words, base + cross_start, base + cross_end) {
                        return Some(self.primary_range.start + usize_to_i32(primary_index));
                    }
                }
            }
            OccupancyStorage::Sparse(rows) => {
                for primary_index in self.primary_indices(primary).rev() {
                    if sparse_range_any(&rows[primary_index], cross_start, cross_end) {
                        return Some(self.primary_range.start + usize_to_i32(primary_index));
                    }
                }
            }
        }
        None
    }
}

fn insert_sparse_interval(intervals: &mut Vec<(usize, usize)>, start: usize, end: usize) {
    debug_assert!(start < end);
    let first = intervals.partition_point(|&(_, interval_end)| interval_end < start);
    let mut merged_start = start;
    let mut merged_end = end;
    let mut after = first;
    while after < intervals.len() && intervals[after].0 <= merged_end {
        merged_start = merged_start.min(intervals[after].0);
        merged_end = merged_end.max(intervals[after].1);
        after += 1;
    }
    if first == after {
        intervals.insert(first, (merged_start, merged_end));
    } else {
        intervals[first] = (merged_start, merged_end);
        intervals.drain(first + 1..after);
    }
}

#[inline]
fn sparse_last_set(intervals: &[(usize, usize)], start: usize, end: usize) -> Option<usize> {
    debug_assert!(start < end);
    let candidate = intervals.partition_point(|&(interval_start, _)| interval_start < end);
    let &(_, interval_end) = candidate
        .checked_sub(1)
        .and_then(|index| intervals.get(index))?;
    (interval_end > start).then(|| interval_end.min(end) - 1)
}

#[inline]
fn sparse_range_any(intervals: &[(usize, usize)], start: usize, end: usize) -> bool {
    sparse_last_set(intervals, start, end).is_some()
}

/// Range-`chmax`/range-maximum tree used by sparse step-2 cursors.
#[derive(Debug)]
struct RangeMax {
    len: usize,
    size: usize,
    maximum: Vec<i32>,
    lazy: Vec<i32>,
}

impl RangeMax {
    fn new(len: usize, initial: i32) -> Self {
        let size = len.max(1).next_power_of_two();
        Self {
            len,
            size,
            maximum: vec![initial; size * 2],
            lazy: vec![i32::MIN; size],
        }
    }

    fn query(&mut self, range: core::ops::Range<usize>) -> i32 {
        debug_assert!(range.start < range.end && range.end <= self.len);
        self.query_inner(1, 0, self.size, range.start, range.end)
    }

    fn query_inner(
        &mut self,
        node: usize,
        left: usize,
        right: usize,
        query_left: usize,
        query_right: usize,
    ) -> i32 {
        if query_left <= left && right <= query_right {
            return self.maximum[node];
        }
        self.push(node);
        let middle = left.midpoint(right);
        let mut result = i32::MIN;
        if query_left < middle {
            result = self.query_inner(node * 2, left, middle, query_left, query_right);
        }
        if query_right > middle {
            result =
                result.max(self.query_inner(node * 2 + 1, middle, right, query_left, query_right));
        }
        result
    }

    fn raise(&mut self, range: core::ops::Range<usize>, value: i32) {
        debug_assert!(range.start < range.end && range.end <= self.len);
        self.raise_inner(1, 0, self.size, range.start, range.end, value);
    }

    fn raise_inner(
        &mut self,
        node: usize,
        left: usize,
        right: usize,
        update_left: usize,
        update_right: usize,
        value: i32,
    ) {
        if update_left <= left && right <= update_right {
            self.apply(node, value);
            return;
        }
        self.push(node);
        let middle = left.midpoint(right);
        if update_left < middle {
            self.raise_inner(node * 2, left, middle, update_left, update_right, value);
        }
        if update_right > middle {
            self.raise_inner(
                node * 2 + 1,
                middle,
                right,
                update_left,
                update_right,
                value,
            );
        }
        self.maximum[node] = self.maximum[node * 2].max(self.maximum[node * 2 + 1]);
    }

    #[inline]
    fn apply(&mut self, node: usize, value: i32) {
        self.maximum[node] = self.maximum[node].max(value);
        if node < self.size {
            self.lazy[node] = self.lazy[node].max(value);
        }
    }

    #[inline]
    fn push(&mut self, node: usize) {
        if node >= self.size {
            return;
        }
        let value = self.lazy[node];
        if value != i32::MIN {
            self.apply(node * 2, value);
            self.apply(node * 2 + 1, value);
            self.lazy[node] = i32::MIN;
        }
    }
}

#[inline]
fn normalized(placement: GridPlacement) -> GridPlacement {
    match placement {
        // `grid_placement` never produces a zero line, but placements are
        // also built directly by in-crate callers; stay defensive.
        GridPlacement::Line(0) => GridPlacement::Auto,
        other => other,
    }
}

#[inline]
fn normalized_span(span: i32) -> usize {
    usize::try_from(span.max(1))
        .expect("span is clamped to at least one")
        .min(MAX_GRID_TRACKS)
}

#[inline]
fn normalized_span_i64(span: i32) -> i64 {
    i64::try_from(normalized_span(span)).unwrap()
}

#[inline]
fn resolve_line(line: i32, explicit_tracks: usize) -> i64 {
    let explicit_tracks = i64::try_from(explicit_tracks).unwrap_or(i64::MAX);
    let line = i64::from(line);
    if line > 0 {
        line - 1
    } else {
        explicit_tracks.saturating_add(line).saturating_add(1)
    }
}

#[inline]
fn clamp_area(start: i64, end: i64) -> TrackSpan {
    debug_assert!(start < end);
    let minimum = i64::from(MIN_GRID_LINE);
    let maximum = i64::from(MAX_GRID_LINE);
    if end <= minimum {
        return TrackSpan {
            start: MIN_GRID_LINE,
            end: MIN_GRID_LINE + 1,
        };
    }
    if start >= maximum {
        return TrackSpan {
            start: MAX_GRID_LINE - 1,
            end: MAX_GRID_LINE,
        };
    }
    TrackSpan {
        start: i32::try_from(start.max(minimum)).unwrap(),
        end: i32::try_from(end.min(maximum)).unwrap(),
    }
}

#[inline]
fn explicit_range(tracks: usize) -> TrackSpan {
    TrackSpan {
        start: 0,
        end: usize_to_i32(tracks.min(MAX_GRID_LINE as usize)),
    }
}

#[inline]
fn bounded_add(start: i32, amount: usize) -> i32 {
    let amount = i64::try_from(amount).unwrap_or(i64::MAX);
    i32::try_from((i64::from(start) + amount).min(i64::from(MAX_GRID_LINE))).unwrap()
}

#[inline]
fn usize_to_i32(value: usize) -> i32 {
    i32::try_from(value).expect("grid track count is clamped to 20,000")
}

fn finish_result(
    areas: Vec<GridArea>,
    column_range: TrackSpan,
    row_range: TrackSpan,
) -> PlacementResult {
    let occupied_columns = occupied_tracks(&areas, column_range, |area| area.column);
    let occupied_rows = occupied_tracks(&areas, row_range, |area| area.row);
    PlacementResult {
        areas,
        column_range,
        row_range,
        occupied_columns,
        occupied_rows,
    }
}

fn occupied_tracks<Select>(areas: &[GridArea], range: TrackSpan, select: Select) -> Vec<bool>
where
    Select: Fn(&GridArea) -> TrackSpan,
{
    let len = range.len();
    if len == 0 {
        return Vec::new();
    }
    let mut differences = vec![0_i64; len + 1];
    for area in areas {
        let span = select(area);
        let start = usize::try_from(span.start - range.start).unwrap();
        let end = usize::try_from(span.end - range.start).unwrap();
        differences[start] += 1;
        differences[end] -= 1;
    }
    let mut occupied = Vec::with_capacity(len);
    let mut covering = 0_i64;
    for difference in differences.into_iter().take(len) {
        covering += difference;
        occupied.push(covering > 0);
    }
    occupied
}

#[inline]
fn bit_mask(start: usize, end: usize) -> u64 {
    debug_assert!(start < end && end <= u64::BITS as usize);
    let lower = u64::MAX << start;
    let upper = if end == u64::BITS as usize {
        u64::MAX
    } else {
        (1_u64 << end) - 1
    };
    lower & upper
}

fn set_bit_range(words: &mut [u64], start: usize, end: usize) {
    debug_assert!(start < end);
    let first_word = start / u64::BITS as usize;
    let last_word = (end - 1) / u64::BITS as usize;
    let first_bit = start % u64::BITS as usize;
    let last_bit = (end - 1) % u64::BITS as usize + 1;
    if first_word == last_word {
        words[first_word] |= bit_mask(first_bit, last_bit);
        return;
    }
    words[first_word] |= bit_mask(first_bit, u64::BITS as usize);
    words[first_word + 1..last_word].fill(u64::MAX);
    words[last_word] |= bit_mask(0, last_bit);
}

fn bit_range_any(words: &[u64], start: usize, end: usize) -> bool {
    debug_assert!(start < end);
    let first_word = start / u64::BITS as usize;
    let last_word = (end - 1) / u64::BITS as usize;
    let first_bit = start % u64::BITS as usize;
    let last_bit = (end - 1) % u64::BITS as usize + 1;
    if first_word == last_word {
        return words[first_word] & bit_mask(first_bit, last_bit) != 0;
    }
    words[first_word] & bit_mask(first_bit, u64::BITS as usize) != 0
        || words[first_word + 1..last_word]
            .iter()
            .any(|word| *word != 0)
        || words[last_word] & bit_mask(0, last_bit) != 0
}

fn last_set_bit(words: &[u64], start: usize, end: usize) -> Option<usize> {
    debug_assert!(start < end);
    let first_word = start / u64::BITS as usize;
    let last_word = (end - 1) / u64::BITS as usize;
    let first_bit = start % u64::BITS as usize;
    let last_bit = (end - 1) % u64::BITS as usize + 1;

    let last_mask = if first_word == last_word {
        bit_mask(first_bit, last_bit)
    } else {
        bit_mask(0, last_bit)
    };
    let last_value = words[last_word] & last_mask;
    if last_value != 0 {
        return Some(
            last_word * u64::BITS as usize + (u64::BITS - 1 - last_value.leading_zeros()) as usize,
        );
    }
    if first_word == last_word {
        return None;
    }
    for word_index in (first_word + 1..last_word).rev() {
        let value = words[word_index];
        if value != 0 {
            return Some(
                word_index * u64::BITS as usize + (u64::BITS - 1 - value.leading_zeros()) as usize,
            );
        }
    }
    let first_value = words[first_word] & bit_mask(first_bit, u64::BITS as usize);
    (first_value != 0).then(|| {
        first_word * u64::BITS as usize + (u64::BITS - 1 - first_value.leading_zeros()) as usize
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto() -> GridPlacement {
        GridPlacement::Auto
    }

    fn line(value: i32) -> GridPlacement {
        GridPlacement::Line(value)
    }

    fn span(value: i32) -> GridPlacement {
        GridPlacement::Span(value)
    }

    fn stylo_line(line_num: i32, is_span: bool) -> GridLine {
        let mut line = GridLine::auto();
        line.line_num = line_num;
        line.is_span = is_span;
        line
    }

    #[test]
    fn stylo_grid_lines_decode_to_placements() {
        assert_eq!(grid_placement(&GridLine::auto()), GridPlacement::Auto);
        assert_eq!(grid_placement(&stylo_line(2, false)), GridPlacement::Line(2));
        assert_eq!(
            grid_placement(&stylo_line(-3, false)),
            GridPlacement::Line(-3)
        );
        assert_eq!(grid_placement(&stylo_line(3, true)), GridPlacement::Span(3));
        // `span <ident>` carries no number; the engine spans one track.
        assert_eq!(grid_placement(&stylo_line(0, true)), GridPlacement::Span(1));
    }

    fn input(
        column_start: GridPlacement,
        column_end: GridPlacement,
        row_start: GridPlacement,
        row_end: GridPlacement,
    ) -> PlacementInput {
        PlacementInput {
            column: Line::new(column_start, column_end),
            row: Line::new(row_start, row_end),
        }
    }

    #[test]
    fn resolves_lines_spans_and_conflicts() {
        assert_eq!(
            resolve_axis_placement(Line::new(line(1), line(-1)), 4),
            AxisPlacement::Definite(TrackSpan { start: 0, end: 4 })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(line(3), line(1)), 4),
            AxisPlacement::Definite(TrackSpan { start: 0, end: 2 })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(line(2), line(2)), 4),
            AxisPlacement::Definite(TrackSpan { start: 1, end: 2 })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(line(2), span(3)), 4),
            AxisPlacement::Definite(TrackSpan { start: 1, end: 4 })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(span(2), line(4)), 4),
            AxisPlacement::Definite(TrackSpan { start: 1, end: 3 })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(span(2), span(5)), 4),
            AxisPlacement::Indefinite { span: 2 }
        );
    }

    #[test]
    fn sparse_keeps_holes_while_dense_backfills() {
        let inputs = [
            input(auto(), span(2), auto(), auto()),
            input(auto(), span(2), auto(), auto()),
            input(auto(), auto(), auto(), auto()),
        ];
        let sparse = place_items(&inputs, 3, 0, GridAutoFlow::ROW);
        let dense = place_items(&inputs, 3, 0, GridAutoFlow::ROW | GridAutoFlow::DENSE);

        assert_eq!(
            sparse.areas[2],
            GridArea {
                column: TrackSpan { start: 2, end: 3 },
                row: TrackSpan { start: 1, end: 2 },
            }
        );
        assert_eq!(
            dense.areas[2],
            GridArea {
                column: TrackSpan { start: 2, end: 3 },
                row: TrackSpan { start: 0, end: 1 },
            }
        );
    }

    #[test]
    fn row_and_column_flow_swap_the_search_axes() {
        let inputs = [
            input(auto(), auto(), auto(), auto()),
            input(auto(), auto(), auto(), auto()),
        ];
        let rows = place_items(&inputs, 2, 2, GridAutoFlow::ROW);
        let columns = place_items(&inputs, 2, 2, GridAutoFlow::COLUMN);
        assert_eq!(rows.areas[0].column, TrackSpan { start: 0, end: 1 });
        assert_eq!(rows.areas[1].column, TrackSpan { start: 1, end: 2 });
        assert_eq!(rows.areas[1].row, TrackSpan { start: 0, end: 1 });
        assert_eq!(columns.areas[0].row, TrackSpan { start: 0, end: 1 });
        assert_eq!(columns.areas[1].row, TrackSpan { start: 1, end: 2 });
        assert_eq!(columns.areas[1].column, TrackSpan { start: 0, end: 1 });
    }

    #[test]
    fn leading_implicit_tracks_participate_in_auto_placement() {
        let inputs = [
            input(line(-5), line(-4), line(1), line(2)),
            input(auto(), auto(), auto(), auto()),
        ];
        let result = place_items(&inputs, 2, 1, GridAutoFlow::ROW);
        assert_eq!(result.column_range, TrackSpan { start: -2, end: 2 });
        assert_eq!(result.areas[0].column, TrackSpan { start: -2, end: -1 });
        assert_eq!(result.areas[1].column, TrackSpan { start: -1, end: 0 });
        assert_eq!(result.occupied_columns, [true, true, false, false]);
    }

    #[test]
    fn automatic_spans_cover_both_axes() {
        let result = place_items(
            &[input(auto(), span(2), auto(), span(2))],
            3,
            2,
            GridAutoFlow::ROW,
        );
        assert_eq!(
            result.areas[0],
            GridArea {
                column: TrackSpan { start: 0, end: 2 },
                row: TrackSpan { start: 0, end: 2 },
            }
        );
    }

    #[test]
    fn explicit_items_may_overlap() {
        let placed = input(line(1), line(3), line(1), line(2));
        let result = place_items(&[placed, placed], 2, 1, GridAutoFlow::ROW);
        assert_eq!(result.areas[0], result.areas[1]);
        assert_eq!(result.occupied_columns, [true, true]);
        assert_eq!(result.occupied_rows, [true]);
    }

    #[test]
    fn clamps_areas_to_supported_grid_lines() {
        assert_eq!(
            resolve_axis_placement(Line::new(line(1_000_000), auto()), 1),
            AxisPlacement::Definite(TrackSpan {
                start: 9_999,
                end: 10_000,
            })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(line(-1_000_000), auto()), 1),
            AxisPlacement::Definite(TrackSpan {
                start: -10_000,
                end: -9_999,
            })
        );
        assert_eq!(
            resolve_axis_placement(Line::new(line(1), span(i32::MAX)), 1),
            AxisPlacement::Definite(TrackSpan {
                start: 0,
                end: 10_000,
            })
        );

        // The leading implicit track is introduced in step 3. It must not
        // move the step-2 item's oversized automatic span startward.
        let result = place_items(
            &[
                input(auto(), span(i32::MAX), line(1), line(2)),
                input(line(-10_001), line(-10_000), auto(), auto()),
            ],
            0,
            1,
            GridAutoFlow::ROW,
        );
        assert_eq!(
            result.areas[0].column,
            TrackSpan {
                start: 0,
                end: 10_000,
            }
        );
    }

    #[test]
    fn bit_ranges_cross_word_and_row_boundaries() {
        let mut words = vec![0; 4];
        set_bit_range(&mut words, 61, 131);
        assert!(!bit_range_any(&words, 0, 61));
        assert!(bit_range_any(&words, 61, 131));
        assert_eq!(last_set_bit(&words, 0, 160), Some(130));
        assert!(!bit_range_any(&words, 131, 160));
    }

    #[test]
    fn oversized_occupancy_uses_sparse_merged_intervals() {
        let mut occupancy = Occupancy::new(
            TrackSpan { start: 0, end: 500 },
            TrackSpan {
                start: -10_000,
                end: 10_000,
            },
        );
        assert!(matches!(occupancy.storage, OccupancyStorage::Sparse(_)));

        occupancy.occupy(LogicalArea {
            primary: TrackSpan { start: 7, end: 9 },
            cross: TrackSpan {
                start: -20,
                end: 30,
            },
        });
        occupancy.occupy(LogicalArea {
            primary: TrackSpan { start: 7, end: 8 },
            cross: TrackSpan { start: 30, end: 45 },
        });

        assert_eq!(
            occupancy.last_cross_collision(TrackSpan { start: 7, end: 8 }, -100, 100,),
            Some(44)
        );
        assert_eq!(
            occupancy.last_primary_collision(
                TrackSpan { start: 0, end: 20 },
                TrackSpan { start: 0, end: 1 },
            ),
            Some(8)
        );
        assert_eq!(
            occupancy.find_cross(TrackSpan { start: 7, end: 9 }, -20, 10, 100),
            Some(45)
        );
    }
}
