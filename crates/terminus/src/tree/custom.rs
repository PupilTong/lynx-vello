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
//!   [`Node`](crate::tree::node::Node) holds a backpointer to *its* arena set, so a node cannot
//!   change documents.
//! - **`connectedMoveCallback` has no move primitive.** `insert_before` detaches unconditionally,
//!   so every move is disconnect-then-connect — which is exactly the fallback the standard
//!   synthesizes for a definition that does not implement one.
//! - **Reactions run in the order they were enqueued, not grouped per element** (user ruling,
//!   2026-10-22). The standard gives each element its own reaction queue and, at a drain, empties
//!   one element's queue before the next element's, a nested drain included. Here one queue holds
//!   every pending reaction with its element, and a scope runs its tail of it in order. The two
//!   differ where one scope queues reactions for two elements interleaved: moving a connected
//!   subtree holding `A` and `B` runs `A` disconnected, `B` disconnected, `A` connected, `B`
//!   connected, where the standard runs `A`'s two, then `B`'s. They also differ where a callback
//!   mutates an element whose reaction is still pending in the enclosing scope: the callback's own
//!   scope runs the new reaction first (an inserted visible overlay's shadow dialog hears `open`
//!   change before it hears it is connected), and if the callback freed that element, its pending
//!   reaction then runs nothing.
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
//! - **`slotchange` fires at this drain boundary, not at a microtask checkpoint.** The standard
//!   collects every slot whose assigned nodes changed in a *signal slots* set and fires
//!   `slotchange` at each one from the mutation-observer microtask (DOM §4.2.2.5, "notify mutation
//!   observers"). This crate owns no checkpoint, so the set is fired where the reactions drain
//!   instead: once the outermost mutation's reactions have run, in signal order, each slot once,
//!   through [`Document::dispatch_element_event`] — after the free, for a removal that frees. A
//!   slot signalled while the set is being fired is fired after it, as the next checkpoint would.
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

use std::cell::Cell;
use std::num::NonZeroU32;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use stylo::LocalName;

use crate::event::{ElementEvent, EventDetail};
use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeId, NodeSlot};

const MAX_REACTION_DEPTH: usize = 64;

pub(crate) const SLOT_CHANGE: &str = "slotchange";

const MAX_REACTIONS_PER_SCOPE: usize = 1 << 20;

pub trait CustomElement<T> {
    fn observed_attributes(&self) -> Vec<String> {
        Vec::new()
    }

    fn observed_events(&self) -> &'static [&'static str] {
        &[]
    }

    fn is_lynx_text(&self) -> bool {
        false
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

    fn freed(&self, element: NodeId) {
        let _ = element;
    }

    fn handle_event(&self, document: &mut Document<T>, element: NodeId, event: &mut ElementEvent) {
        let _ = (document, element, event);
    }

    fn invoke(
        &self,
        document: &mut Document<T>,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let _ = (document, element, call);
        MethodOutcome::NotFound
    }

    fn intersections_changed(
        &self,
        document: &mut Document<T>,
        element: NodeId,
        observer: crate::visual::intersection::IntersectionObserverId,
        entries: Vec<crate::visual::intersection::IntersectionObserverEntry>,
    ) {
        let _ = (document, element, observer, entries);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodCall<'a> {
    pub name: &'a str,
    pub params: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodOutcome {
    NotFound,
    Done,
    Data(String),
    Failed(MethodError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodError {
    InvalidParams,
    InvalidState,
    Operation,
}

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
    events: &'static [&'static str],
    lynx_text: bool,
    instances: usize,
    handler: Rc<dyn CustomElement<T>>,
}

pub(crate) struct CustomElementRegistry<T> {
    definitions: Vec<Definition<T>>,
    by_name: FxHashMap<LocalName, DefinitionId>,
    observed_events: FxHashSet<&'static str>,
    live_observers: FxHashMap<&'static str, usize>,
    observed_name_changes: Vec<(&'static str, bool)>,
    queue: Vec<(NodeId, Option<Reaction>)>,
    pinned: SmallVec<[NodeId; 4]>,
    depth: Rc<Cell<usize>>,
    abandoned: Rc<Cell<bool>>,
    signal_slots: Vec<NodeId>,
    firing_slots: Rc<Cell<bool>>,
}

impl<T> Default for CustomElementRegistry<T> {
    fn default() -> Self {
        Self {
            definitions: Vec::new(),
            by_name: FxHashMap::default(),
            observed_events: FxHashSet::default(),
            live_observers: FxHashMap::default(),
            observed_name_changes: Vec::new(),
            queue: Vec::new(),
            pinned: SmallVec::new(),
            depth: Rc::new(Cell::new(0)),
            abandoned: Rc::new(Cell::new(false)),
            signal_slots: Vec::new(),
            firing_slots: Rc::new(Cell::new(false)),
        }
    }
}

impl<T> CustomElementRegistry<T> {
    fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    fn is_draining(&self) -> bool {
        self.depth.get() != 0
    }

    fn instance_created(&mut self, definition: DefinitionId) {
        let entry = &mut self.definitions[definition.index()];
        entry.instances += 1;
        if entry.instances != 1 {
            return;
        }
        for &name in entry.events {
            let count = self.live_observers.entry(name).or_insert(0);
            *count += 1;
            if *count == 1 {
                Self::record_observed_name_change(&mut self.observed_name_changes, name, true);
            }
        }
    }

    fn instance_freed(&mut self, definition: DefinitionId) {
        let entry = &mut self.definitions[definition.index()];
        entry.instances = entry
            .instances
            .checked_sub(1)
            .expect("an element is freed once, and was counted when it was given its definition");
        if entry.instances != 0 {
            return;
        }
        for &name in entry.events {
            let count = self
                .live_observers
                .get_mut(name)
                .expect("a live definition counted its observed names");
            *count -= 1;
            if *count == 0 {
                self.live_observers.remove(name);
                Self::record_observed_name_change(&mut self.observed_name_changes, name, false);
            }
        }
    }

    fn record_observed_name_change(
        changes: &mut Vec<(&'static str, bool)>,
        name: &'static str,
        live: bool,
    ) {
        if let Some(pending) = changes.iter().position(|&(held, _)| held == name) {
            debug_assert_ne!(
                changes[pending].1, live,
                "a name's liveness changes alternate"
            );
            changes.remove(pending);
        } else {
            changes.push((name, live));
        }
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

struct ReactionDepthToken {
    depth: Rc<Cell<usize>>,
    abandoned: Rc<Cell<bool>>,
}

impl ReactionDepthToken {
    fn enter(depth: &Rc<Cell<usize>>, abandoned: &Rc<Cell<bool>>) -> Self {
        let token = Self {
            depth: Rc::clone(depth),
            abandoned: Rc::clone(abandoned),
        };
        let entered = token.depth.get() + 1;
        token.depth.set(entered);
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
        self.depth.set(self.depth.get() - 1);
        if std::thread::panicking() {
            self.abandoned.set(true);
        }
    }
}

struct FiringSlotsToken(Rc<Cell<bool>>);

impl Drop for FiringSlotsToken {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl<T> Document<T> {
    pub fn define(&mut self, local_name: &str, element: Box<dyn CustomElement<T>>) {
        self.define_shared(local_name, Rc::from(element));
    }

    pub fn define_shared(&mut self, local_name: &str, element: Rc<dyn CustomElement<T>>) {
        assert!(
            !local_name.is_empty(),
            "Document::define: the local name cannot be empty"
        );
        assert!(
            local_name != crate::svg::SVG_TAG,
            "Document::define: `svg` is the SVG namespace's `svg` element, which \
             `create_element` makes, and an SVG element takes no definition"
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
        let events = element.observed_events();
        let lynx_text = element.is_lynx_text();
        self.custom_elements
            .observed_events
            .extend(events.iter().copied());
        let definition = DefinitionId(
            u32::try_from(self.custom_elements.definitions.len() + 1)
                .ok()
                .and_then(NonZeroU32::new)
                .expect("a document cannot hold u32::MAX custom element definitions"),
        );
        self.custom_elements.definitions.push(Definition {
            observed,
            events,
            lynx_text,
            instances: 0,
            handler: element,
        });
        self.custom_elements
            .by_name
            .insert(name.clone(), definition);

        let arenas = self.arenas();
        let existing = arenas.ids().find(|&id| {
            let node = arenas.live(id);
            id != DOCUMENT_ELEMENT_NODE_ID
                && !node.in_svg_namespace()
                && node.local_name.as_ref() == Some(&name)
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
                if lynx_text {
                    root.mark_lynx_text();
                }
                root.set_custom_state(CustomElementState::Constructing);
                root.mark_custom_subtree_may_contain();
            }
            self.custom_elements.instance_created(definition);
            self.note_custom_subtree_inserted(DOCUMENT_ELEMENT_NODE_ID);
            self.enqueue_reaction(DOCUMENT_ELEMENT_NODE_ID, Reaction::Constructed);
            self.enqueue_reaction(DOCUMENT_ELEMENT_NODE_ID, Reaction::Connected);
            self.drain_reactions(base);
        }
    }

    pub(crate) fn begin_reactions(&mut self) -> usize {
        if !self.custom_elements.queue.is_empty() && !self.custom_elements.is_draining() {
            debug_assert!(
                self.custom_elements.abandoned.get(),
                "custom element reactions outlived the mutation that raised them"
            );
            self.custom_elements.queue.clear();
            self.custom_elements.pinned.clear();
        }
        self.custom_elements.abandoned.set(false);
        self.custom_elements.queue.len()
    }

    pub(crate) fn drain_reactions(&mut self, base: usize) {
        self.run_reactions(base);
        self.fire_slot_changes();
    }

    pub(crate) fn run_reactions(&mut self, base: usize) {
        if self.custom_elements.queue.len() == base {
            return;
        }
        let _depth =
            ReactionDepthToken::enter(&self.custom_elements.depth, &self.custom_elements.abandoned);
        let mut budget = MAX_REACTIONS_PER_SCOPE;
        let mut cursor = base;
        while cursor < self.custom_elements.queue.len() {
            let (element, reaction) = &mut self.custom_elements.queue[cursor];
            let (element, reaction) = (*element, reaction.take());
            cursor += 1;
            let Some(reaction) = reaction else {
                continue;
            };
            budget = budget.checked_sub(1).expect(
                "custom element reactions did not reach a fixpoint: a lifecycle callback \
                 enqueues more work than it consumes",
            );
            self.invoke(element, reaction);
        }
        self.custom_elements.queue.truncate(base);
    }

    pub(crate) fn signal_slot_change(&mut self, slot: NodeId) {
        if !self.observes_event(SLOT_CHANGE) {
            return;
        }
        let signalled = &mut self.custom_elements.signal_slots;
        if !signalled.contains(&slot) {
            signalled.push(slot);
        }
    }

    pub(crate) fn fire_slot_changes(&mut self) {
        if self.custom_elements.signal_slots.is_empty()
            || self.custom_elements.is_draining()
            || self.custom_elements.firing_slots.get()
        {
            return;
        }
        self.custom_elements.firing_slots.set(true);
        let _firing = FiringSlotsToken(Rc::clone(&self.custom_elements.firing_slots));
        let mut budget = MAX_REACTIONS_PER_SCOPE;
        while !self.custom_elements.signal_slots.is_empty() {
            let signalled = std::mem::take(&mut self.custom_elements.signal_slots);
            for slot in signalled {
                budget = budget.checked_sub(1).expect(
                    "slotchange did not reach a fixpoint: a handler keeps changing slot \
                     assignments",
                );
                self.dispatch_element_event(
                    slot,
                    SLOT_CHANGE,
                    &EventDetail::None,
                    true,
                    false,
                    0.0,
                );
            }
        }
    }

    fn enqueue_reaction(&mut self, element: NodeId, reaction: Reaction) {
        self.custom_elements.queue.push((element, Some(reaction)));
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

    #[must_use]
    pub fn observes_event(&self, type_name: &str) -> bool {
        self.custom_elements.observed_events.contains(type_name)
    }

    pub fn observed_event_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.custom_elements.observed_events.iter().copied()
    }

    pub fn take_observed_name_changes(&mut self) -> Vec<(&'static str, bool)> {
        std::mem::take(&mut self.custom_elements.observed_name_changes)
    }

    pub(crate) fn custom_elements_observed_event(&self, type_name: &str) -> Option<&&'static str> {
        self.custom_elements.observed_events.get(type_name)
    }

    pub(crate) fn custom_element_handler(
        &self,
        element: NodeId,
    ) -> Option<Rc<dyn CustomElement<T>>> {
        let node = self.get(element)?;
        if node.custom_state() != CustomElementState::Custom {
            return None;
        }
        self.dispatch_target(element)
    }

    pub(crate) fn custom_element_hearing(
        &self,
        element: NodeId,
        type_name: &str,
    ) -> Option<Rc<dyn CustomElement<T>>> {
        let node = self.get(element)?;
        if node.custom_state() != CustomElementState::Custom {
            return None;
        }
        let definition = &self.custom_elements.definitions[node.custom_definition?.index()];
        definition
            .events
            .contains(&type_name)
            .then(|| Rc::clone(&definition.handler))
    }

    pub fn invoke_element_method(
        &mut self,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let Some(handler) = self.custom_element_handler(element) else {
            return MethodOutcome::NotFound;
        };
        let base = self.begin_reactions();
        let outcome = handler.invoke(self, element, call);
        self.drain_reactions(base);
        outcome
    }

    fn dispatch_target(&self, element: NodeId) -> Option<Rc<dyn CustomElement<T>>> {
        let definition = self.get(element)?.custom_definition?;
        Some(Rc::clone(
            &self.custom_elements.definitions[definition.index()].handler,
        ))
    }

    fn construct_element(&mut self, element: NodeId) {
        let Some(definition) = self.get(element).and_then(|node| node.custom_definition) else {
            return;
        };
        debug_assert_eq!(
            self.live(element).custom_state(),
            CustomElementState::Constructing,
            "construct_element runs once, on the element the creation path marked"
        );
        let handler = Rc::clone(&self.custom_elements.definitions[definition.index()].handler);
        self.pin_node(element);
        handler.constructed(self, element);
        self.unpin_node(element);

        let still_constructing = self
            .get(element)
            .is_some_and(|node| node.custom_state() == CustomElementState::Constructing);
        if still_constructing {
            self.live_node_mut(element)
                .set_custom_state(CustomElementState::Custom);
        }
    }

    pub(crate) fn note_custom_element_created(&mut self, element: NodeId, local_name: &LocalName) {
        debug_assert!(
            !self.live(element).in_svg_namespace(),
            "an SVG element takes no custom element definition"
        );
        let Some(definition) = self.custom_elements.by_name.get(local_name).copied() else {
            return;
        };
        let lynx_text = self.custom_elements.definitions[definition.index()].lynx_text;
        {
            let node = self.live_node_mut(element);
            node.custom_definition = Some(definition);
            if lynx_text {
                node.mark_lynx_text();
            }
            node.set_custom_state(CustomElementState::Constructing);
            node.mark_custom_subtree_may_contain();
        }
        self.custom_elements.instance_created(definition);
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
        if node.custom_state() != CustomElementState::Custom {
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
        let old = self
            .live(element)
            .attr_local_name(name)
            .map(std::borrow::Cow::into_owned);
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

    pub(crate) fn note_custom_element_freed(
        &mut self,
        element: NodeId,
        definition: Option<DefinitionId>,
    ) {
        if let Some(definition) = definition {
            self.custom_elements.instance_freed(definition);
            self.custom_elements.definitions[definition.index()]
                .handler
                .freed(element);
        }
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
            if node.custom_state() == CustomElementState::Custom {
                out.push(node.id());
            }
            stack.extend(node.child_slots().iter().rev().copied());
            if let Some(shadow_root) = node.shadow_root_slot() {
                stack.push(shadow_root);
            }
        }
    }
}
