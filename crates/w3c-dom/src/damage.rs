//! Restyle damage — what a style change means for downstream layout/paint.
//!
//! A style flush ([`Document::flush_styles`](crate::Document::flush_styles))
//! restyles the affected nodes and, for each node whose computed style
//! actually changed, stylo records a
//! [`ServoRestyleDamage`] bitset in its `ElementData` classifying the change:
//! repaint-only, stacking-context rebuild, overflow recalculation, or a full
//! relayout (the classes are hierarchical — each higher phase implies the ones
//! below it).
//!
//! The harvest at the end of the flush (the crate-private
//! `Document::harvest_flush`) copies that damage into [`StyleDamage`],
//! **clears it** from stylo's `ElementData`, then immediately consumes the
//! relayout-class effect into the document's layout caches and streams all
//! damage to the embedder. Clearing is not optional: stylo never clears damage for a
//! normal restyle, and in servo builds `element_needs_traversal` re-traverses
//! any element that still carries non-empty damage, so an un-harvested node
//! would be re-styled on every later flush forever (see
//! [`Document::flush_styles_with_sink`](crate::Document::flush_styles_with_sink)
//! for the full argument).
//!
//! # No damage on initial styling
//!
//! Damage is a *diff*: stylo only produces it when a node already had an old
//! computed style to compare against. The first flush of a subtree therefore
//! reports **no** damage even though every node was styled — embedders lay
//! out a freshly styled subtree from their own structural knowledge, not from
//! damage. A later `display: none → visible` flip does produce `RELAYOUT`
//! damage on the flipped node, which (being a relayout) covers its whole
//! subtree.

use stylo::servo::restyle_damage::ServoRestyleDamage;

use crate::document::NodeId;

/// The restyle damage produced for one node by a flush.
///
/// A thin, `Copy` wrapper over stylo's [`ServoRestyleDamage`] exposing the
/// four standard damage classes as predicates. The classes are **cumulative**:
/// `RELAYOUT ⊇ RECALCULATE_OVERFLOW ⊇ REBUILD_STACKING_CONTEXT ⊇ REPAINT`, so a
/// relayout also reports [`needs_repaint`](Self::needs_repaint) etc. Any
/// non-empty standard damage repaints.
///
/// v1 uses only stylo's standard low nibble; the custom upper bits
/// (`TElement::compute_layout_damage`) are not populated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleDamage(ServoRestyleDamage);

impl StyleDamage {
    /// The raw damage bits (stylo's `u16` bitflags value).
    #[must_use]
    pub fn bits(self) -> u16 {
        self.0.bits()
    }

    /// Whether there is no damage at all (the node's style did not change, or
    /// the change was style-only with no layout/paint consequence).
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Whether the node (and, transitively, its subtree) must be laid out
    /// again. This is the boundary-crossing damage class layout cares about.
    #[must_use]
    pub fn needs_relayout(self) -> bool {
        self.0.contains(ServoRestyleDamage::RELAYOUT)
    }

    /// Whether the node's scrollable overflow must be recomputed (e.g. a
    /// `transform` change) without a full relayout.
    #[must_use]
    pub fn needs_overflow_recalculation(self) -> bool {
        self.0.contains(ServoRestyleDamage::RECALCULATE_OVERFLOW)
    }

    /// Whether the node's stacking contexts must be rebuilt (e.g. a `z-index`
    /// change) without a full relayout.
    #[must_use]
    pub fn needs_stacking_context_rebuild(self) -> bool {
        self.0
            .contains(ServoRestyleDamage::REBUILD_STACKING_CONTEXT)
    }

    /// Whether the node must be repainted. True for any non-empty standard
    /// damage, since every higher class implies repaint.
    #[must_use]
    pub fn needs_repaint(self) -> bool {
        self.0.contains(ServoRestyleDamage::REPAINT)
    }

    /// Whether this is a full reconstruct (every bit set) — stylo's
    /// "rebuild everything" signal, produced on pseudo-element appear/disappear
    /// and frame reconstruction.
    #[must_use]
    pub fn is_reconstruct(self) -> bool {
        self.0.bits() == u16::MAX
    }
}

impl From<ServoRestyleDamage> for StyleDamage {
    fn from(damage: ServoRestyleDamage) -> Self {
        Self(damage)
    }
}

impl From<StyleDamage> for ServoRestyleDamage {
    fn from(damage: StyleDamage) -> Self {
        damage.0
    }
}

/// The result of a style flush: the per-node damage it produced and whether a
/// traversal actually ran.
///
/// Returned by [`Document::flush_styles`](crate::Document::flush_styles)
/// and friends. Deliberately **not** `#[must_use]`: many callers flush purely
/// for its side effect (styles land on the document) and legitimately ignore
/// the summary in statement position. Layout correctness does not depend on
/// consuming it: relayout-class damage has already invalidated the document's
/// layout caches during harvest. The entries remain useful to downstream
/// paint, stacking-context, and overflow phases.
///
/// `#[non_exhaustive]` so future damage-adjacent fields can be added without a
/// breaking change; construct the empty value with
/// [`FlushSummary::empty`](Self::empty) / `FlushSummary::default()`.
///
/// # Id staleness
///
/// The [`NodeId`]s in [`damage`](Self::damage) are raw slab indices carrying
/// no generation, so a downstream phase that uses the entries must **consume
/// the summary before mutating the document again.** Once a harvested node is freed (a
/// [`remove_subtree`](crate::Document::remove_subtree)) the next node-factory
/// call can reuse its slot, at which point a retained id silently resolves to
/// an *unrelated* node instead of failing closed — there is no generation to
/// detect the reuse. Within a single flush-then-consume step every id is live.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct FlushSummary {
    /// One entry per node whose style changed with non-empty damage, in the
    /// order the harvest visited them (a pre-order spine walk; under
    /// [`Parallelism::Auto`](crate::Parallelism) the *set* is deterministic but
    /// the styling that produced it may have run out of order — compare damage
    /// as a set, not a sequence).
    pub damage: Vec<(NodeId, StyleDamage)>,
    /// Whether stylo's restyle traversal actually ran (its `pre_traverse`
    /// scheduling token said there was work to do). `false` for a no-op flush
    /// (nothing scheduled, or no document root) — the regression signal that
    /// the clear-on-harvest fix keeps repeat flushes from re-traversing.
    pub traversed: bool,
}

impl FlushSummary {
    /// The empty summary (no damage, no traversal) — for no-op / early-return
    /// flush paths.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether the flush produced no damage to act on.
    ///
    /// Note this ignores [`traversed`](Self::traversed): an initial flush
    /// styles a whole subtree yet reports empty damage by design (no old values
    /// to diff), so `is_empty()` is `true` while `traversed` is `true`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.damage.is_empty()
    }
}
