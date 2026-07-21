//! The engine-provided per-node measurement cache.
//!
//! Layout over a tree is worst-case super-linear: flex and grid both probe
//! children under several sizing constraints (min-content, max-content, and
//! one or more definite widths) before the final positioning pass, and each
//! probe recurses. The cache collapses that blow-up back to roughly one
//! visit per constraint shape — it is the single most important performance
//! mechanism in the protocol, which is why the *protocol* (not the host)
//! defines its semantics, while the *host* still owns its storage via the
//! [`LayoutNode`](crate::tree::LayoutNode) cache methods.
//!
//! [`Cache`] is the reference container hosts are expected to embed
//! per-node in an interior-mutable slot (delegating
//! [`cache_get`](crate::tree::LayoutNode::cache_get)/
//! [`cache_store`](crate::tree::LayoutNode::cache_store)/
//! [`cache_clear`](crate::tree::LayoutNode::cache_clear) to it), keeping the
//! slot policy uniform across hosts. It is a fixed-size, allocation-free array of
//! slots: one slot for the final [`Commit`](crate::tree::LayoutGoal::Commit)
//! result, the rest for [`Measure`](crate::tree::LayoutGoal::Measure) probes
//! keyed by constraint shape.
//!
//! # Keying semantics
//!
//! The key is the **complete [`LayoutInput`]** — every field can change the
//! result, so none may be dropped from matching. In particular:
//!
//! - `sizing_mode`: an [`InherentSize`](crate::tree::SizingMode) run applies the node's own
//!   `size`/`min`/`max`/`aspect-ratio`; a [`ContentSize`](crate::tree::SizingMode) probe ignores
//!   them. Entries from one must never answer the other.
//! - `parent_size`: the percentage basis. Identical constraints under a different parent size
//!   resolve percentage styles differently.
//! - `definite_dimensions`: equal used geometry can differ in whether it establishes a percentage
//!   basis for descendants (notably after Flexbox's post-flexing step).
//! - `goal`: distinguishes a geometry-committing run from a measurement and, for measurements,
//!   scopes which axes the answer actually computed. A single-axis entry must not answer a request
//!   for the other axis.
//!
//! On top of the exact key, a stored entry may satisfy a request under
//! *provable equivalences only*: a
//! [`Commit`](crate::tree::LayoutGoal::Commit) entry may answer a
//! [`Measure(Both)`](crate::tree::LayoutGoal::Measure) lookup whose remaining
//! key fields match (never the reverse, and never a single-axis measurement);
//! measurement axes must otherwise match exactly; and a
//! definite `available_space` component equal to
//! the same axis's `known_dimensions` component is equivalent to that known
//! dimension. Slot assignment groups entries by constraint shape (which
//! inputs are definite) so repeated probes of the same shape overwrite
//! instead of evicting other shapes.

use crate::tree::{AvailableSpace, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis};

/// Number of measurement slots in a [`Cache`] (excluding the dedicated
/// final-layout slot).
///
/// Sized to cover the distinct constraint *shapes* used by layout sizing
/// passes for one node (known/unknown width × height crossed with
/// definite/min-/max-content available-space patterns).
/// The constant is public so hosts can size columnar storage consistently
/// with the reference cache.
pub const MEASURE_CACHE_SLOTS: usize = 8;

/// One cached input→output pair (the full [`LayoutInput`] is the key).
#[derive(Debug, Clone, Copy, PartialEq)]
struct CacheSlot {
    input: LayoutInput,
    output: LayoutOutput,
}

/// A fixed-size, allocation-free per-node layout cache.
///
/// Embed one per node behind interior mutability and delegate the
/// [`LayoutNode`](crate::tree::LayoutNode) cache methods to it:
///
/// ```
/// use std::cell::RefCell;
///
/// use neutron_star::cache::Cache;
///
/// // Layout is single-threaded; a RefCell keeps the per-node slot
/// // writable through the Copy node handle.
/// struct HostNode {
///     cache: RefCell<Cache>,
/// }
///
/// let node = HostNode {
///     cache: RefCell::new(Cache::new()),
/// };
/// assert!(node.cache.borrow().is_empty());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cache {
    /// The final full-layout entry ([`LayoutGoal::Commit`]).
    committed_layout: Option<CacheSlot>,
    /// Measurement entries ([`LayoutGoal::Measure`]), slotted by constraint
    /// shape.
    measurements: [Option<CacheSlot>; MEASURE_CACHE_SLOTS],
}

impl Cache {
    /// An empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            committed_layout: None,
            measurements: [None; MEASURE_CACHE_SLOTS],
        }
    }

    /// `true` if no entry is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.committed_layout.is_none() && self.measurements.iter().all(Option::is_none)
    }

    /// The [`LayoutInput`] stored in the committed
    /// ([`LayoutGoal::Commit`]) slot, if the node has been committed.
    ///
    /// This is the exact input the node was last laid out with — including any
    /// caller-imposed constraints (a stretched cross size, a flex-grown main
    /// size, a resolved percentage size) that made its **used** size differ
    /// from its self-determined size. When re-rooting a relayout at a
    /// size-and-layout containment boundary, this is the input to reuse via
    /// [`compute_boundary_relayout`](crate::compute::compute_boundary_relayout):
    /// re-deriving the boundary's size from `available_space` alone
    /// ([`compute_root_layout`](crate::compute::compute_root_layout)) would drop
    /// those constraints and desync the boundary from its un-invalidated
    /// ancestors. Capture it **before**
    /// [`invalidate_for_relayout`](crate::invalidate::invalidate_for_relayout)
    /// clears the cache.
    #[must_use]
    pub const fn committed_input(&self) -> Option<LayoutInput> {
        match self.committed_layout {
            Some(slot) => Some(slot.input),
            None => None,
        }
    }

    /// Looks up an entry usable for the complete `input` key (see the
    /// module docs for the matching contract and its allowed equivalences).
    #[must_use]
    pub fn get(&self, input: LayoutInput) -> Option<LayoutOutput> {
        // A full-layout result contains at least as much information as a
        // both-axis measurement result, so it is the preferred answer for a
        // compatible `Measure(Both)` request as well.
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

    /// Stores `output` under the complete `input` key (see the module docs
    /// for the slot-assignment contract).
    pub fn store(&mut self, input: LayoutInput, output: LayoutOutput) {
        let slot = CacheSlot { input, output };
        match input.goal {
            LayoutGoal::Commit => self.committed_layout = Some(slot),
            LayoutGoal::Measure(_) => {
                // Exact repeats overwrite in place. If the numeric values
                // changed but the constraint shape did not, reuse that
                // shape's slot so a stream of resize values cannot flush
                // every other useful probe shape from the cache.
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

    /// Drops every entry.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

#[inline]
#[allow(clippy::float_cmp)] // Cache-key equivalence is intentionally exact.
fn available_space_matches(
    stored: AvailableSpace,
    requested: AvailableSpace,
    known_dimension: Option<f32>,
) -> bool {
    if stored == requested {
        return true;
    }

    // Once the caller has fixed an axis, an available-space value carrying
    // that exact same definite extent supplies no additional constraint.
    // This is deliberately narrower than ignoring available space whenever
    // a known dimension exists: it is the only non-exact equivalence the
    // cache contract permits.
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
        LayoutInput::compute_size(
            known_dimensions,
            Size::new(Some(320.0), Some(240.0)),
            available_space,
            RequestedAxis::Both,
        )
    }

    #[test]
    fn committed_entries_only_answer_compatible_commit_or_both_axis_measurement() {
        let mut cache = Cache::new();
        let input = LayoutInput::perform_layout(
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
        requested.sizing_mode = SizingMode::ContentSize;
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
