//! The engine-provided per-node measurement cache.
//!
//! Layout over a tree is worst-case super-linear: flex and grid both probe
//! children under several sizing constraints (min-content, max-content, and
//! one or more definite widths) before the final positioning pass, and each
//! probe recurses. The cache collapses that blow-up back to roughly one
//! visit per constraint shape — it is the single most important performance
//! mechanism in the protocol, which is why the *protocol* (not the host)
//! defines its semantics, while the *host* still owns its storage via
//! [`CacheTree`](crate::tree::CacheTree).
//!
//! [`Cache`] is the reference container hosts are expected to embed
//! per-node (delegating `CacheTree`'s methods to it), keeping the slot
//! policy uniform across hosts. It is a fixed-size, allocation-free array of
//! slots: one slot for the final [`PerformLayout`](crate::tree::RunMode)
//! result, the rest for [`ComputeSize`](crate::tree::RunMode) measurements
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
//! - `requested_axis`: scopes which axes of a `ComputeSize` answer were actually computed; a
//!   single-axis entry must not answer a request for the other axis.
//!
//! On top of the exact key, a stored entry may satisfy a request under
//! *provable equivalences only*: a
//! [`PerformLayout`](crate::tree::RunMode) entry may answer a
//! [`ComputeSize`](crate::tree::RunMode) lookup whose remaining key fields
//! match (never the reverse); `requested_axis` must match exactly; and a
//! definite `available_space` component equal to
//! the same axis's `known_dimensions` component is equivalent to that known
//! dimension. Slot assignment groups entries by constraint shape (which
//! inputs are definite) so repeated probes of the same shape overwrite
//! instead of evicting other shapes.
//! [`PerformHiddenLayout`](crate::tree::RunMode) results are never cached.

use crate::tree::{AvailableSpace, LayoutInput, LayoutOutput, RunMode};

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
/// Embed one per node and delegate
/// [`CacheTree`](crate::tree::CacheTree)'s methods to it:
///
/// ```
/// use neutron_star::cache::Cache;
///
/// struct HostNode {
///     // … styles, children …
///     cache: Cache,
/// }
///
/// let node = HostNode {
///     cache: Cache::new(),
/// };
/// assert!(node.cache.is_empty());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cache {
    /// The final full-layout entry (`RunMode::PerformLayout`).
    perform_layout: Option<CacheSlot>,
    /// Measurement entries (`RunMode::ComputeSize`), slotted by constraint
    /// shape.
    measurements: [Option<CacheSlot>; MEASURE_CACHE_SLOTS],
}

impl Cache {
    /// An empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            perform_layout: None,
            measurements: [None; MEASURE_CACHE_SLOTS],
        }
    }

    /// `true` if no entry is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.perform_layout.is_none() && self.measurements.iter().all(Option::is_none)
    }

    /// Looks up an entry usable for the complete `input` key (see the
    /// module docs for the matching contract and its allowed equivalences).
    #[must_use]
    pub fn get(&self, input: LayoutInput) -> Option<LayoutOutput> {
        if input.run_mode == RunMode::PerformHiddenLayout {
            return None;
        }

        // A full-layout result contains at least as much information as a
        // measurement result, so it is the preferred answer for a compatible
        // ComputeSize request as well.
        if let Some(slot) = self.perform_layout
            && inputs_match(slot.input, input)
        {
            return Some(slot.output);
        }

        if input.run_mode == RunMode::PerformLayout {
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
        match input.run_mode {
            RunMode::PerformLayout => self.perform_layout = Some(slot),
            RunMode::ComputeSize => {
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
            RunMode::PerformHiddenLayout => {}
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
    let run_mode_matches = match requested.run_mode {
        RunMode::PerformLayout => stored.run_mode == RunMode::PerformLayout,
        RunMode::ComputeSize => {
            matches!(
                stored.run_mode,
                RunMode::PerformLayout | RunMode::ComputeSize
            )
        }
        RunMode::PerformHiddenLayout => false,
    };
    if !run_mode_matches
        || stored.sizing_mode != requested.sizing_mode
        || stored.known_dimensions != requested.known_dimensions
        || stored.parent_size != requested.parent_size
    {
        return false;
    }

    stored.requested_axis == requested.requested_axis
        && available_space_matches(
            stored.available_space.width,
            requested.available_space.width,
            requested.known_dimensions.width,
        )
        && available_space_matches(
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
    axis_constraint_shape(left.known_dimensions.width, left.available_space.width)
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
