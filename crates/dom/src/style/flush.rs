//! The stylo-traversal-driven style flush.

use stylo::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
    StyleSystemOptions,
};
use stylo::dom::{TElement, TNode};
use stylo::driver;
use stylo::global_style_data::STYLE_THREAD_POOL;
use stylo::shared_lock::StylesheetGuards;
use stylo::thread_state::{self, ThreadState};
use stylo::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use stylo::traversal_flags::TraversalFlags;
use stylo_atoms::Atom;

use crate::style::damage::StyleDamage;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

#[cfg(target_arch = "wasm32")]
static EMBEDDER_STYLE_THREAD_POOL: std::sync::OnceLock<rayon::ThreadPool> =
    std::sync::OnceLock::new();

/// Installs the process-wide style traversal pool supplied by a Wasm embedder.
///
/// The persistent presenting owner occupies index zero. Script owners enter
/// traversal from outside the pool, and Stylo moves their root closures onto a
/// managed worker. It must be installed before the first style flush.
/// Subsequent calls return the supplied pool unchanged.
#[cfg(target_arch = "wasm32")]
pub fn install_style_thread_pool(pool: rayon::ThreadPool) -> Result<(), rayon::ThreadPool> {
    EMBEDDER_STYLE_THREAD_POOL.set(pool)
}

/// The CSS Paint API is unsupported: no speculative painters are registered.
#[derive(Debug)]
pub(super) struct NoPainters;

impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

pub(super) static NO_PAINTERS: NoPainters = NoPainters;

/// Serializes style traversals across every document in the process.
///
/// Stylo's bloom filter and style-sharing cache do not own their buffers: each
/// takes an `AtomicRefMut` on a leaked, per-OS-thread `AtomicRefCell`
/// (`StyleBloom::new` borrows `BLOOM_KEY`, `StyleSharingCache` borrows
/// `SHARING_CACHE_KEY`), so a second live instance on one thread panics on the
/// borrow. Both sit in a `ThreadLocalStyleContext`, and those contexts live in
/// the `ScopedTLS` that [`driver::traverse_dom`] owns for the whole call — so
/// every pool worker a traversal touched keeps its two buffers borrowed until
/// that traversal returns, not until it finishes a chunk. A rayon worker
/// waiting on its own scope runs any job it can find, including a chunk
/// belonging to a different traversal; that chunk builds a second
/// `ThreadLocalStyleContext` on a thread that already holds the borrows.
///
/// The mutex is process-wide because the pool is: [`STYLE_THREAD_POOL`] is a
/// global, and a Wasm embedder installs one pool for the module. Per-document
/// would protect nothing — `flush_styles_with_damage_sink` takes `&mut self`,
/// so one document already cannot traverse twice at once, and the collision is
/// between separate documents whose traversals share worker threads. Narrowing
/// it means guaranteeing no OS thread serves two traversals, which takes one
/// pool per concurrently flushing document; until something supplies those
/// thread sets, the pool's granularity is the guard's.
///
/// Taken only after `pre_traverse` reports work, so a flush with nothing to
/// restyle never touches it. The animation-only traversal passes no pool and
/// runs inline on the caller's thread, which is why it is not guarded.
static STYLE_POOL_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Balances [`thread_state::enter`] on unwind, so a panicking traversal does
/// not leave the embedder's thread permanently flagged `LAYOUT`.
pub(super) struct LayoutThreadStateGuard {
    entered: bool,
}

impl LayoutThreadStateGuard {
    pub(super) fn enter() -> Self {
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
pub(super) struct RecalcStyle<'a> {
    shared: SharedStyleContext<'a>,
}

impl<'a> RecalcStyle<'a> {
    pub(super) const fn new(shared: SharedStyleContext<'a>) -> Self {
        Self { shared }
    }

    pub(super) const fn shared(&self) -> &SharedStyleContext<'a> {
        &self.shared
    }
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
        let element = node
            .as_element()
            .expect("style traversal only schedules element nodes");
        #[expect(
            unsafe_code,
            reason = "TElement::ensure_data is an unsafe trait entry point; the traversal owns this element exclusively"
        )]
        // SAFETY: `ensure_data`'s precondition is exclusive access to the
        // element — `ElementDataWrapper` is an `UnsafeCell<ElementData>` whose
        // `AtomicRefCell` guard is compiled in only under `debug_assertions`,
        // so an overlapping `borrow_mut` aliases `&mut` unchecked in release.
        // The driver supplies that exclusivity: an element is queued only by
        // its one parent's `note_child`, work units are disjoint ranges split
        // off that queue, and each entry is popped into `process_preorder`
        // exactly once.
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
    pub(crate) fn flush_styles_with_damage_sink<F>(&mut self, sink: &mut F)
    where
        F: FnMut(NodeId, StyleDamage),
    {
        let root = self.document_element();
        if !root.needs_style_flush() {
            return;
        }
        let root = root.id();
        let snapshots = self.take_snapshot_map();
        let phase = self.begin_flush_phase();
        let harvest_root = {
            let root_ref = self
                .get(root)
                .expect("the root element child is kept live or absent");
            let guard = self.style_engine().shared_lock().read();
            let shared = SharedStyleContext {
                stylist: self.style_engine().stylist(),
                visited_styles_enabled: false,
                options: StyleSystemOptions::default(),
                guards: StylesheetGuards::same(&guard),
                current_time_for_animations: self.animations().now(),
                traversal_flags: TraversalFlags::empty(),
                snapshot_map: &snapshots,
                animations: self.animations().context_handle(),
                registered_speculative_painters: &NO_PAINTERS,
            };
            let traversal = RecalcStyle { shared };
            let token = <RecalcStyle<'_> as DomTraversal<&Node<T>>>::pre_traverse(
                root_ref,
                &traversal.shared,
            );
            let should_traverse = token.should_traverse();
            if should_traverse {
                let _thread_state = LayoutThreadStateGuard::enter();
                let _pool_guard = STYLE_POOL_GUARD
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                #[cfg(target_arch = "wasm32")]
                if let Some(pool) = EMBEDDER_STYLE_THREAD_POOL.get() {
                    Node::id(driver::traverse_dom(&traversal, token, Some(pool)))
                } else {
                    let pool = STYLE_THREAD_POOL.pool();
                    Node::id(driver::traverse_dom(&traversal, token, pool.as_ref()))
                }

                #[cfg(not(target_arch = "wasm32"))]
                {
                    let pool = STYLE_THREAD_POOL.pool();
                    Node::id(driver::traverse_dom(&traversal, token, pool.as_ref()))
                }
            } else {
                root
            }
        };
        drop(phase);
        self.harvest_flush(harvest_root, snapshots, sink);
        // The flush is where animations start and stop; the timeline has to
        // learn what it now owns before the next frame asks whether to tick.
        self.sync_animation_state();
        // Every declaration block the flush retired — one per inline-style
        // write — leaves its rule node on the tree's free list, and nothing
        // else in the engine ever drains it. Servo runs the same collection
        // after each reflow; the call is a counter check until 300 nodes have
        // accumulated, so a flush that retires nothing pays nothing.
        self.style_engine().stylist().rule_tree().maybe_gc();
    }
}
