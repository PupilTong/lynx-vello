//! The stylo-traversal-driven style flush.
//!
//! [`Document::flush_styles`] restyles everything scheduled since the
//! last flush by driving **stylo's own restyle traversal**
//! ([`driver::traverse_dom`]) over the document — in place, on the one tree
//! (the one-word `&Node` reference is the traversal's element type; no
//! mirror tree is built). That buys, in one move:
//!
//! - **Parallelism**: Firefox-style rayon work-stealing over wide DOM levels (via stylo's global
//!   [`STYLE_THREAD_POOL`]), with a sequential fallback for small trees driven by stylo's own
//!   work-unit heuristics.
//! - **Invalidation sets**: pending node snapshots (recorded by the document's setters, see
//!   [`crate::invalidation`]) are matched against the stylist's dependency maps, so a class flip
//!   restyles only the nodes whose rules could be affected.
//! - **The style sharing cache and bloom filter**, managed per worker by stylo's
//!   `ThreadLocalStyleContext`.
//!
//! Computed styles land in each element node's stylo `ElementData`
//! ([`Node::computed_style`](crate::Node::computed_style) reads them); the
//! document's harvest then consumes relayout-class damage into layout-cache
//! invalidation, exposes all per-node damage through a [`FlushSummary`] or
//! sink, drops the consumed snapshots, and clears stylo's restyle state so the
//! next flush does not re-traverse. The harvest is rooted at the traversal's
//! **actual** root, which stylo may raise to the passed root's parent when a
//! subtree flush invalidated the root's siblings (see
//! [`Document::flush_styles_with_sink`]).
//!
//! # Safety
//!
//! The one `unsafe` block calls `TElement::ensure_data` from
//! `process_preorder`, which stylo's traversal contract guarantees is invoked
//! by exactly one worker per node.
#![allow(unsafe_code)]

use stylo::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
    StyleSystemOptions,
};
use stylo::dom::{TElement, TNode};
use stylo::driver;
use stylo::global_style_data::STYLE_THREAD_POOL;
use stylo::servo::animation::DocumentAnimationSet;
use stylo::shared_lock::StylesheetGuards;
use stylo::thread_state::{self, ThreadState};
use stylo::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use stylo::traversal_flags::TraversalFlags;
use stylo_atoms::Atom;

use crate::damage::{FlushSummary, StyleDamage};
use crate::document::{Document, NodeId};
use crate::node::Node;

/// How [`Document::flush_styles_with`] schedules the traversal.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Parallelism {
    /// Use stylo's global style thread pool when it exists; stylo still
    /// starts sequentially and only fans out on DOM levels wide enough to
    /// amortize the work-stealing overhead.
    #[default]
    Auto,
    /// Force a fully sequential traversal (deterministic scheduling;
    /// benchmarking baseline).
    Sequential,
}

/// The CSS Paint API is unsupported: no speculative painters are registered.
#[derive(Debug)]
struct NoPainters;

impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

static NO_PAINTERS: NoPainters = NoPainters;

/// Serializes parallel traversals process-wide. stylo's global style pool
/// assumes one traversal at a time (its workers keep per-traversal state in
/// TLS — the style sharing cache, bloom filter); Servo guarantees that by
/// architecture (a single layout thread), we guarantee it here. Uncontended
/// in the intended one-flusher-thread setup.
static STYLE_POOL_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Balances [`thread_state::enter`] on unwind, so a panicking traversal does
/// not leave the embedder's thread permanently flagged `LAYOUT`.
struct LayoutThreadStateGuard {
    entered: bool,
}

impl LayoutThreadStateGuard {
    fn enter() -> Self {
        let entered = !thread_state::get().is_layout();
        if entered {
            thread_state::enter(ThreadState::LAYOUT);
        }
        Self { entered }
    }
}

impl Drop for LayoutThreadStateGuard {
    fn drop(&mut self) {
        if self.entered {
            thread_state::exit(ThreadState::LAYOUT);
        }
    }
}

/// The restyle-only traversal: recalculate styles preorder, no postorder pass.
struct RecalcStyle<'a> {
    shared: SharedStyleContext<'a>,
}

impl<'a, T: Sync> DomTraversal<&'a Node<T>> for RecalcStyle<'a> {
    fn process_preorder<F>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<&'a Node<T>>,
        node: &'a Node<T>,
        note_child: F,
    ) where
        F: FnMut(&'a Node<T>),
    {
        // Text nodes remain in DOM/layout child iteration, but stylo only
        // schedules preorder style work for nodes whose `as_element()`
        // succeeds.
        let element = node
            .as_element()
            .expect("style traversal only schedules element nodes");
        // SAFETY: stylo's traversal contract — exactly one worker processes
        // this node, so creating/borrowing its data cannot race.
        let mut data = unsafe { element.ensure_data() };
        recalc_style_at(
            self,
            traversal_data,
            context,
            element,
            &mut data,
            note_child,
        );
    }

    fn process_postorder(&self, _: &mut StyleContext<&'a Node<T>>, _: &'a Node<T>) {
        debug_assert!(false, "needs_postorder_traversal() is false");
    }

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.shared
    }
}

impl<T: Sync> Document<T> {
    /// Restyle everything scheduled since the last flush under the document
    /// element, using the style thread pool when the tree is wide enough
    /// ([`Parallelism::Auto`]).
    ///
    /// Returns a [`FlushSummary`] of the per-node restyle damage the flush
    /// produced (see [`StyleDamage`]). Initial styling of a subtree produces
    /// **no** damage by design — there are no old computed values to diff, so
    /// embedders lay out a freshly styled subtree from their own structural
    /// knowledge. A `display: none → visible` flip does produce `RELAYOUT`
    /// damage on the flipped node, which covers its whole subtree.
    /// Relayout-class damage has already invalidated the document's layout
    /// caches before this method returns, so discarding the summary cannot make
    /// a later [`layout`](Self::layout) reuse stale layout.
    ///
    /// A no-op (empty summary, `traversed == false`) when the document has no
    /// element child or nothing is scheduled.
    ///
    /// If the traversal panics, the document's scheduling state (dirty bits,
    /// pending snapshots) is left unspecified; an embedder that catches the
    /// unwind should discard or rebuild the document rather than keep
    /// flushing it.
    pub fn flush_styles(&mut self) -> FlushSummary {
        self.flush_styles_with(Parallelism::Auto)
    }

    /// [`flush_styles`](Self::flush_styles) with explicit traversal
    /// scheduling.
    ///
    /// Collects the harvested damage into the returned [`FlushSummary`]'s
    /// `Vec`. Embedders that want to avoid that allocation stream the damage
    /// directly with
    /// [`flush_styles_with_sink`](Self::flush_styles_with_sink).
    pub fn flush_styles_with(&mut self, parallelism: Parallelism) -> FlushSummary {
        let mut damage = Vec::new();
        let traversed = self.flush_styles_with_sink(parallelism, &mut |id, d| {
            damage.push((id, d));
        });
        FlushSummary { damage, traversed }
    }

    /// The allocation-free damage-delivery primitive: restyle under the
    /// document element, then stream each node's non-empty restyle damage to
    /// `sink` as it is harvested, instead of collecting it into a `Vec`.
    /// Relayout-class damage first drives the document's internal cache
    /// invalidation; the sink remains for paint/stacking/overflow consumers and
    /// observability. Returns whether the traversal ran (stylo's `pre_traverse`
    /// scheduling token said there was work) — the `traversed` flag
    /// [`flush_styles_with`](Self::flush_styles_with) records.
    ///
    /// `sink` is a `&mut dyn FnMut` rather than a generic `impl FnMut` so the
    /// harvest walk (already monomorphized per opaque payload `T`) is
    /// not additionally monomorphized per closure; the per-node dynamic call
    /// is negligible next to the cascade work that produced the damage.
    ///
    /// The harvest walks from the traversal's **actual** root, which stylo's
    /// `pre_traverse` (`vendor/stylo/style/traversal.rs`) may substitute: when
    /// a flush root's snapshot invalidated its *siblings*, the traversal is
    /// raised to the root's **parent** (the restyled siblings live under it,
    /// and `propagate_dirty_bit_up_to` sets its `dirty_descendants`).
    /// `driver::traverse_dom` returns that actual root; harvesting from it —
    /// rather than the passed root — is what reaches (and clears) the siblings'
    /// damage and the parent's dirty bit. Flushes here root at the document
    /// **element** (`root_element`), for which the substitution is
    /// structurally impossible: it has no element siblings for a snapshot to
    /// invalidate (a document owns one element child), and stylo's raise path
    /// resolves the substitute via `parent_element_or_host()`, which is `None`
    /// for the document element (its parent is the slot-zero document *node*).
    /// The harvest still follows the driver's returned root by contract — and
    /// tolerates a non-element root defensively (no `ElementData`, so it
    /// yields no damage and only clears its own bits) — purely as insurance
    /// for a future subtree-flush entry point, where real element parents and
    /// siblings exist. Without a traversal the passed root is harvested
    /// directly (it is always inspected, even with no dirty bits).
    ///
    /// # Panics
    ///
    /// As [`flush_styles_with`](Self::flush_styles_with).
    pub fn flush_styles_with_sink(
        &mut self,
        parallelism: Parallelism,
        sink: &mut dyn FnMut(NodeId, StyleDamage),
    ) -> bool {
        let Some(root) = self.root_element().map(Node::id) else {
            // No document element: no traversal, and nothing to harvest.
            return false;
        };
        // Nodes own snapshots between mutations and flushes. Stylo's API
        // expects a map for the traversal, so drain the reachable snapshots
        // along the dirty spine into this temporary adapter; it is dropped
        // when this flush returns.
        let snapshots = self.take_snapshot_map(root);
        // Debug traversal-phase marker: per-node style readers assert they
        // never run concurrently with the traversal, and the trait accessors
        // assert they only run inside one. Cleared on unwind too (individual
        // slot poisoning covers mid-panic node state).
        #[cfg(debug_assertions)]
        let phase = self.begin_flush_phase();
        let (harvest_root, traversed) = {
            let root_ref = self
                .get(root)
                .expect("the root element child is kept live or absent");
            let guard = self.style_engine().shared_lock().read();
            let shared = SharedStyleContext {
                stylist: self.style_engine().stylist(),
                visited_styles_enabled: false,
                options: StyleSystemOptions::default(),
                guards: StylesheetGuards::same(&guard),
                // Animations are future work (docs/style-assumptions.md
                // §C.11/§C.12): the clock is pinned at 0 and the animation
                // set is per-flush, so declared animations/transitions stay
                // at their start value. The render/runtime layer will own
                // the real clock and a persistent `DocumentAnimationSet`.
                current_time_for_animations: 0.0,
                traversal_flags: TraversalFlags::empty(),
                snapshot_map: &snapshots,
                animations: DocumentAnimationSet::default(),
                registered_speculative_painters: &NO_PAINTERS,
            };
            let traversal = RecalcStyle { shared };
            let token = <RecalcStyle<'_> as DomTraversal<&Node<T>>>::pre_traverse(
                root_ref,
                &traversal.shared,
            );
            // Read the scheduling decision before the token is moved into
            // `traverse_dom`.
            let should_traverse = token.should_traverse();
            // `driver::traverse_dom` returns the traversal's actual root (the
            // passed root, or its parent when a sibling invalidation raised
            // it). `NodeId` is `Copy`, so capture the harvest root inside this
            // shared-borrow scope before `root_ref` (and the shared context
            // borrowing it) go out of scope.
            let harvest_root = if should_traverse {
                // stylo's sequential-task teardown asserts it runs on a
                // LAYOUT thread (its pool workers are initialized as such);
                // mark the embedder's calling thread for the traversal.
                let _thread_state = LayoutThreadStateGuard::enter();
                match parallelism {
                    Parallelism::Sequential => {
                        Node::id(driver::traverse_dom(&traversal, token, None))
                    }
                    Parallelism::Auto => {
                        let _pool_guard = STYLE_POOL_GUARD
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let pool = STYLE_THREAD_POOL.pool();
                        Node::id(driver::traverse_dom(&traversal, token, pool.as_ref()))
                    }
                }
            } else {
                // No traversal ran, so the actual root is the passed root.
                root
            };
            (harvest_root, should_traverse)
        };
        // `invalidate_layout` reads the freshly computed styles while harvest
        // consumes relayout damage. The parallel traversal has joined, so end
        // its debug phase before those ordinary style reads begin.
        #[cfg(debug_assertions)]
        drop(phase);
        // Harvest runs under `&mut Document` now that the traversal (which
        // borrowed the document through `root_ref`/the shared context) has
        // finished.
        self.harvest_flush(harvest_root, &snapshots, sink);
        traversed
    }
}
