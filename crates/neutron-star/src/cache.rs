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
//! # Keying semantics (the contract `get`/`store` will implement in L1)
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
//! match (never the reverse); a `RequestedAxis::Both` entry may answer a
//! single-axis probe; and a definite `available_space` component equal to
//! the same axis's `known_dimensions` component is equivalent to that known
//! dimension. Slot assignment groups entries by constraint shape (which
//! inputs are definite) so repeated probes of the same shape overwrite
//! instead of evicting other shapes.
//! [`PerformHiddenLayout`](crate::tree::RunMode) results are never cached.

use crate::tree::{LayoutInput, LayoutOutput};

/// Number of measurement slots in a [`Cache`] (excluding the dedicated
/// final-layout slot).
///
/// Sized to the distinct constraint *shapes* flex/grid sizing passes emit
/// for one node (known/unknown width × height crossed with
/// definite/min-/max-content available space patterns actually produced).
/// The exact figure will be validated against probe traces in L1; the
/// constant is public so hosts can size columnar storage.
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
    ///
    /// # Panics
    ///
    /// Protocol stub — the matching policy is implemented in milestone L1;
    /// calling this currently panics with `todo!`.
    #[must_use]
    pub fn get(&self, input: LayoutInput) -> Option<LayoutOutput> {
        let _ = input;
        todo!("L1: cache slot matching (see module docs for the contract)")
    }

    /// Stores `output` under the complete `input` key (see the module docs
    /// for the slot-assignment contract).
    ///
    /// # Panics
    ///
    /// Protocol stub — the slot policy is implemented in milestone L1;
    /// calling this currently panics with `todo!`.
    pub fn store(&mut self, input: LayoutInput, output: LayoutOutput) {
        let _ = (input, output);
        todo!("L1: cache slot assignment (see module docs for the contract)")
    }

    /// Drops every entry.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}
