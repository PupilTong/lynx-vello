//! The engine-provided per-node measurement cache.

use smallvec::SmallVec;

use crate::geometry::{Point, Size};
use crate::tree::{
    AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

/// Constraint-shape buckets the last-resort eviction hashes into.
pub const MEASURE_CACHE_SLOTS: usize = 8;

/// The hard ceiling on live measurements for one node.
///
/// Flex, Linear and Relative containers ask a child for at most a handful of
/// distinct constraints, so they never leave the inline slots. Grid's two-axis
/// track sizing asks for far more — a single-child `display: grid` chain needs
/// thirteen live entries per node — and evicting one of those turns every later
/// re-request into a full subtree recomputation, which is quadratic in the
/// number of grid levels. Growing on the heap up to this ceiling keeps that
/// cost linear and is paid only by the nodes that need it.
const MEASURE_CACHE_LIMIT: usize = 32;

/// Inline measurement capacity. Most nodes (fixed-size leaves, committed flex
/// items) record at most a couple of measurement shapes, so the cache keeps
/// two entries inline and spills to the heap only for the nodes whose
/// containers actually probe many constraint shapes.
const MEASURE_CACHE_INLINE: usize = 2;

const KNOWN_WIDTH_PRESENT: u32 = 1 << 0;
const KNOWN_HEIGHT_PRESENT: u32 = 1 << 1;
const PARENT_WIDTH_PRESENT: u32 = 1 << 2;
const PARENT_HEIGHT_PRESENT: u32 = 1 << 3;
const OPTIONAL_SIZE_PRESENCE: u32 =
    KNOWN_WIDTH_PRESENT | KNOWN_HEIGHT_PRESENT | PARENT_WIDTH_PRESENT | PARENT_HEIGHT_PRESENT;
const AVAILABLE_WIDTH_SHIFT: u32 = 4;
const AVAILABLE_HEIGHT_SHIFT: u32 = 6;
const AVAILABLE_TAG_MASK: u32 = 0b11;
const AVAILABLE_DEFINITE: u32 = 0;
const AVAILABLE_MIN_CONTENT: u32 = 1;
const AVAILABLE_MAX_CONTENT: u32 = 2;
const GOAL_SHIFT: u32 = 8;
const GOAL_MASK: u32 = 0b11;
const GOAL_COMMIT: u32 = 0;
const GOAL_HORIZONTAL: u32 = 1;
const GOAL_VERTICAL: u32 = 2;
const GOAL_BOTH: u32 = 3;
const IGNORE_SIZE_STYLES: u32 = 1 << 10;
const DEFINITE_WIDTH: u32 = 1 << 11;
const DEFINITE_HEIGHT: u32 = 1 << 12;
const EXACT_INPUT_FLAGS: u32 =
    OPTIONAL_SIZE_PRESENCE | IGNORE_SIZE_STYLES | DEFINITE_WIDTH | DEFINITE_HEIGHT;
const BASELINE_X_PRESENT: u32 = 1 << 13;
const BASELINE_Y_PRESENT: u32 = 1 << 14;
const BASELINE_PRESENCE: u32 = BASELINE_X_PRESENT | BASELINE_Y_PRESENT;
/// The independence a `LayoutGoal::Commit` carries, per axis. Like the
/// baseline-presence bits these ride in `flags` but stay out of `INPUT_FLAGS`
/// and `EXACT_INPUT_FLAGS`: independence never changes computed geometry, so
/// two commits that differ only in it answer each other from the cache.
const INDEPENDENT_WIDTH: u32 = 1 << 15;
const INDEPENDENT_HEIGHT: u32 = 1 << 16;
const INPUT_FLAGS: u32 = EXACT_INPUT_FLAGS
    | (AVAILABLE_TAG_MASK << AVAILABLE_WIDTH_SHIFT)
    | (AVAILABLE_TAG_MASK << AVAILABLE_HEIGHT_SHIFT)
    | (GOAL_MASK << GOAL_SHIFT);

/// A lossless compact representation of [`LayoutInput`]. Every field round
/// trips, including the `LayoutGoal::Commit` independence payload, which rides
/// in `flags` outside the key bits.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PackedLayoutInput {
    values: [f32; 6],
    flags: u32,
}

impl PackedLayoutInput {
    #[inline]
    fn new(input: LayoutInput) -> Self {
        let mut values = [0.0; 6];
        let mut flags = 0;
        pack_option(
            input.known_dimensions.width,
            &mut values[0],
            &mut flags,
            KNOWN_WIDTH_PRESENT,
        );
        pack_option(
            input.known_dimensions.height,
            &mut values[1],
            &mut flags,
            KNOWN_HEIGHT_PRESENT,
        );
        pack_option(
            input.parent_size.width,
            &mut values[2],
            &mut flags,
            PARENT_WIDTH_PRESENT,
        );
        pack_option(
            input.parent_size.height,
            &mut values[3],
            &mut flags,
            PARENT_HEIGHT_PRESENT,
        );
        pack_available_space(
            input.available_space.width,
            &mut values[4],
            &mut flags,
            AVAILABLE_WIDTH_SHIFT,
        );
        pack_available_space(
            input.available_space.height,
            &mut values[5],
            &mut flags,
            AVAILABLE_HEIGHT_SHIFT,
        );

        let goal_tag = match input.goal {
            LayoutGoal::Commit {
                content_independent,
            } => {
                if content_independent.width {
                    flags |= INDEPENDENT_WIDTH;
                }
                if content_independent.height {
                    flags |= INDEPENDENT_HEIGHT;
                }
                GOAL_COMMIT
            }
            LayoutGoal::Measure(RequestedAxis::Horizontal) => GOAL_HORIZONTAL,
            LayoutGoal::Measure(RequestedAxis::Vertical) => GOAL_VERTICAL,
            LayoutGoal::Measure(RequestedAxis::Both) => GOAL_BOTH,
        };
        flags |= goal_tag << GOAL_SHIFT;
        if input.sizing_mode == SizingMode::IgnoreSizeStyles {
            flags |= IGNORE_SIZE_STYLES;
        }
        if input.definite_dimensions.width {
            flags |= DEFINITE_WIDTH;
        }
        if input.definite_dimensions.height {
            flags |= DEFINITE_HEIGHT;
        }

        Self { values, flags }
    }

    #[inline]
    fn unpack(self) -> LayoutInput {
        LayoutInput {
            goal: match (self.flags >> GOAL_SHIFT) & GOAL_MASK {
                GOAL_COMMIT => LayoutGoal::Commit {
                    content_independent: Size::new(
                        self.flags & INDEPENDENT_WIDTH != 0,
                        self.flags & INDEPENDENT_HEIGHT != 0,
                    ),
                },
                GOAL_HORIZONTAL => LayoutGoal::Measure(RequestedAxis::Horizontal),
                GOAL_VERTICAL => LayoutGoal::Measure(RequestedAxis::Vertical),
                GOAL_BOTH => LayoutGoal::Measure(RequestedAxis::Both),
                _ => unreachable!("the two-bit goal tag covers every bit pattern"),
            },
            sizing_mode: if self.flags & IGNORE_SIZE_STYLES == 0 {
                SizingMode::ApplySizeStyles
            } else {
                SizingMode::IgnoreSizeStyles
            },
            known_dimensions: Size::new(
                unpack_option(self.values[0], self.flags, KNOWN_WIDTH_PRESENT),
                unpack_option(self.values[1], self.flags, KNOWN_HEIGHT_PRESENT),
            ),
            definite_dimensions: Size::new(
                self.flags & DEFINITE_WIDTH != 0,
                self.flags & DEFINITE_HEIGHT != 0,
            ),
            parent_size: Size::new(
                unpack_option(self.values[2], self.flags, PARENT_WIDTH_PRESENT),
                unpack_option(self.values[3], self.flags, PARENT_HEIGHT_PRESENT),
            ),
            available_space: Size::new(
                unpack_available_space(self.values[4], self.flags, AVAILABLE_WIDTH_SHIFT),
                unpack_available_space(self.values[5], self.flags, AVAILABLE_HEIGHT_SHIFT),
            ),
        }
    }

    #[inline]
    fn with_baseline_presence(mut self, baselines: Point<Option<f32>>) -> Self {
        self.flags &= !BASELINE_PRESENCE;
        if baselines.x.is_some() {
            self.flags |= BASELINE_X_PRESENT;
        }
        if baselines.y.is_some() {
            self.flags |= BASELINE_Y_PRESENT;
        }
        self
    }

    #[inline]
    fn key_eq(self, other: Self) -> bool {
        self.values == other.values && self.flags & INPUT_FLAGS == other.flags & INPUT_FLAGS
    }
}

#[inline]
fn pack_option(value: Option<f32>, target: &mut f32, flags: &mut u32, present: u32) {
    if let Some(value) = value {
        *target = value;
        *flags |= present;
    }
}

#[inline]
fn unpack_option(value: f32, flags: u32, present: u32) -> Option<f32> {
    (flags & present != 0).then_some(value)
}

#[inline]
fn pack_available_space(value: AvailableSpace, target: &mut f32, flags: &mut u32, shift: u32) {
    let tag = match value {
        AvailableSpace::Definite(value) => {
            *target = value;
            AVAILABLE_DEFINITE
        }
        AvailableSpace::MinContent => AVAILABLE_MIN_CONTENT,
        AvailableSpace::MaxContent => AVAILABLE_MAX_CONTENT,
    };
    *flags |= tag << shift;
}

#[inline]
fn unpack_available_space(value: f32, flags: u32, shift: u32) -> AvailableSpace {
    match (flags >> shift) & AVAILABLE_TAG_MASK {
        AVAILABLE_DEFINITE => AvailableSpace::Definite(value),
        AVAILABLE_MIN_CONTENT => AvailableSpace::MinContent,
        AVAILABLE_MAX_CONTENT => AvailableSpace::MaxContent,
        _ => unreachable!("packed available-space tags never use the spare value"),
    }
}

/// A dense representation of all six layout output values.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PackedLayoutOutput {
    values: [f32; 6],
}

impl PackedLayoutOutput {
    #[inline]
    fn new(output: LayoutOutput) -> Self {
        Self {
            values: [
                output.size.width,
                output.size.height,
                output.content_size.width,
                output.content_size.height,
                output.first_baselines.x.unwrap_or(0.0),
                output.first_baselines.y.unwrap_or(0.0),
            ],
        }
    }

    #[inline]
    fn unpack(self, input_flags: u32) -> LayoutOutput {
        LayoutOutput {
            size: Size::new(self.values[0], self.values[1]),
            content_size: Size::new(self.values[2], self.values[3]),
            first_baselines: Point::new(
                (input_flags & BASELINE_X_PRESENT != 0).then_some(self.values[4]),
                (input_flags & BASELINE_Y_PRESENT != 0).then_some(self.values[5]),
            ),
        }
    }
}

/// One compact cached measurement input→output pair.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MeasurementSlot {
    input: PackedLayoutInput,
    output: PackedLayoutOutput,
}

impl MeasurementSlot {
    #[inline]
    fn new(input: LayoutInput, output: LayoutOutput) -> Self {
        Self {
            input: PackedLayoutInput::new(input).with_baseline_presence(output.first_baselines),
            output: PackedLayoutOutput::new(output),
        }
    }

    #[inline]
    fn unpack_output(self) -> LayoutOutput {
        self.output.unpack(self.input.flags)
    }
}

/// A bounded per-node layout cache holding two measurements inline and
/// spilling to the heap up to a ceiling of 32.
#[derive(Debug, PartialEq, Default)]
pub struct Cache {
    committed: Option<MeasurementSlot>,
    measurements: SmallVec<[MeasurementSlot; MEASURE_CACHE_INLINE]>,
}

impl Cache {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            committed: None,
            measurements: SmallVec::new_const(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.committed.is_none() && self.measurements.is_empty()
    }

    /// The complete committed input, independence payload included.
    #[must_use]
    pub fn committed_input(&self) -> Option<LayoutInput> {
        self.committed.map(|slot| slot.input.unpack())
    }

    #[must_use]
    pub fn committed_output(&self) -> Option<LayoutOutput> {
        self.committed.map(MeasurementSlot::unpack_output)
    }

    /// Returns a cached output.
    #[must_use]
    pub fn get(&self, input: LayoutInput) -> Option<LayoutOutput> {
        let requested = PackedLayoutInput::new(input);
        if let Some(slot) = self.committed
            && packed_inputs_match(slot.input, requested)
        {
            return Some(slot.unpack_output());
        }

        if input.goal.commits() {
            return None;
        }

        self.measurements
            .iter()
            .find(|slot| packed_inputs_match(slot.input, requested))
            .map(|slot| slot.unpack_output())
    }

    pub fn store(&mut self, input: LayoutInput, output: LayoutOutput) {
        let slot = MeasurementSlot::new(input, output);
        match input.goal {
            LayoutGoal::Commit { .. } => self.committed = Some(slot),
            LayoutGoal::Measure(_) => {
                // Retiring a live same-shape measurement while the cache still
                // has room costs a whole subtree recomputation the next time
                // its constraint comes back, so a replacement target is only
                // worth looking for once there is nowhere left to grow.
                let full = self.measurements.len() >= MEASURE_CACHE_LIMIT;
                let mut exact = None;
                let mut same_shape = None;
                for (index, cached) in self.measurements.iter().enumerate() {
                    if cached.input.key_eq(slot.input) {
                        exact = Some(index);
                        break;
                    }
                    if full
                        && same_shape.is_none()
                        && packed_same_constraint_shape(cached.input, slot.input)
                    {
                        same_shape = Some(index);
                    }
                }
                if let Some(target) = exact {
                    self.measurements[target] = slot;
                } else if !full {
                    self.measurements.push(slot);
                } else if let Some(target) = same_shape {
                    self.measurements[target] = slot;
                } else {
                    self.measurements[packed_constraint_shape_hash(slot.input)] = slot;
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.committed = None;
        self.measurements.clear();
    }
}

#[inline]
#[allow(
    clippy::float_cmp,
    reason = "packed layout-cache keys preserve LayoutInput's exact f32 PartialEq semantics"
)]
fn packed_inputs_match(stored: PackedLayoutInput, requested: PackedLayoutInput) -> bool {
    let stored_goal = (stored.flags >> GOAL_SHIFT) & GOAL_MASK;
    let requested_goal = (requested.flags >> GOAL_SHIFT) & GOAL_MASK;
    let goal_matches = if stored_goal == GOAL_COMMIT {
        requested_goal == GOAL_COMMIT || requested_goal == GOAL_BOTH
    } else {
        stored_goal == requested_goal
    };
    if !goal_matches
        || (stored.flags ^ requested.flags) & EXACT_INPUT_FLAGS != 0
        || stored.values[0] != requested.values[0]
        || stored.values[1] != requested.values[1]
        || stored.values[2] != requested.values[2]
        || stored.values[3] != requested.values[3]
    {
        return false;
    }

    packed_available_space_matches(
        stored,
        requested,
        4,
        AVAILABLE_WIDTH_SHIFT,
        0,
        KNOWN_WIDTH_PRESENT,
    ) && packed_available_space_matches(
        stored,
        requested,
        5,
        AVAILABLE_HEIGHT_SHIFT,
        1,
        KNOWN_HEIGHT_PRESENT,
    )
}

#[inline]
#[allow(
    clippy::float_cmp,
    reason = "packed available-space keys preserve exact f32 equivalence"
)]
fn packed_available_space_matches(
    stored: PackedLayoutInput,
    requested: PackedLayoutInput,
    available_index: usize,
    tag_shift: u32,
    known_index: usize,
    known_present: u32,
) -> bool {
    let stored_tag = (stored.flags >> tag_shift) & AVAILABLE_TAG_MASK;
    let requested_tag = (requested.flags >> tag_shift) & AVAILABLE_TAG_MASK;
    if stored_tag == requested_tag
        && (stored_tag != AVAILABLE_DEFINITE
            || stored.values[available_index] == requested.values[available_index])
    {
        return true;
    }

    requested.flags & known_present != 0
        && ((stored_tag == AVAILABLE_DEFINITE
            && stored.values[available_index] == requested.values[known_index])
            || (requested_tag == AVAILABLE_DEFINITE
                && requested.values[available_index] == requested.values[known_index]))
}

#[cfg(test)]
#[inline]
#[allow(
    clippy::float_cmp,
    reason = "layout-cache keys intentionally use exact float equivalence"
)]
fn available_space_matches(
    stored: AvailableSpace,
    requested: AvailableSpace,
    known_dimension: Option<f32>,
) -> bool {
    if stored == requested {
        return true;
    }

    known_dimension.is_some_and(|known| {
        matches!(stored, AvailableSpace::Definite(value) if value == known)
            || matches!(requested, AvailableSpace::Definite(value) if value == known)
    })
}

#[cfg(test)]
#[inline]
fn inputs_match(stored: LayoutInput, requested: LayoutInput) -> bool {
    let goal_matches = match (stored.goal, requested.goal) {
        (
            LayoutGoal::Commit { .. },
            LayoutGoal::Commit { .. } | LayoutGoal::Measure(RequestedAxis::Both),
        ) => true,
        (LayoutGoal::Measure(stored), LayoutGoal::Measure(requested)) => stored == requested,
        _ => false,
    };
    if !goal_matches
        || stored.sizing_mode != requested.sizing_mode
        || stored.known_dimensions != requested.known_dimensions
        || stored.definite_dimensions != requested.definite_dimensions
        || stored.parent_size != requested.parent_size
    {
        return false;
    }

    available_space_matches(
        stored.available_space.width,
        requested.available_space.width,
        requested.known_dimensions.width,
    ) && available_space_matches(
        stored.available_space.height,
        requested.available_space.height,
        requested.known_dimensions.height,
    )
}

fn packed_axis_constraint_shape(
    input: PackedLayoutInput,
    known_present: u32,
    available_shift: u32,
) -> usize {
    if input.flags & known_present != 0 {
        3
    } else {
        ((input.flags >> available_shift) & AVAILABLE_TAG_MASK) as usize
    }
}

#[inline]
fn packed_same_constraint_shape(left: PackedLayoutInput, right: PackedLayoutInput) -> bool {
    (left.flags ^ right.flags) & (DEFINITE_WIDTH | DEFINITE_HEIGHT) == 0
        && packed_axis_constraint_shape(left, KNOWN_WIDTH_PRESENT, AVAILABLE_WIDTH_SHIFT)
            == packed_axis_constraint_shape(right, KNOWN_WIDTH_PRESENT, AVAILABLE_WIDTH_SHIFT)
        && packed_axis_constraint_shape(left, KNOWN_HEIGHT_PRESENT, AVAILABLE_HEIGHT_SHIFT)
            == packed_axis_constraint_shape(right, KNOWN_HEIGHT_PRESENT, AVAILABLE_HEIGHT_SHIFT)
}

#[inline]
fn packed_constraint_shape_hash(input: PackedLayoutInput) -> usize {
    let width = packed_axis_constraint_shape(input, KNOWN_WIDTH_PRESENT, AVAILABLE_WIDTH_SHIFT);
    let height = packed_axis_constraint_shape(input, KNOWN_HEIGHT_PRESENT, AVAILABLE_HEIGHT_SHIFT);
    (width * 4 + height) % MEASURE_CACHE_SLOTS
}

#[cfg(test)]
#[inline]
fn same_constraint_shape(left: LayoutInput, right: LayoutInput) -> bool {
    packed_same_constraint_shape(PackedLayoutInput::new(left), PackedLayoutInput::new(right))
}

#[cfg(test)]
#[inline]
fn constraint_shape_hash(input: LayoutInput) -> usize {
    packed_constraint_shape_hash(PackedLayoutInput::new(input))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn measurement(
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
    ) -> LayoutInput {
        LayoutInput::measure(
            known_dimensions,
            Size::new(Some(320.0), Some(240.0)),
            available_space,
            RequestedAxis::Both,
        )
    }

    fn assert_option_bits_eq(left: Option<f32>, right: Option<f32>) {
        match (left, right) {
            (None, None) => {}
            (Some(left), Some(right)) => assert_eq!(left.to_bits(), right.to_bits()),
            _ => panic!("option presence changed during packing"),
        }
    }

    fn assert_available_bits_eq(left: AvailableSpace, right: AvailableSpace) {
        match (left, right) {
            (AvailableSpace::Definite(left), AvailableSpace::Definite(right)) => {
                assert_eq!(left.to_bits(), right.to_bits());
            }
            (AvailableSpace::MinContent, AvailableSpace::MinContent)
            | (AvailableSpace::MaxContent, AvailableSpace::MaxContent) => {}
            _ => panic!("available-space tag changed during packing"),
        }
    }

    const fn commit_goal(width: bool, height: bool) -> LayoutGoal {
        LayoutGoal::Commit {
            content_independent: Size::new(width, height),
        }
    }

    fn assert_input_bits_eq(left: LayoutInput, right: LayoutInput) {
        assert_eq!(left.goal, right.goal);
        assert_eq!(left.sizing_mode, right.sizing_mode);
        assert_option_bits_eq(left.known_dimensions.width, right.known_dimensions.width);
        assert_option_bits_eq(left.known_dimensions.height, right.known_dimensions.height);
        assert_eq!(left.definite_dimensions, right.definite_dimensions);
        assert_option_bits_eq(left.parent_size.width, right.parent_size.width);
        assert_option_bits_eq(left.parent_size.height, right.parent_size.height);
        assert_available_bits_eq(left.available_space.width, right.available_space.width);
        assert_available_bits_eq(left.available_space.height, right.available_space.height);
    }

    fn assert_output_bits_eq(left: LayoutOutput, right: LayoutOutput) {
        assert_eq!(left.size.width.to_bits(), right.size.width.to_bits());
        assert_eq!(left.size.height.to_bits(), right.size.height.to_bits());
        assert_eq!(
            left.content_size.width.to_bits(),
            right.content_size.width.to_bits()
        );
        assert_eq!(
            left.content_size.height.to_bits(),
            right.content_size.height.to_bits()
        );
        assert_option_bits_eq(left.first_baselines.x, right.first_baselines.x);
        assert_option_bits_eq(left.first_baselines.y, right.first_baselines.y);
    }

    fn assert_packed_match_agrees_with_oracle(stored: LayoutInput, requested: LayoutInput) {
        assert_eq!(
            packed_inputs_match(
                PackedLayoutInput::new(stored),
                PackedLayoutInput::new(requested)
            ),
            inputs_match(stored, requested),
            "packed matcher diverged for stored={stored:?}, requested={requested:?}"
        );
    }

    #[test]
    fn packed_inputs_exhaustively_round_trip_all_tags_and_presence_bits() {
        let goals = [
            commit_goal(false, false),
            commit_goal(true, false),
            commit_goal(false, true),
            commit_goal(true, true),
            LayoutGoal::Measure(RequestedAxis::Horizontal),
            LayoutGoal::Measure(RequestedAxis::Vertical),
            LayoutGoal::Measure(RequestedAxis::Both),
        ];
        let sizing_modes = [SizingMode::ApplySizeStyles, SizingMode::IgnoreSizeStyles];
        let available = [
            AvailableSpace::Definite(f32::from_bits(0x7fc0_1234)),
            AvailableSpace::MinContent,
            AvailableSpace::MaxContent,
        ];
        let option_values = [
            f32::from_bits(0x8000_0000),
            f32::INFINITY,
            f32::from_bits(0x7fc0_5678),
            -93.25,
        ];

        for goal in goals {
            for sizing_mode in sizing_modes {
                for option_presence in 0_u16..16 {
                    for definite_dimensions in 0_u16..4 {
                        for available_width in available {
                            for available_height in available {
                                let option = |index: usize| {
                                    (option_presence & (1 << index) != 0)
                                        .then_some(option_values[index])
                                };
                                let input = LayoutInput {
                                    goal,
                                    sizing_mode,
                                    known_dimensions: Size::new(option(0), option(1)),
                                    definite_dimensions: Size::new(
                                        definite_dimensions & 1 != 0,
                                        definite_dimensions & 2 != 0,
                                    ),
                                    parent_size: Size::new(option(2), option(3)),
                                    available_space: Size::new(available_width, available_height),
                                };
                                assert_input_bits_eq(input, PackedLayoutInput::new(input).unpack());
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn packed_outputs_exhaustively_round_trip_baseline_presence() {
        for presence in 0_u8..4 {
            let input = LayoutInput::default();
            let output = LayoutOutput {
                size: Size::new(f32::from_bits(0x8000_0000), f32::INFINITY),
                content_size: Size::new(f32::NEG_INFINITY, f32::from_bits(0x7fc0_1234)),
                first_baselines: Point::new(
                    (presence & 1 != 0).then_some(f32::from_bits(0x7fc0_5678)),
                    (presence & 2 != 0).then_some(-0.0),
                ),
            };
            assert_output_bits_eq(output, MeasurementSlot::new(input, output).unpack_output());
        }
    }

    #[test]
    fn packed_matcher_matches_the_layout_input_oracle_across_hot_key_states() {
        let goals = [
            commit_goal(false, false),
            commit_goal(true, false),
            commit_goal(false, true),
            commit_goal(true, true),
            LayoutGoal::Measure(RequestedAxis::Horizontal),
            LayoutGoal::Measure(RequestedAxis::Vertical),
            LayoutGoal::Measure(RequestedAxis::Both),
        ];
        let sizing_modes = [SizingMode::ApplySizeStyles, SizingMode::IgnoreSizeStyles];
        let option_values = [None, Some(0.0), Some(-0.0), Some(f32::NAN)];
        let available_values = [
            AvailableSpace::Definite(0.0),
            AvailableSpace::Definite(-0.0),
            AvailableSpace::Definite(f32::NAN),
            AvailableSpace::MinContent,
            AvailableSpace::MaxContent,
        ];

        for stored_goal in goals {
            for requested_goal in goals {
                for stored_sizing in sizing_modes {
                    for requested_sizing in sizing_modes {
                        for stored_definite in [false, true] {
                            for requested_definite in [false, true] {
                                for stored_known in option_values {
                                    for requested_known in option_values {
                                        for stored_available in available_values {
                                            for requested_available in available_values {
                                                let stored = LayoutInput {
                                                    goal: stored_goal,
                                                    sizing_mode: stored_sizing,
                                                    known_dimensions: Size::new(
                                                        stored_known,
                                                        Some(8.0),
                                                    ),
                                                    definite_dimensions: Size::new(
                                                        stored_definite,
                                                        true,
                                                    ),
                                                    parent_size: Size::new(Some(-0.0), None),
                                                    available_space: Size::new(
                                                        stored_available,
                                                        AvailableSpace::MaxContent,
                                                    ),
                                                };
                                                let requested = LayoutInput {
                                                    goal: requested_goal,
                                                    sizing_mode: requested_sizing,
                                                    known_dimensions: Size::new(
                                                        requested_known,
                                                        Some(8.0),
                                                    ),
                                                    definite_dimensions: Size::new(
                                                        requested_definite,
                                                        true,
                                                    ),
                                                    parent_size: Size::new(Some(0.0), None),
                                                    available_space: Size::new(
                                                        requested_available,
                                                        AvailableSpace::MaxContent,
                                                    ),
                                                };
                                                assert_packed_match_agrees_with_oracle(
                                                    stored, requested,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn packed_matcher_preserves_parent_option_presence_and_baseline_independence() {
        let base = measurement(Size::NONE, Size::MAX_CONTENT);
        let option_values = [None, Some(0.0), Some(-0.0), Some(f32::NAN)];
        for stored_parent in option_values {
            for requested_parent in option_values {
                let mut stored = base;
                stored.parent_size.width = stored_parent;
                let mut requested = base;
                requested.parent_size.width = requested_parent;
                assert_packed_match_agrees_with_oracle(stored, requested);
            }
        }

        let stored =
            PackedLayoutInput::new(base).with_baseline_presence(Point::new(Some(f32::NAN), None));
        assert!(packed_inputs_match(stored, PackedLayoutInput::new(base)));
    }

    #[test]
    fn nan_keys_keep_partial_eq_semantics_after_packing() {
        let input = measurement(
            Size::new(Some(f32::NAN), None),
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        );
        let packed = PackedLayoutInput::new(input);
        assert_ne!(packed, packed);
        assert!(!inputs_match(packed.unpack(), input));
        assert!(!packed_inputs_match(packed, PackedLayoutInput::new(input)));

        let negative_zero = measurement(
            Size::new(Some(-0.0), None),
            Size::new(AvailableSpace::Definite(-0.0), AvailableSpace::MaxContent),
        );
        let positive_zero = measurement(
            Size::new(Some(0.0), None),
            Size::new(AvailableSpace::Definite(0.0), AvailableSpace::MaxContent),
        );
        assert!(packed_inputs_match(
            PackedLayoutInput::new(negative_zero),
            PackedLayoutInput::new(positive_zero)
        ));

        let available_nan = measurement(
            Size::NONE,
            Size::new(
                AvailableSpace::Definite(f32::NAN),
                AvailableSpace::MaxContent,
            ),
        );
        assert!(!inputs_match(
            PackedLayoutInput::new(available_nan).unpack(),
            available_nan
        ));
    }

    #[test]
    fn committed_entries_round_trip_the_independent_compact_output() {
        let mut cache = Cache::new();
        let input = LayoutInput::commit(
            Size::new(Some(80.0), Some(40.0)),
            Size::new(Some(320.0), Some(240.0)),
            Size::new(
                AvailableSpace::Definite(80.0),
                AvailableSpace::Definite(40.0),
            ),
            Size::new(true, false),
        );
        let stored = LayoutOutput::new(Size::new(80.0, 40.0), Size::new(90.0, 45.0))
            .with_first_baselines(Point::new(None, Some(14.0)));
        cache.store(input, stored);

        assert_eq!(cache.get(input), Some(stored));
        assert_eq!(cache.committed_input(), Some(input));

        let mut measure_both = input;
        measure_both.goal = LayoutGoal::Measure(RequestedAxis::Both);
        assert_eq!(cache.get(measure_both), Some(stored));
        let mut measure_width = measure_both;
        measure_width.goal = LayoutGoal::Measure(RequestedAxis::Horizontal);
        assert_eq!(cache.get(measure_width), None);

        // Independence is outside the key: a commit request that claims
        // something else still answers from the stored entry, and the stored
        // entry keeps its own payload.
        let mut other_independence = input;
        other_independence.goal = commit_goal(false, true);
        assert_eq!(cache.get(other_independence), Some(stored));
        assert_eq!(cache.committed_input(), Some(input));

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn known_dimension_equivalence_is_exact_and_axis_local() {
        assert!(available_space_matches(
            AvailableSpace::MinContent,
            AvailableSpace::MinContent,
            None,
        ));
        assert!(available_space_matches(
            AvailableSpace::Definite(50.0),
            AvailableSpace::MaxContent,
            Some(50.0),
        ));
        assert!(available_space_matches(
            AvailableSpace::MinContent,
            AvailableSpace::Definite(50.0),
            Some(50.0),
        ));
        assert!(!available_space_matches(
            AvailableSpace::Definite(49.0),
            AvailableSpace::MaxContent,
            Some(50.0),
        ));
        assert!(!available_space_matches(
            AvailableSpace::Definite(50.0),
            AvailableSpace::MaxContent,
            None,
        ));
    }

    #[test]
    fn packed_height_available_space_uses_the_height_known_dimension_fallback() {
        let requested = measurement(
            Size::new(Some(13.0), Some(29.0)),
            Size::new(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
        );
        let mut height_equivalent = requested;
        height_equivalent.available_space.height = AvailableSpace::Definite(29.0);
        assert_packed_match_agrees_with_oracle(height_equivalent, requested);
        assert!(packed_inputs_match(
            PackedLayoutInput::new(height_equivalent),
            PackedLayoutInput::new(requested),
        ));

        let mut transposed_axis = requested;
        transposed_axis.available_space.height = AvailableSpace::Definite(13.0);
        assert_packed_match_agrees_with_oracle(transposed_axis, requested);
        assert!(!packed_inputs_match(
            PackedLayoutInput::new(transposed_axis),
            PackedLayoutInput::new(requested),
        ));
    }

    #[test]
    fn full_input_key_rejects_goal_mode_dimension_and_parent_mismatches() {
        let stored = measurement(
            Size::new(Some(50.0), None),
            Size::new(AvailableSpace::Definite(50.0), AvailableSpace::MaxContent),
        );
        let mut requested = stored;
        assert!(inputs_match(stored, requested));

        requested.goal = commit_goal(false, false);
        assert!(!inputs_match(stored, requested));
        requested = stored;
        requested.sizing_mode = SizingMode::IgnoreSizeStyles;
        assert!(!inputs_match(stored, requested));
        requested = stored;
        requested.known_dimensions.width = Some(51.0);
        assert!(!inputs_match(stored, requested));
        requested = stored;
        requested.parent_size.width = Some(321.0);
        assert!(!inputs_match(stored, requested));
    }

    #[test]
    fn all_measurement_shapes_stay_inline_and_round_trip() {
        let inputs = [
            measurement(Size::NONE, Size::MIN_CONTENT),
            measurement(Size::NONE, Size::MAX_CONTENT),
            measurement(
                Size::NONE,
                Size::new(AvailableSpace::Definite(33.0), AvailableSpace::MinContent),
            ),
            measurement(
                Size::NONE,
                Size::new(AvailableSpace::MaxContent, AvailableSpace::Definite(44.0)),
            ),
            measurement(
                Size::NONE,
                Size::new(AvailableSpace::MinContent, AvailableSpace::MaxContent),
            ),
            measurement(
                Size::NONE,
                Size::new(AvailableSpace::MinContent, AvailableSpace::Definite(55.0)),
            ),
            measurement(
                Size::NONE,
                Size::new(AvailableSpace::MaxContent, AvailableSpace::MinContent),
            ),
            measurement(
                Size::NONE,
                Size::new(AvailableSpace::Definite(77.0), AvailableSpace::MaxContent),
            ),
        ];
        let mut cache = Cache::new();
        for (value, input) in [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
            .into_iter()
            .zip(inputs)
        {
            let output = LayoutOutput::new(Size::new(value, 10.0), Size::new(20.0, value));
            cache.store(input, output);
            assert_eq!(cache.get(input), Some(output));
        }
        assert_eq!(cache.measurements.len(), MEASURE_CACHE_SLOTS);
    }

    #[test]
    fn baseline_presence_is_output_metadata_not_part_of_the_input_key() {
        let input = measurement(Size::NONE, Size::MIN_CONTENT);
        let first = LayoutOutput::new(Size::new(10.0, 20.0), Size::new(30.0, 40.0));
        let latest = first.with_first_baselines(Point::new(Some(5.0), Some(6.0)));

        let mut cache = Cache::new();
        cache.store(input, first);
        cache.store(input, latest);

        assert_eq!(cache.measurements.len(), 1);
        assert_eq!(cache.get(input), Some(latest));
    }

    #[test]
    fn cache_grows_to_its_limit_before_retiring_a_live_measurement() {
        let first = measurement(
            Size::NONE,
            Size::new(AvailableSpace::Definite(10.0), AvailableSpace::MinContent),
        );
        let same_shape = measurement(
            Size::NONE,
            Size::new(AvailableSpace::Definite(20.0), AvailableSpace::MinContent),
        );
        let different_shape = measurement(Size::NONE, Size::MAX_CONTENT);
        assert!(same_constraint_shape(first, same_shape));
        assert!(!same_constraint_shape(first, different_shape));

        let filled = |count: usize| {
            let mut cache = Cache::new();
            for index in 0..count {
                let width = u8::try_from(index).expect("the cache limit fits a byte");
                cache.store(
                    measurement(
                        Size::NONE,
                        Size::new(
                            AvailableSpace::Definite(100.0 + f32::from(width)),
                            AvailableSpace::MinContent,
                        ),
                    ),
                    LayoutOutput::new(Size::ZERO, Size::ZERO),
                );
            }
            cache
        };

        // Distinct constraints of one shape accumulate instead of replacing
        // each other, first through the inline slots and then on the heap.
        let inline = filled(MEASURE_CACHE_INLINE);
        assert_eq!(inline.measurements.len(), MEASURE_CACHE_INLINE);
        assert!(!inline.measurements.spilled());
        let spilled = filled(MEASURE_CACHE_INLINE + 1);
        assert_eq!(spilled.measurements.len(), MEASURE_CACHE_INLINE + 1);
        assert!(spilled.measurements.spilled());

        // At the limit the cache stops growing and retires the first entry of
        // the incoming constraint's shape.
        let mut cache = filled(MEASURE_CACHE_LIMIT);
        assert_eq!(cache.measurements.len(), MEASURE_CACHE_LIMIT);
        cache.store(same_shape, LayoutOutput::new(Size::ZERO, Size::ZERO));
        assert_eq!(cache.measurements.len(), MEASURE_CACHE_LIMIT);
        assert_input_bits_eq(cache.measurements[0].input.unpack(), same_shape);

        // A shape nothing in the cache shares falls back to the hashed slot.
        let mut cache = filled(MEASURE_CACHE_LIMIT);
        let target = constraint_shape_hash(different_shape);
        cache.store(different_shape, LayoutOutput::new(Size::ZERO, Size::ZERO));
        assert_eq!(cache.measurements.len(), MEASURE_CACHE_LIMIT);
        assert_input_bits_eq(cache.measurements[target].input.unpack(), different_shape);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn packed_cache_stays_within_the_layout_state_budget() {
        assert!(core::mem::size_of::<PackedLayoutInput>() <= 28);
        assert!(core::mem::size_of::<PackedLayoutOutput>() <= 24);
        assert!(core::mem::size_of::<MeasurementSlot>() <= 52);
        assert!(core::mem::size_of::<Cache>() <= 288);
    }

    #[test]
    fn clear_preserves_the_spilled_full_budget_and_inline_small_use() {
        let mut small = Cache::new();
        small.store(
            measurement(Size::NONE, Size::MAX_CONTENT),
            LayoutOutput::new(Size::ZERO, Size::ZERO),
        );
        small.store(
            measurement(Size::NONE, Size::MIN_CONTENT),
            LayoutOutput::new(Size::ZERO, Size::ZERO),
        );
        assert!(
            !small.measurements.spilled(),
            "a node measured under two shapes must stay allocation-free"
        );

        let mut cache = Cache::new();
        let shapes = [
            AvailableSpace::MinContent,
            AvailableSpace::MaxContent,
            AvailableSpace::Definite(1.0),
        ];
        for width in shapes {
            for height in shapes {
                cache.store(
                    measurement(Size::NONE, Size::new(width, height)),
                    LayoutOutput::new(Size::ZERO, Size::ZERO),
                );
                if cache.measurements.len() == MEASURE_CACHE_SLOTS {
                    break;
                }
            }
        }
        assert_eq!(cache.measurements.len(), MEASURE_CACHE_SLOTS);

        cache.clear();

        assert!(cache.is_empty());
        assert!(
            cache.measurements.capacity() >= MEASURE_CACHE_SLOTS,
            "clear keeps the spilled budget so a busy node does not re-allocate"
        );
    }
}
