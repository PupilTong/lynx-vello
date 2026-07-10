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
//! slots: one slot for the final [`RunMode::PerformLayout`] result, the rest
//! for [`RunMode::ComputeSize`] measurements keyed by constraint shape.
//!
//! # Keying semantics (the contract `get`/`store` will implement in L1)
//!
//! A stored entry matches a lookup when it is *usable*: the entry's
//! `known_dimensions` and `available_space` are compatible with the
//! request's (equal known dimensions; equal-or-equivalent available space,
//! where a definite available space equal to a known dimension is
//! equivalent), and a `PerformLayout` entry may answer a `ComputeSize`
//! lookup but never the reverse. Slot assignment groups entries by
//! constraint shape (which of the four known/available inputs are definite)
//! so repeated probes of the same shape overwrite instead of evicting other
//! shapes. `PerformHiddenLayout` results are never cached.

use crate::geometry::Size;
use crate::tree::{AvailableSpace, LayoutOutput, RunMode};

/// Number of measurement slots in a [`Cache`] (excluding the dedicated
/// final-layout slot).
///
/// Sized to the distinct constraint *shapes* flex/grid sizing passes emit
/// for one node (known/unknown width × height crossed with
/// definite/min-/max-content available space patterns actually produced).
/// The exact figure will be validated against probe traces in L1; the
/// constant is public so hosts can size columnar storage.
pub const MEASURE_CACHE_SLOTS: usize = 8;

/// One cached constraint→output pair.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CacheSlot {
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
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

    /// Looks up a usable entry for these constraints (see the module docs
    /// for the matching contract).
    ///
    /// # Panics
    ///
    /// Protocol stub — the matching policy is implemented in milestone L1;
    /// calling this currently panics with `todo!`.
    #[must_use]
    pub fn get(
        &self,
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        run_mode: RunMode,
    ) -> Option<LayoutOutput> {
        let _ = (known_dimensions, available_space, run_mode);
        todo!("L1: cache slot matching (see module docs for the contract)")
    }

    /// Stores `output` under these constraints (see the module docs for the
    /// slot-assignment contract).
    ///
    /// # Panics
    ///
    /// Protocol stub — the slot policy is implemented in milestone L1;
    /// calling this currently panics with `todo!`.
    pub fn store(
        &mut self,
        known_dimensions: Size<Option<f32>>,
        available_space: Size<AvailableSpace>,
        run_mode: RunMode,
        output: LayoutOutput,
    ) {
        let _ = (known_dimensions, available_space, run_mode, output);
        todo!("L1: cache slot assignment (see module docs for the contract)")
    }

    /// Drops every entry.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}
