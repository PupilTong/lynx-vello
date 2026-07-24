//! The stylo-traversal-driven style flush.
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

use crate::damage::{FlushStatus, FlushSummary, StyleDamage, StyleDamageEntry};
use crate::document::{Document, NodeId};
use crate::node::Node;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Parallelism {
    #[default]
    Auto,
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
        let element = node
            .as_element()
            .expect("style traversal only schedules element nodes");
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
    pub fn flush_styles(&mut self) -> FlushSummary {
        self.flush_styles_with_parallelism(Parallelism::Auto)
    }

    pub fn flush_styles_with_parallelism(&mut self, parallelism: Parallelism) -> FlushSummary {
        let mut damage = Vec::new();
        let status =
            self.flush_styles_with_damage_sink(parallelism, &mut |node_id, damage_value| {
                damage.push(StyleDamageEntry {
                    node_id,
                    damage: damage_value,
                });
            });
        FlushSummary { damage, status }
    }

    pub fn flush_styles_with_damage_sink(
        &mut self,
        parallelism: Parallelism,
        sink: &mut dyn FnMut(NodeId, StyleDamage),
    ) -> FlushStatus {
        let Some(root) = self.root_element() else {
            return FlushStatus::Skipped;
        };
        if !root.needs_style_flush() {
            return FlushStatus::Skipped;
        }
        let root = root.id();
        let snapshots = self.take_snapshot_map();
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
            let should_traverse = token.should_traverse();
            let harvest_root = if should_traverse {
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
                root
            };
            (harvest_root, should_traverse)
        };
        #[cfg(debug_assertions)]
        drop(phase);
        self.harvest_flush(harvest_root, snapshots, sink);
        if traversed {
            FlushStatus::Traversed
        } else {
            FlushStatus::Skipped
        }
    }
}
