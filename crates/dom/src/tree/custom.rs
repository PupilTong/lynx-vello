//! Custom elements: the per-local-name definition set, the element state
//! machine, and the reaction queue that runs the lifecycle callbacks.
//!
//! A definition here is one handler per local name, not one constructed object
//! per element: this crate has no script realm to hold instances in, so every
//! callback names its element by [`NodeId`] and per-element state belongs to
//! the layer that owns the payload `T`.
//!
//! Reactions are **queued, never called inline**, and drained at the end of the
//! public mutation that raised them — HTML's `[CEReactions]` boundary. Both
//! halves matter. Queuing is what keeps author code out of the middle of a DOM
//! algorithm: [`Document::insert_before`] detaches unconditionally before it
//! relinks, so a callback dispatched from inside that detach could observe one
//! node in two parents' child lists. Draining before returning is what keeps
//! this crate's contract uniform — slot assignment resolves inside the mutation
//! that changed it, so nothing else here may leave work for a later "please
//! resolve" call either.
//!
//! # Scope: user-agent components, not script-defined elements
//!
//! Definitions here are installed by the layer above — the engine's own
//! components — not by application script, and that single fact removes the
//! standard's entire *upgrade* half. Upgrade exists because a script can define
//! a tag at any moment, so an element may outlive its own definitionlessness; a
//! definition compiled into the engine always precedes every element with its
//! tag. [`Document::define`] therefore **requires** that, and panics rather
//! than accepting a definition that would arrive too late to reach anything.
//!
//! What that removes: the `undefined` state and with it every `:defined`
//! transition, *upgrade an element*, *try to upgrade*, the `define`-time
//! document sweep, the replay of attributes an element already carried, and the
//! *valid custom element name* predicate — whose only job was deciding whether
//! a definitionless element counted as `undefined`.
//!
//! What it does **not** remove, and what a reader coming from a browser
//! implementation will expect to be simpler than it is: the reaction queue and
//! its drain boundary. Those exist because a lifecycle callback mutates the
//! tree while its handler lives inside the [`Document`] being mutated, and that
//! is true of an engine-authored handler exactly as it is of a script one.
//!
//! Restoring script-defined elements later is additive: it needs the
//! `undefined` state, an upgrade reaction, and a sweep in `define`. Neither the
//! trait nor the dispatch contract has to move.
//!
//! # Recorded limits
//!
//! - **`adoptedCallback` is unreachable, not unimplemented.** There is no `adoptNode` and no second
//!   document: [`Document::new`] builds a private style engine per document and every
//!   [`Node`](crate::Node) holds a backpointer to *its* arena set, so a node cannot change
//!   documents.
//! - **`connectedMoveCallback` has no move primitive.** `insert_before` detaches unconditionally,
//!   so every move is disconnect-then-connect — which is exactly the fallback the standard
//!   synthesizes for a definition that does not implement one.
//! - **Customized built-ins are absent.** The registry is keyed by local name alone, so there is no
//!   `extends`, no `is` value, and no second name to store; *look up a custom element definition*
//!   collapses to its autonomous arm.
//! - **Scoped registries are absent.** One registry per [`Document`], which is what *look up a
//!   custom element registry* returns for every node in a single-document engine.
//! - **`whenDefined`/`get`/`getName`/`upgrade(root)` are absent.** The first needs promises and an
//!   event loop this crate does not own; the rest are diagnostics, and with no upgrade at all
//!   `upgrade(root)` has nothing to do.
//! - **The `failed` state and the construction-stack machinery are absent.** All of it exists to
//!   police a JavaScript constructor that can throw, skip `super()`, or return a different object.
//!   [`CustomElement::constructed`] returns `()` on an already-allocated node; a panic is a caller
//!   bug that leaves the document unspecified, the same contract a panicking style flush already
//!   has. `Constructing` is kept, because it is not about failure — it is the window in which the
//!   constructor's own mutations enqueue nothing.
//! - **`:defined` matches every element, always.** With no `undefined` state there is nothing for
//!   it to distinguish, so the bit is seeded at element creation and never moves. The selector is
//!   still answered rather than ignored, so a stylesheet using it gets the right answer for this
//!   scope; `:not(:defined)` simply never matches, which is what makes the FOUC idiom a
//!   script-defined-elements feature.
//! - **A no-op `add_class`/`remove_class` reports nothing.** `DOMTokenList`'s update steps re-set
//!   the attribute even when the token set is unchanged, so a browser fires
//!   `attributeChangedCallback` with `old == new` for `classList.add` of a token that is already
//!   there. Both methods early-return here before any reaction is raised, which is the divergence —
//!   deliberate, and asserted by a test.
//! - **`disconnected_callback` receives a shared [`Document`], not a mutable one.** It is the one
//!   callback that runs with a free already committed: the removal that raised it drains while the
//!   subtree is unlinked but still allocated, then frees those slots the moment the drain returns.
//!   A mutation from inside it could re-attach the subtree being freed, link a child to a node
//!   about to die, or free the node its caller is still holding — three hazards a mutable handle
//!   would force every removal to detect and refuse at run time. `&Document` makes them
//!   unrepresentable instead, and costs a teardown handler nothing it needs: its own element, its
//!   subtree, its attributes, and its computed style are all still readable. The other three
//!   callbacks keep `&mut Document`; none of them runs with a free pending.
//! - **A callback may detach any node, but may not free one its caller is still holding.**
//!   [`Document::create_element`] and the constructor call pin the id they will still be naming
//!   once the drain returns, and [`Document::drop_element`]/[`Document::drop_subtree`] refuse to
//!   free a pinned node. Not a convenience limit: freeing retires the id permanently, so without
//!   the pin a callback that frees the node under construction hands its caller a dead id — an
//!   element the mutation is about to link into the tree and return, that already resolves to
//!   nothing.
//! - **A panicking callback leaves the document unspecified but not wedged.** The depth token
//!   balances its counter on the unwinding path and records that the frame was abandoned, so the
//!   next mutation discards the leftovers silently rather than blaming itself, while a scope in
//!   this crate that simply forgot to drain still trips the debug assertion.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use stylo::LocalName;

use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeId, NodeSlot};

const MAX_REACTION_DEPTH: usize = 64;

const MAX_REACTIONS_PER_SCOPE: usize = 1 << 20;

/// Lifecycle behavior shared by elements with one local name.
pub trait CustomElement<T>: Send + Sync {
    fn observed_attributes(&self) -> Vec<String> {
        Vec::new()
    }

    fn constructed(&self, document: &mut Document<T>, element: NodeId) {
        let _ = (document, element);
    }

    fn connected_callback(&self, document: &mut Document<T>, element: NodeId) {
        let _ = (document, element);
    }

    fn disconnected_callback(&self, document: &Document<T>, element: NodeId) {
        let _ = (document, element);
    }

    fn attribute_changed_callback(
        &self,
        document: &mut Document<T>,
        element: NodeId,
        name: &str,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        let _ = (document, element, name, old, new);
    }
}

/// Index into the document's definition list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DefinitionId(NonZeroU32);

impl DefinitionId {
    fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum CustomElementState {
    #[default]
    Uncustomized,
    Constructing,
    Custom,
}

enum Reaction {
    Constructed,
    Connected,
    Disconnected,
    AttributeChanged {
        name: LocalName,
        old: Option<String>,
        new: Option<String>,
    },
}

struct Definition<T> {
    observed: Box<[LocalName]>,
    handler: Arc<dyn CustomElement<T>>,
}

/// Document-owned definitions and reaction queues.
pub(crate) struct CustomElementRegistry<T> {
    definitions: Vec<Definition<T>>,
    by_name: FxHashMap<LocalName, DefinitionId>,
    element_queue: Vec<NodeId>,
    reactions: FxHashMap<NodeId, VecDeque<Reaction>>,
    pinned: SmallVec<[NodeId; 4]>,
    depth: Arc<AtomicUsize>,
    abandoned: Arc<AtomicBool>,
}

impl<T> Default for CustomElementRegistry<T> {
    fn default() -> Self {
        Self {
            definitions: Vec::new(),
            by_name: FxHashMap::default(),
            element_queue: Vec::new(),
            reactions: FxHashMap::default(),
            pinned: SmallVec::new(),
            depth: Arc::new(AtomicUsize::new(0)),
            abandoned: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<T> CustomElementRegistry<T> {
    fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    fn is_draining(&self) -> bool {
        self.depth.load(Ordering::Acquire) != 0
    }
}

impl<T> Document<T> {
    pub(crate) fn pin_node(&mut self, element: NodeId) {
        self.custom_elements.pinned.push(element);
    }

    pub(crate) fn unpin_node(&mut self, element: NodeId) {
        let popped = self.custom_elements.pinned.pop();
        debug_assert_eq!(
            popped,
            Some(element),
            "pins are released in the reverse order they were taken"
        );
    }

    pub(crate) fn assert_subtree_not_pinned(&self, root: NodeId) {
        if self.custom_elements.pinned.is_empty() {
            return;
        }
        // The walk stays in slot space: the tree's own links are slots, so
        // descending costs one load per child instead of a table hop, and the
        // id is only recovered where the pin set is consulted.
        let Some(root) = self.slot(root) else {
            return;
        };
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let node = self.arenas().at(current);
            self.assert_not_pinned(node.id());
            stack.extend_from_slice(node.child_slots());
            if let Some(shadow_root) = node.shadow_root_slot() {
                stack.push(shadow_root);
            }
        }
    }

    pub(crate) fn assert_not_pinned(&self, element: NodeId) {
        assert!(
            !self.custom_elements.pinned.contains(&element),
            "a custom element lifecycle callback destroyed a node the mutation that called it is \
             still holding: detaching it is allowed, freeing it is not, because freeing retires \
             the id and the mutation would return one that names nothing"
        );
    }
}

/// Balances the reaction-depth counter on unwind, so a panicking callback does
/// not leave every later drain reporting "recursed too deep".
struct ReactionDepthToken {
    depth: Arc<AtomicUsize>,
    abandoned: Arc<AtomicBool>,
}

impl ReactionDepthToken {
    fn enter(depth: &Arc<AtomicUsize>, abandoned: &Arc<AtomicBool>) -> Self {
        let token = Self {
            depth: Arc::clone(depth),
            abandoned: Arc::clone(abandoned),
        };
        let entered = token.depth.fetch_add(1, Ordering::AcqRel) + 1;
        assert!(
            entered <= MAX_REACTION_DEPTH,
            "custom element reactions nested more than {MAX_REACTION_DEPTH} deep: a lifecycle \
             callback re-triggers itself"
        );
        token
    }
}

impl Drop for ReactionDepthToken {
    fn drop(&mut self) {
        self.depth.fetch_sub(1, Ordering::AcqRel);
        if std::thread::panicking() {
            self.abandoned.store(true, Ordering::Release);
        }
    }
}

impl<T> Document<T> {
    /// Registers a tag's behavior before any matching element exists.
    pub fn define(&mut self, local_name: &str, element: Box<dyn CustomElement<T>>) {
        assert!(
            !local_name.is_empty(),
            "Document::define: the local name cannot be empty"
        );
        let name = LocalName::from(local_name);
        assert!(
            !self.custom_elements.by_name.contains_key(&name),
            "Document::define: `{local_name}` already has a definition"
        );

        let observed: Box<[LocalName]> = element
            .observed_attributes()
            .into_iter()
            .map(|attribute| LocalName::from(attribute.as_str()))
            .collect();
        let definition = DefinitionId(
            u32::try_from(self.custom_elements.definitions.len() + 1)
                .ok()
                .and_then(NonZeroU32::new)
                .expect("a document cannot hold u32::MAX custom element definitions"),
        );
        self.custom_elements.definitions.push(Definition {
            observed,
            handler: Arc::from(element),
        });
        self.custom_elements
            .by_name
            .insert(name.clone(), definition);

        let arenas = self.arenas();
        let existing = arenas.ids().find(|&id| {
            id != DOCUMENT_ELEMENT_NODE_ID && arenas.live(id).local_name.as_ref() == Some(&name)
        });
        assert!(
            existing.is_none(),
            "Document::define: `{local_name}` already has elements, and a definition never \
             reaches an element created before it — install every definition before building \
             the tree"
        );

        let root_matches = self
            .get(DOCUMENT_ELEMENT_NODE_ID)
            .is_some_and(|node| node.local_name.as_ref() == Some(&name));
        if root_matches {
            let base = self.begin_reactions();
            {
                let root = self.live_node_mut(DOCUMENT_ELEMENT_NODE_ID);
                root.custom_definition = Some(definition);
                root.custom_state = CustomElementState::Constructing;
                root.mark_custom_subtree_may_contain();
            }
            self.note_custom_subtree_inserted(DOCUMENT_ELEMENT_NODE_ID);
            self.enqueue_reaction(DOCUMENT_ELEMENT_NODE_ID, Reaction::Constructed);
            self.enqueue_reaction(DOCUMENT_ELEMENT_NODE_ID, Reaction::Connected);
            self.drain_reactions(base);
        }
    }

    pub(crate) fn begin_reactions(&mut self) -> usize {
        if !self.custom_elements.element_queue.is_empty() && !self.custom_elements.is_draining() {
            debug_assert!(
                self.custom_elements.abandoned.load(Ordering::Acquire),
                "custom element reactions outlived the mutation that raised them"
            );
            self.custom_elements.element_queue.clear();
            self.custom_elements.reactions.clear();
            self.custom_elements.pinned.clear();
        }
        if self.custom_elements.abandoned.load(Ordering::Relaxed) {
            self.custom_elements
                .abandoned
                .store(false, Ordering::Release);
        }
        self.custom_elements.element_queue.len()
    }

    pub(crate) fn drain_reactions(&mut self, base: usize) {
        if self.custom_elements.element_queue.len() == base {
            return;
        }
        let _depth =
            ReactionDepthToken::enter(&self.custom_elements.depth, &self.custom_elements.abandoned);
        let mut budget = MAX_REACTIONS_PER_SCOPE;
        let mut cursor = base;
        while cursor < self.custom_elements.element_queue.len() {
            let element = self.custom_elements.element_queue[cursor];
            cursor += 1;
            loop {
                let Some(reaction) = self
                    .custom_elements
                    .reactions
                    .get_mut(&element)
                    .and_then(VecDeque::pop_front)
                else {
                    break;
                };
                budget = budget.checked_sub(1).expect(
                    "custom element reactions did not reach a fixpoint: a lifecycle callback \
                     enqueues more work than it consumes",
                );
                self.invoke(element, reaction);
            }
            if self
                .custom_elements
                .reactions
                .get(&element)
                .is_some_and(VecDeque::is_empty)
            {
                self.custom_elements.reactions.remove(&element);
            }
        }
        self.custom_elements.element_queue.truncate(base);
    }

    fn enqueue_reaction(&mut self, element: NodeId, reaction: Reaction) {
        self.custom_elements
            .reactions
            .entry(element)
            .or_default()
            .push_back(reaction);
        self.custom_elements.element_queue.push(element);
    }

    fn invoke(&mut self, element: NodeId, reaction: Reaction) {
        match reaction {
            Reaction::Constructed => self.construct_element(element),
            Reaction::Connected => {
                let Some(handler) = self.dispatch_target(element) else {
                    return;
                };
                handler.connected_callback(self, element);
            }
            Reaction::Disconnected => {
                let Some(definition) = self.get(element).and_then(|node| node.custom_definition)
                else {
                    return;
                };
                let handler = &self.custom_elements.definitions[definition.index()].handler;
                handler.disconnected_callback(self, element);
            }
            Reaction::AttributeChanged { name, old, new } => {
                let Some(handler) = self.dispatch_target(element) else {
                    return;
                };
                handler.attribute_changed_callback(
                    self,
                    element,
                    name.0.as_ref(),
                    old.as_deref(),
                    new.as_deref(),
                );
            }
        }
    }

    fn dispatch_target(&self, element: NodeId) -> Option<Arc<dyn CustomElement<T>>> {
        let definition = self.get(element)?.custom_definition?;
        Some(Arc::clone(
            &self.custom_elements.definitions[definition.index()].handler,
        ))
    }

    fn construct_element(&mut self, element: NodeId) {
        let Some(definition) = self.get(element).and_then(|node| node.custom_definition) else {
            return;
        };
        debug_assert_eq!(
            self.live(element).custom_state,
            CustomElementState::Constructing,
            "construct_element runs once, on the element the creation path marked"
        );
        let handler = Arc::clone(&self.custom_elements.definitions[definition.index()].handler);
        self.pin_node(element);
        handler.constructed(self, element);
        self.unpin_node(element);

        let still_constructing = self
            .get(element)
            .is_some_and(|node| node.custom_state == CustomElementState::Constructing);
        if still_constructing {
            self.live_node_mut(element).custom_state = CustomElementState::Custom;
        }
    }

    pub(crate) fn note_custom_element_created(&mut self, element: NodeId, local_name: &LocalName) {
        let Some(definition) = self.custom_elements.by_name.get(local_name).copied() else {
            return;
        };
        {
            let node = self.live_node_mut(element);
            node.custom_definition = Some(definition);
            node.custom_state = CustomElementState::Constructing;
            node.mark_custom_subtree_may_contain();
        }
        self.enqueue_reaction(element, Reaction::Constructed);
    }

    pub(crate) fn note_custom_subtree_inserted(&mut self, root: NodeId) -> bool {
        if !self.live(root).custom_subtree_may_contain() {
            return false;
        }

        let mut current = self.live(root).parent_id();
        while let Some(node_id) = current {
            let parent = self.live(node_id).parent_id();
            if self
                .live_node_mut(node_id)
                .mark_custom_subtree_may_contain()
            {
                break;
            }
            current = parent;
        }
        true
    }

    #[must_use]
    pub(crate) fn custom_subtree_may_contain(&self, root: NodeId) -> bool {
        self.live(root).custom_subtree_may_contain()
    }

    pub(crate) fn note_custom_elements_inserted(&mut self, root: NodeId, connected: bool) {
        if self.custom_elements.is_empty() || !connected {
            return;
        }
        let mut inserted = SmallVec::<[NodeId; 8]>::new();
        self.collect_custom_elements_shadow_including_inclusive(root, &mut inserted);
        for element in inserted {
            self.enqueue_reaction(element, Reaction::Connected);
        }
    }

    pub(crate) fn note_custom_elements_removed(&mut self, root: NodeId, was_connected: bool) {
        if self.custom_elements.is_empty() || !was_connected {
            return;
        }
        let mut removed = SmallVec::<[NodeId; 8]>::new();
        self.collect_custom_elements_shadow_including_inclusive(root, &mut removed);
        for element in removed {
            self.enqueue_reaction(element, Reaction::Disconnected);
        }
    }

    pub(crate) fn observes_attribute(&self, element: NodeId, name: &LocalName) -> bool {
        if self.custom_elements.is_empty() {
            return false;
        }
        let Some(node) = self.get(element) else {
            return false;
        };
        if node.custom_state != CustomElementState::Custom {
            return false;
        }
        node.custom_definition.is_some_and(|definition| {
            self.custom_elements.definitions[definition.index()]
                .observed
                .contains(name)
        })
    }

    pub(crate) fn enqueue_attribute_changed(
        &mut self,
        element: NodeId,
        name: &LocalName,
        new: Option<&str>,
    ) {
        if !self.observes_attribute(element, name) {
            return;
        }
        let old = self.live(element).attr_local_name(name).map(str::to_owned);
        self.enqueue_reaction(
            element,
            Reaction::AttributeChanged {
                name: name.clone(),
                old,
                new: new.map(str::to_owned),
            },
        );
    }

    pub(crate) fn enqueue_attribute_changed_values(
        &mut self,
        element: NodeId,
        name: &LocalName,
        old: Option<String>,
        new: Option<String>,
    ) {
        if !self.observes_attribute(element, name) {
            return;
        }
        self.enqueue_reaction(
            element,
            Reaction::AttributeChanged {
                name: name.clone(),
                old,
                new,
            },
        );
    }

    pub(crate) fn forget_reactions(&mut self, element: NodeId) {
        debug_assert!(
            !self.custom_elements.pinned.contains(&element),
            "the pin preflight let a pinned node reach the destroy loop"
        );
        self.custom_elements.reactions.remove(&element);
    }

    pub(crate) fn custom_elements_are_draining(&self) -> bool {
        self.custom_elements.is_draining()
    }

    fn collect_custom_elements_shadow_including_inclusive(
        &self,
        root: NodeId,
        out: &mut SmallVec<[NodeId; 8]>,
    ) {
        if !self.live(root).custom_subtree_may_contain() {
            return;
        }
        let mut stack: SmallVec<[NodeSlot; 8]> = SmallVec::new();
        stack.push(self.live_slot(root));
        while let Some(current) = stack.pop() {
            let node = self.arenas().at(current);
            if !node.custom_subtree_may_contain() {
                continue;
            }
            if node.custom_state == CustomElementState::Custom {
                out.push(node.id());
            }
            stack.extend(node.child_slots().iter().rev().copied());
            if let Some(shadow_root) = node.shadow_root_slot() {
                stack.push(shadow_root);
            }
        }
    }
}
