//! The stylo-traversal-driven style flush.
//!
//! [`StyleEngine::flush_document`] restyles everything scheduled since the
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
//! document then clears the consumed snapshot flags and the dirty spine.
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

use crate::document::Document;
use crate::engine::StyleEngine;
use crate::ext::ExternalState;
use crate::node::Node;
use crate::traits::DomNode;

/// How [`StyleEngine::flush_document_with`] schedules the traversal.
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

impl<'a, T: ExternalState> DomTraversal<&'a Node<T>> for RecalcStyle<'a> {
    fn process_preorder<F>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<&'a Node<T>>,
        node: DomNode<'a, T>,
        note_child: F,
    ) where
        F: FnMut(DomNode<'a, T>),
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

    fn process_postorder(&self, _: &mut StyleContext<&'a Node<T>>, _: DomNode<'a, T>) {
        debug_assert!(false, "needs_postorder_traversal() is false");
    }

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.shared
    }
}

impl StyleEngine {
    /// Restyle everything scheduled since the last flush under the document
    /// element, using the style thread pool when the tree is wide enough
    /// ([`Parallelism::Auto`]).
    ///
    /// A no-op when the document has no element child or nothing is
    /// scheduled.
    ///
    /// If the traversal panics, the document's scheduling state (dirty bits,
    /// pending snapshots) is left unspecified; an embedder that catches the
    /// unwind should discard or rebuild the document rather than keep
    /// flushing it.
    pub fn flush_document<T: ExternalState>(&self, document: &mut Document<T>) {
        self.flush_document_with(document, Parallelism::Auto);
    }

    /// [`flush_document`](Self::flush_document) with explicit traversal
    /// scheduling.
    ///
    /// # Panics
    ///
    /// Panics when `document` was not created by this engine
    /// (`StyleEngine::new_document` pairs them; flushing across the pair
    /// boundary would run the wrong stylist and take the wrong lock), or
    /// if an internal document-element link is dangling — impossible through
    /// the public mutation API.
    pub fn flush_document_with<T: ExternalState>(
        &self,
        document: &mut Document<T>,
        parallelism: Parallelism,
    ) {
        self.assert_owns(document);
        let Some(root) = document.document_element() else {
            return;
        };
        // Nodes own snapshots between mutations and flushes. Stylo's API
        // expects a map for the traversal, so drain the reachable snapshots
        // along the dirty spine into this temporary adapter; it is dropped
        // when this flush returns.
        let snapshots = document.take_snapshot_map(root);
        // Debug traversal-phase marker: per-node style readers assert they
        // never run concurrently with the traversal, and the trait accessors
        // assert they only run inside one. Cleared on unwind too (individual
        // slot poisoning covers mid-panic node state).
        #[cfg(debug_assertions)]
        let _phase = document.begin_flush_phase();
        {
            let root_ref = document
                .get(root)
                .expect("the document element is kept live or unset");
            let guard = self.shared_lock().read();
            let shared = SharedStyleContext {
                stylist: self.stylist(),
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
            if token.should_traverse() {
                // stylo's sequential-task teardown asserts it runs on a
                // LAYOUT thread (its pool workers are initialized as such);
                // mark the embedder's calling thread for the traversal.
                let _thread_state = LayoutThreadStateGuard::enter();
                match parallelism {
                    Parallelism::Sequential => {
                        driver::traverse_dom(&traversal, token, None);
                    }
                    Parallelism::Auto => {
                        let _pool_guard = STYLE_POOL_GUARD
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let pool = STYLE_THREAD_POOL.pool();
                        driver::traverse_dom(&traversal, token, pool.as_ref());
                    }
                }
            }
        }
        document.complete_flush(root, &snapshots);
    }
}
