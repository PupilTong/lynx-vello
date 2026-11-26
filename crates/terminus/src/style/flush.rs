//! The stylo-traversal-driven style flush.

use stylo::computed_value_flags::ComputedValueFlags;
use stylo::context::{
    RegisteredSpeculativePainter, RegisteredSpeculativePainters, SharedStyleContext, StyleContext,
    StyleSystemOptions,
};
use stylo::dom::{TElement, TNode};
use stylo::driver;
use stylo::thread_state::{self, ThreadState};
use stylo::traversal::{DomTraversal, recalc_style_at};
use stylo::traversal_flags::TraversalFlags;
use stylo_atoms::Atom;

use crate::style::damage::StyleDamage;
use crate::style::pool::StylePool;
use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

#[derive(Debug)]
pub(super) struct NoPainters;

impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

pub(super) static NO_PAINTERS: NoPainters = NoPainters;

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

pub(super) struct RecalcStyle<'a> {
    shared: SharedStyleContext<'a>,
    container_units: &'a std::sync::atomic::AtomicBool,
}

impl<'a> RecalcStyle<'a> {
    pub(super) const fn new(
        shared: SharedStyleContext<'a>,
        container_units: &'a std::sync::atomic::AtomicBool,
    ) -> Self {
        Self {
            shared,
            container_units,
        }
    }

    pub(super) const fn shared(&self) -> &SharedStyleContext<'a> {
        &self.shared
    }
}

impl<'a, T: Sync> DomTraversal<&'a Node<T>> for RecalcStyle<'a> {
    fn process_preorder<F>(
        &self,
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
        recalc_style_at(self, context, element, &mut data, note_child);
        if data.styles.get_primary().is_some_and(|style| {
            style
                .flags
                .contains(ComputedValueFlags::USES_CONTAINER_UNITS)
        }) {
            self.container_units
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
            if self.backdrops_need_restyle() {
                self.restyle_backdrops();
            }
            self.rebuild_anonymous_boxes();
            return;
        }
        let root = root.id();
        let snapshots = self.take_snapshot_map();
        let phase = self.begin_flush_phase();
        let harvest_root = {
            let root_ref = self
                .get(root)
                .expect("the root element child is kept live or absent");
            let guards = self.style_engine().rule_guards();
            let shared = SharedStyleContext {
                stylist: self.style_engine().stylist(),
                visited_styles_enabled: false,
                options: StyleSystemOptions::default(),
                guards: guards.guards(),
                current_time_for_animations: self.animations().now(),
                traversal_flags: TraversalFlags::empty(),
                snapshot_map: &snapshots,
                animations: self.animations().context_handle(),
                registered_speculative_painters: &NO_PAINTERS,
            };
            let traversal = RecalcStyle::new(shared, self.arenas().container_units_flag());
            let token = <RecalcStyle<'_> as DomTraversal<&Node<T>>>::pre_traverse(
                root_ref,
                &traversal.shared,
            );
            let should_traverse = token.should_traverse();
            if should_traverse {
                let _thread_state = LayoutThreadStateGuard::enter();
                let pool = self.style_pool().map(StylePool::rayon);
                Node::id(driver::traverse_dom(&traversal, token, pool))
            } else {
                root
            }
        };
        drop(phase);
        let anchor_restyled = self.harvest_flush(harvest_root, snapshots, sink);
        self.refresh_anchor_state(
            &anchor_restyled,
            crate::layout::anchors::RestyleSource::Flush,
        );
        self.restyle_backdrops();
        self.rebuild_anonymous_boxes();
        self.sync_animation_state();
        self.style_engine().stylist().rule_tree().maybe_gc();
    }
}
