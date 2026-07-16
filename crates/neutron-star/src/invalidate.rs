//! Containment-bounded, damage-driven cache invalidation.
//!
//! The engine owns no dirty state and no parent links: incremental relayout is
//! a **host workflow** over the [`LayoutNode`] cache methods. This module
//! supplies the two pieces that make that workflow containment-aware — a
//! relayout-boundary predicate and an ancestor-walking invalidator — so a host
//! that has classified per-element style damage can invalidate exactly the
//! caches a change can reach, and re-run layout from the nearest containment
//! boundary instead of the document root.
//!
//! # The relayout-boundary theorem
//!
//! A box is a **relayout boundary** iff its effective containment includes
//! **both** [`Contain::LAYOUT`] **and** [`Contain::SIZE`] (i.e. `contain:
//! strict`, or a skipped `content-visibility` box). Under those two together,
//! an internal descendant mutation (a) cannot escape the box's formatting
//! context and (b) cannot change the box's own outer size — so no ancestor or
//! sibling needs re-laying-out, and the box can be re-laid-out in place.
//!
//! **Re-root with the committed input, not `compute_root_layout`.** A
//! boundary's *used* size is frequently **parent-imposed** —
//! `align-items: stretch`, `flex-grow`, a percentage size — so it differs from
//! the size the box would compute for itself. Re-running the box through
//! [`compute_root_layout`](crate::compute::compute_root_layout) (which
//! synthesizes a fresh input from `available_space` only) would re-derive that
//! *self-determined* size and desync the boundary from its un-invalidated
//! ancestors. The boundary is re-runnable only when re-fed the exact
//! [`LayoutInput`] it was committed with, via
//! [`compute_boundary_relayout`](crate::compute::compute_boundary_relayout).
//! `compute_root_layout` stays valid for the **true tree root** (or a boundary
//! whose size is genuinely independent of parent constraints).
//!
//! **Critical caveat — layout alone is not a boundary.** `contain: layout` (or
//! `contain: content`) *without* size containment still lets the container's
//! intrinsic size depend on its contents, so an internal change can resize the
//! container and reflow ancestors. Only `+size` closes the upward path. A
//! definite/known outer size can close it too, but that is a per-layout-input
//! property, not a style property, so [`is_relayout_boundary`] keys off style
//! containment only.
//!
//! # Damage → host action (conceptual translation table)
//!
//! Hosts map their style-system damage classes onto cache actions. Conceptually
//! (naming the four servo `ServoRestyleDamage` classes the stylo fork emits):
//!
//! | Style-system damage | Host action |
//! | --- | --- |
//! | `REPAINT` / `REBUILD_STACKING_CONTEXT` / `RECALCULATE_OVERFLOW` only | No cache work — geometry is unchanged (repaint/stacking/scroll-range are render-side). |
//! | `RELAYOUT` | Capture the boundary's committed input (see the workflow on [`invalidate_for_relayout`]), run [`invalidate_for_relayout`] on the damaged node, then re-run from the returned root with [`compute_boundary_relayout`](crate::compute::compute_boundary_relayout) (or [`compute_root_layout`](crate::compute::compute_root_layout) when it is the tree root). |
//! | reconstruct / `display` change / structural DOM mutation | Same as `RELAYOUT`, but start the ancestor walk from the mutated node's **parent** (box generation changed, so the parent must re-collect its children). |
//!
//! Text-artifact coupling: a host that caches shaped-text artifacts separately
//! from the box cache must invalidate them **together** — the box cache and the
//! artifact cache are coupled by convention (see the leaf/text docs), and
//! clearing one without the other yields stale geometry.
//!
//! What is *not* here: this module never touches [`LayoutInput`] or cache keys.
//! Damage gates *which nodes* get
//! [`cache_clear`](LayoutNode::cache_clear)ed; it never weakens key matching,
//! which stays the complete `LayoutInput`.
//!
//! [`Contain`]: crate::style::Contain
//! [`LayoutInput`]: crate::tree::LayoutInput

use crate::style::{Contain, CoreStyle};
use crate::tree::LayoutNode;

/// Whether `style`'s effective containment makes its box a **relayout
/// boundary** — i.e. it includes both [`Contain::LAYOUT`] and
/// [`Contain::SIZE`].
///
/// Read the module docs' theorem first: `contain: layout` (or `content`) alone
/// is **not** a boundary, because the container's size still depends on its
/// contents. Only layout **and** size together (`contain: strict`, or a
/// skipped `content-visibility` box whose host-reported containment includes
/// both) stop damage from propagating to ancestors. The two effect bits are
/// tested individually (never the `STRICT` marker composite — see the
/// [`containment`](crate::style::containment) module).
#[must_use]
pub fn is_relayout_boundary<S: CoreStyle>(style: &S) -> bool {
    let containment = style.containment();
    containment.contains(Contain::SIZE) && containment.contains(Contain::LAYOUT)
}

/// Clears the caches a relayout of `node` can invalidate, stopping at the
/// nearest containment boundary, and returns the recommended re-layout root
/// handle.
///
/// The engine has no parent links, so the host streams the ancestor path
/// (nearest first, up to the tree root) as `ancestors`. This:
///
/// 1. clears `node`'s own cache;
/// 2. walks `ancestors`, clearing each one's cache — because an ancestor's cached measurement
///    encodes its children's contributions, a size-affecting change to `node` invalidates every
///    ancestor up the chain;
/// 3. **stops at and returns** the first ancestor for which [`is_relayout_boundary`] holds (its own
///    size cannot change, so nothing above it needs re-laying-out — it is the re-layout root);
/// 4. if no ancestor is a boundary, returns the **last** ancestor yielded (the tree root), or
///    `node` itself when `ancestors` is empty.
///
/// The returned handle is the re-layout root: re-running from there walks only
/// the cache-missing subtree, while clean siblings answer from their cache
/// slots at the recursion boundary.
///
/// # Host workflow
///
/// Re-rooting at a containment boundary must **preserve** that boundary's used
/// size, which is often parent-imposed (`align-items: stretch`, `flex-grow`, a
/// percentage size) and would change if re-derived from available space alone
/// (see the module theorem). So:
///
/// 1. **Before** calling this, capture the boundary's committed
///    [`LayoutInput`](crate::tree::LayoutInput) — for a reference-cache host,
///    [`Cache::committed_input`](crate::cache::Cache::committed_input) — because this walk clears
///    that cache slot.
/// 2. Call `invalidate_for_relayout` to clear the boundary-bounded cache path and learn the
///    re-layout root.
/// 3. If the returned root is a boundary, re-run it with the captured input via
///    [`compute_boundary_relayout`](crate::compute::compute_boundary_relayout).
///    [`compute_root_layout`](crate::compute::compute_root_layout) is valid only when the returned
///    root is the **true tree root** (or a boundary whose size is independent of parent
///    constraints); it synthesizes a fresh input and would otherwise resize the boundary.
///
/// This only ever calls [`cache_clear`](LayoutNode::cache_clear); it never
/// reads or weakens cache keys. Hosts must still invalidate any coupled
/// text-artifact caches alongside (see the module docs).
pub fn invalidate_for_relayout<N: LayoutNode>(node: N, ancestors: impl Iterator<Item = N>) -> N {
    node.cache_clear();
    let mut root = node;
    for ancestor in ancestors {
        ancestor.cache_clear();
        root = ancestor;
        if is_relayout_boundary(&ancestor.style()) {
            return ancestor;
        }
    }
    root
}
