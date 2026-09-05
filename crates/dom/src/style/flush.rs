//! The stylo-traversal-driven style flush.

use stylo::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
    StyleSystemOptions,
};
use stylo::dom::{TElement, TNode};
use stylo::driver;
use stylo::shared_lock::StylesheetGuards;
use stylo::thread_state::{self, ThreadState};
use stylo::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use stylo::traversal_flags::TraversalFlags;
use stylo_atoms::Atom;

use crate::style::damage::StyleDamage;
use crate::style::pool::StylePool;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

/// The CSS Paint API is unsupported: no speculative painters are registered.
#[derive(Debug)]
pub(super) struct NoPainters;

impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

pub(super) static NO_PAINTERS: NoPainters = NoPainters;

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
                // This thread's workers, or none — in which case the
                // traversal runs here, on the thread that flushes. Nothing
                // reaches for a process-wide pool, so nothing has to be
                // serialized against a document on another thread.
                let pool = self.style_pool().map(StylePool::rayon);
                Node::id(driver::traverse_dom(&traversal, token, pool))
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
