//! DOM event paths: the ordered set of node visits one event resolves to.
//!
//! This crate does not dispatch. It cannot: a listener is a JavaScript value
//! in a realm this crate knows nothing about, and reaching one means leaving
//! the thread that owns the document. What it can do is the half that needs
//! the tree, and that no caller could do correctly from outside — work out
//! *which* nodes an event visits, in which order, and which target each of
//! them is allowed to see.
//!
//! [`Document::event_steps`] returns exactly that: the capture pass
//! root-inward followed by the bubble pass target-outward, one [`EventStep`]
//! per visit, computed in a single borrow and owning no reference to the
//! document afterwards. The host takes the list, releases the document, and
//! delivers the steps at its own pace — which is what lets a listener mutate
//! the tree, and what keeps a walk from holding a lock across a call into
//! script.
//!
//! ```
//! # use dom::{Document, NodeId};
//! # fn f(document: &Document<()>, button: NodeId) {
//! for step in document.event_steps(button, true, false).steps() {
//!     println!("{} sees target {}", step.node, step.target);
//!     let _ = step.capture;
//! }
//! # }
//! ```
//!
//! # Shadow trees
//!
//! Path construction is the standard's, including its two shadow rules. The
//! parent of a slotted node is its assigned slot, so an event on light content
//! walks up through the shadow tree that displays it. Crossing out of a shadow
//! tree to its host **retargets**: every step from the host outward reports
//! the host as its [`EventStep::target`], because the outer tree may not learn
//! the identity of a node inside a shadow tree. A non-composed event does not
//! cross out of the tree its target is in at all.
//!
//! # Recorded limits
//!
//! - **The path is a snapshot, and needs no staleness protocol.** It names nodes by [`NodeId`],
//!   which names one node for the life of the document: freeing retires the id, so a step that
//!   outlived its node resolves to nothing rather than to whatever took its storage. A host may
//!   therefore hold a path across a thread hand-off and deliver it later, and the worst a free can
//!   do is make a step reach no one. Detaching changes nothing at all, which is what a re-render
//!   actually does.
//! - **No `composedPath()`.** Reporting it requires the standard's *root-of-closed-tree* and
//!   *slot-in-closed-tree* bookkeeping, whose only consumer is that one method. A closed shadow
//!   root is still honored where it changes the path — retargeting — because that part is not
//!   optional.
//! - **No `relatedTarget`, and no touch target lists.** Both exist for event interfaces this crate
//!   does not model; their retargeting steps are absent with them.
//! - **No event object, no listener registry, no `preventDefault`.** The event's name, its payload,
//!   and whether anything cancels are all above this boundary. Lynx dispatches no cancelable event
//!   in any case, and suppressing a user-agent default action arrives on
//!   [`InputEvent::default_prevented`](crate::input::InputEvent::default_prevented) instead.
//! - **The path ends at the node with no event parent** — the document node for a connected target,
//!   the topmost ancestor for a detached one. A detached target still produces a path, exactly as
//!   the standard specifies.

use smallvec::SmallVec;

use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

/// Inline steps: a tree deeper than this spills to the heap rather than
/// capping the walk. Two passes over the same path, so twice the depth.
const INLINE_STEPS: usize = 32;

/// One node's visit, in the order the host must deliver it.
///
/// Plain `Copy` data owning no borrow of the document: the host computes a
/// whole list, releases the document, and delivers from the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EventStep {
    /// The node whose listeners run — the standard's `currentTarget`.
    pub node: NodeId,
    /// The event's `target` as this node may see it. Equal to the dispatch
    /// target except above a shadow boundary, where it is the host that
    /// stands in for it.
    ///
    /// Equal to [`Self::node`] exactly at an at-target step, which is what a
    /// consumer derives the standard's `eventPhase` from: at-target in either
    /// pass, otherwise capturing or bubbling according to [`Self::capture`].
    /// The number itself is a property of an event object, and this crate has
    /// none.
    pub target: NodeId,
    /// Which listener set this step runs. Not derivable from the phase: at the
    /// target both passes are at-target, and each runs a different set.
    pub capture: bool,
}

/// The steps one event resolves to.
///
/// Plain data with no tie to the document it came from. A step names its nodes
/// by [`NodeId`], and an id names one node for the life of the document — a
/// handle taken before a free stops matching, so a step that outlived its node
/// resolves to nothing rather than to whatever took its storage. That is what
/// lets a path be handed to another thread and delivered later with no
/// staleness protocol at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSteps {
    steps: SmallVec<[EventStep; INLINE_STEPS]>,
}

impl EventSteps {
    #[must_use]
    pub fn steps(&self) -> &[EventStep] {
        &self.steps
    }
}

/// One node on the computed path, before the two passes are laid out.
#[derive(Debug, Clone, Copy)]
struct PathEntry {
    /// The standard's *invocation target*.
    node: NodeId,
    /// The standard's `target` while this entry is invoked: the nearest
    /// shadow-adjusted target at or below it.
    target: NodeId,
    /// Whether this entry has a shadow-adjusted target of its own, which is
    /// what makes it an at-target entry: both passes visit it, and each runs a
    /// different listener set.
    at_target: bool,
}

impl<T> Document<T> {
    /// The ordered node visits an event at `target` resolves to.
    ///
    /// The capture pass runs root-inward, then the bubble pass runs
    /// target-outward. The target appears in both, since each pass runs a
    /// different listener set on it. `bubbles` drops the non-target steps of
    /// the bubble pass; `composed` lets the path leave the shadow tree its
    /// target is in.
    ///
    /// <https://dom.spec.whatwg.org/#event-path>
    ///
    /// # Panics
    ///
    /// If `target` is not a live node.
    #[must_use]
    pub fn event_steps(&self, target: NodeId, bubbles: bool, composed: bool) -> EventSteps {
        assert!(
            self.contains_node(target),
            "Document::event_steps: stale target NodeId"
        );
        let path = self.event_path(target, composed);
        let mut steps = SmallVec::new();

        for entry in path.iter().rev() {
            steps.push(EventStep {
                node: entry.node,
                target: entry.target,
                capture: true,
            });
        }
        for entry in &path {
            if !entry.at_target && !bubbles {
                continue;
            }
            steps.push(EventStep {
                node: entry.node,
                target: entry.target,
                capture: false,
            });
        }

        EventSteps { steps }
    }

    /// The standard's event path, target first and root last.
    ///
    /// The standard makes an entry a *shadow-adjusted target* when the root of
    /// the current target is no longer a shadow-including inclusive ancestor of
    /// the entry, which read literally is an ancestor walk per step. On an
    /// upward walk it has a one-comparison equivalent, used here: the predicate
    /// fails **exactly** when the step leaves the current target's own tree,
    /// and the only step that can do that is a shadow root moving to its host.
    ///
    /// Every other step stays inside that tree or descends into a shadow tree
    /// nested within it. Stepping to an assigned slot is the case worth
    /// stating: the slot lives in the shadow tree of the node's own parent, so
    /// the current root is still an ancestor of it through the host, and light
    /// content keeps its identity all the way out — which is why a slotted node
    /// is never retargeted while a node inside the shadow tree always is.
    ///
    /// <https://dom.spec.whatwg.org/#event-path>
    fn event_path(&self, target: NodeId, composed: bool) -> SmallVec<[PathEntry; INLINE_STEPS]> {
        let mut path = SmallVec::new();
        path.push(PathEntry {
            node: target,
            target,
            at_target: true,
        });

        let target_root = self.tree_root(target);
        let mut current_target = target;
        let mut current_root = target_root;
        let mut node = target;

        while let Some(parent) = self.event_parent(node, composed, target_root) {
            let at_target = node == current_root && self.is_shadow_root(node);
            if at_target {
                current_target = parent;
                current_root = self.tree_root(parent);
            }
            path.push(PathEntry {
                node: parent,
                target: current_target,
                at_target,
            });
            node = parent;
        }
        path
    }

    fn is_shadow_root(&self, node: NodeId) -> bool {
        self.get(node).is_some_and(Node::is_shadow_root)
    }

    /// The standard's *get the parent*: a slotted node's parent is its assigned
    /// slot, and a shadow root's is its host unless a non-composed event would
    /// have to leave the tree it started in.
    ///
    /// <https://dom.spec.whatwg.org/#get-the-parent>
    fn event_parent(&self, node: NodeId, composed: bool, target_root: NodeId) -> Option<NodeId> {
        let live = self.get(node)?;
        if live.is_shadow_root() {
            if !composed && node == target_root {
                return None;
            }
            return self.shadow_host(node);
        }
        if let Some(slot) = self.assigned_slot(node) {
            return Some(slot);
        }
        live.parent_id()
    }

    /// The root of `node`'s own tree: a shadow root, the document node, or the
    /// topmost ancestor of a detached subtree.
    ///
    /// A shadow root's [`Node::parent_id`] is its host in this crate, so a
    /// plain parent walk would run straight out of the shadow tree and report
    /// the document. The shadow-tree case has to be asked for by name.
    fn tree_root(&self, node: NodeId) -> NodeId {
        if self.is_shadow_root(node) {
            return node;
        }
        if let Some(root) = self.containing_shadow_root(node) {
            return root;
        }
        let mut current = node;
        while let Some(parent) = self.get(current).and_then(Node::parent_id) {
            current = parent;
        }
        current
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
