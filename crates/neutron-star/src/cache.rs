//! The engine-provided per-node measurement cache.

use crate::tree::{AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis};

pub const MEASURE_CACHE_SLOTS: usize = 8;

/// One cached input→output pair (the full [`LayoutInput`] is the key).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CacheSlot {
    input: LayoutInput,
    output: LayoutOutput,
}

/// A fixed-size, allocation-free per-node layout cache.
#[derive(Debug, PartialEq, Default)]
pub struct Cache {
    committed_layout: Option<CacheSlot>,
    measurements: [Option<CacheSlot>; MEASURE_CACHE_SLOTS],
}

impl Cache {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            committed_layout: None,
            measurements: [None; MEASURE_CACHE_SLOTS],
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.committed_layout.is_none() && self.measurements.iter().all(Option::is_none)
    }

    #[must_use]
    pub const fn committed_input(&self) -> Option<LayoutInput> {
        match self.committed_layout {
            Some(slot) => Some(slot.input),
            None => None,
        }
    }

    #[must_use]
    pub fn get(&self, input: LayoutInput) -> Option<LayoutOutput> {
        if let Some(slot) = self.committed_layout
            && inputs_match(slot.input, input)
        {
            return Some(slot.output);
        }

        if input.goal == LayoutGoal::Commit {
            return None;
        }

        self.measurements
            .iter()
            .flatten()
            .find(|slot| inputs_match(slot.input, input))
            .map(|slot| slot.output)
    }

    pub fn store(&mut self, input: LayoutInput, output: LayoutOutput) {
        let slot = CacheSlot { input, output };
        match input.goal {
            LayoutGoal::Commit => self.committed_layout = Some(slot),
            LayoutGoal::Measure(_) => {
                let target = self
                    .measurements
                    .iter()
                    .position(|cached| cached.is_some_and(|cached| cached.input == input))
                    .or_else(|| {
                        self.measurements.iter().position(|cached| {
                            cached.is_some_and(|cached| same_constraint_shape(cached.input, input))
                        })
                    })
                    .or_else(|| self.measurements.iter().position(Option::is_none))
                    .unwrap_or_else(|| constraint_shape_hash(input));
                self.measurements[target] = Some(slot);
            }
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

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

#[inline]
fn inputs_match(stored: LayoutInput, requested: LayoutInput) -> bool {
    let goal_matches = match (stored.goal, requested.goal) {
        (LayoutGoal::Commit, LayoutGoal::Commit | LayoutGoal::Measure(RequestedAxis::Both)) => true,
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

#[inline]
fn axis_constraint_shape(known_dimension: Option<f32>, available_space: AvailableSpace) -> usize {
    if known_dimension.is_some() {
        return 3;
    }

    match available_space {
        AvailableSpace::Definite(_) => 0,
        AvailableSpace::MinContent => 1,
        AvailableSpace::MaxContent => 2,
    }
}

#[inline]
fn same_constraint_shape(left: LayoutInput, right: LayoutInput) -> bool {
    left.definite_dimensions == right.definite_dimensions
        && axis_constraint_shape(left.known_dimensions.width, left.available_space.width)
            == axis_constraint_shape(right.known_dimensions.width, right.available_space.width)
        && axis_constraint_shape(left.known_dimensions.height, left.available_space.height)
            == axis_constraint_shape(right.known_dimensions.height, right.available_space.height)
}

#[inline]
fn constraint_shape_hash(input: LayoutInput) -> usize {
    let width = axis_constraint_shape(input.known_dimensions.width, input.available_space.width);
    let height = axis_constraint_shape(input.known_dimensions.height, input.available_space.height);
    (width * 4 + height) % MEASURE_CACHE_SLOTS
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::geometry::Size;
    use crate::tree::SizingMode;

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

    #[test]
    fn committed_entries_only_answer_compatible_commit_or_both_axis_measurement() {
        let mut cache = Cache::new();
        let input = LayoutInput::commit(
            Size::new(Some(80.0), Some(40.0)),
            Size::new(Some(320.0), Some(240.0)),
            Size::new(
                AvailableSpace::Definite(80.0),
                AvailableSpace::Definite(40.0),
            ),
        );
        let output = LayoutOutput::new(Size::new(80.0, 40.0), Size::new(90.0, 45.0));
        cache.store(input, output);

        assert_eq!(cache.get(input), Some(output));
        let mut measure_both = input;
        measure_both.goal = LayoutGoal::Measure(RequestedAxis::Both);
        assert_eq!(cache.get(measure_both), Some(output));
        let mut measure_width = measure_both;
        measure_width.goal = LayoutGoal::Measure(RequestedAxis::Horizontal);
        assert_eq!(cache.get(measure_width), None);

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
    fn full_input_key_rejects_goal_mode_dimension_and_parent_mismatches() {
        let stored = measurement(
            Size::new(Some(50.0), None),
            Size::new(AvailableSpace::Definite(50.0), AvailableSpace::MaxContent),
        );
        let mut requested = stored;
        assert!(inputs_match(stored, requested));

        requested.goal = LayoutGoal::Commit;
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
    fn constraint_shapes_cover_every_axis_state_and_bound_slot_replacement() {
        assert_eq!(
            axis_constraint_shape(Some(10.0), AvailableSpace::MinContent),
            3
        );
        assert_eq!(
            axis_constraint_shape(None, AvailableSpace::Definite(10.0)),
            0
        );
        assert_eq!(axis_constraint_shape(None, AvailableSpace::MinContent), 1);
        assert_eq!(axis_constraint_shape(None, AvailableSpace::MaxContent), 2);

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

        let shape_values = [
            AvailableSpace::Definite(10.0),
            AvailableSpace::MinContent,
            AvailableSpace::MaxContent,
        ];
        let mut inputs = Vec::new();
        for width in shape_values {
            for height in shape_values {
                inputs.push(measurement(Size::NONE, Size::new(width, height)));
            }
        }

        let mut cache = Cache::new();
        for input in inputs.iter().copied().take(MEASURE_CACHE_SLOTS) {
            let size = Size::new(1.0, 0.0);
            cache.store(input, LayoutOutput::new(size, size));
        }
        let replacement = inputs[MEASURE_CACHE_SLOTS];
        let target = constraint_shape_hash(replacement);
        cache.store(
            replacement,
            LayoutOutput::new(Size::new(99.0, 0.0), Size::new(99.0, 0.0)),
        );
        assert_eq!(
            cache.measurements[target].map(|slot| slot.input),
            Some(replacement)
        );
    }
}
