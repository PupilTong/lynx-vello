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
//!   event loop this crate does not own; the rest are diagnostics, and eager upgrade at creation
//!   and insertion leaves `upgrade(root)` with no behavior of its own.
//! - **The `failed` state and the construction-stack machinery are absent.** All of it exists to
//!   police a JavaScript constructor that can throw, skip `super()`, or return a different object.
//!   [`CustomElement::constructed`] returns `()` on an already-allocated node; a panic is a caller
//!   bug that leaves the document unspecified, the same contract a panicking style flush already
//!   has. `Precustomized` is kept, because it is not about failure — it is the window in which the
//!   constructor's own mutations enqueue nothing.
//! - **`:defined` stays set for the `Precustomized` window** even though the standard says that
//!   state is not defined. Selector matching cannot run inside a drain
//!   ([`Document::begin_flush_phase`] asserts it), so the transient is unobservable, and skipping
//!   it saves a snapshot plus an ancestor-spine walk per upgrade.
//! - **A handler can forge `:defined`** through the public `add_element_state`. Unenforced, and
//!   recorded rather than policed.
//! - **A callback may detach any node, but may not free one its caller is still holding.**
//!   [`Document::create_element`], [`Document::remove_subtree`], and the constructor call all pin
//!   the id they will still be naming once the drain returns, and freeing a pinned node panics. Not
//!   a convenience limit: a [`NodeId`] is a slab key the arena recycles on free, so without the pin
//!   a callback that frees the node and creates a replacement hands its caller a live id naming a
//!   *different* node — and every liveness check passes while it happens.
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
use stylo_dom::ElementState;

use crate::tree::document::{DOCUMENT_NODE_ID, Document, NodeId};

/// Nesting limit for reaction scopes. A handler whose callback re-triggers
/// itself through nested mutations would otherwise overflow the native stack,
/// and this crate imposes no tree-depth cap of its own.
const MAX_REACTION_DEPTH: usize = 64;

/// Per-scope fixpoint budget. A handler that enqueues more than it consumes at
/// one depth would otherwise spin forever while an embedder holds the tree
/// lock, and a hung frame is strictly worse than a crash in a crash-on-misuse
/// core.
const MAX_REACTIONS_PER_SCOPE: usize = 1 << 20;

/// The behavior every element with one local name has.
///
/// One handler serves **every** element with that name, because
/// [`Document::define`] registers by name. That is why the callbacks take
/// `&self` and identify their element by [`NodeId`]: per-element state belongs
/// in the embedder's payload `T` or in a map the handler owns, and per-tag
/// state that must change needs the handler's own interior mutability.
///
/// `&self` is also forced by re-entrancy. A callback on tag `x-row` that
/// creates another `x-row` re-enters this very handler while the outer call is
/// still on the stack — the ordinary list-component shape, not misuse — so the
/// dispatcher hands out a cloned [`Arc`] rather than lending the handler out of
/// the registry, and there is no window in which a definition is missing.
///
/// `Send + Sync` is not decoration: an embedder that moves the tree to another
/// thread needs `Document<T>: Send`, and `Arc<X>: Send` requires
/// `X: Send + Sync`. A thread-affine script handle must therefore be wrapped by
/// the binding layer before it can become a definition.
pub trait CustomElement<T>: Send + Sync {
    /// The attribute local names whose changes reach
    /// [`Self::attribute_changed_callback`].
    ///
    /// Read **exactly once**, by [`Document::define`], and interned into the
    /// definition there. That is required behavior rather than a cache: the
    /// standard converts `observedAttributes` once at definition time, so a
    /// handler that later changes its mind is deliberately not observed.
    /// Matching is against the attribute's local name alone.
    ///
    /// Returning an empty list is the standard's "no `attributeChangedCallback`"
    /// gate, and it is the fast path: an unobserved attribute mutation never
    /// reads an old value, never allocates, and never enqueues.
    ///
    /// The return type is owned because a definition built from a script class
    /// cannot hand back borrowed `&'static str`s. It costs one allocation per
    /// [`Document::define`], and definitions are permanent.
    fn observed_attributes(&self) -> Vec<String> {
        Vec::new()
    }

    /// The upgrade reaction — the standard's custom element constructor.
    ///
    /// Runs once per element, and runs **before** the replayed
    /// [`Self::attribute_changed_callback`]s for the attributes the element
    /// already carried and before [`Self::connected_callback`], because the
    /// upgrade algorithm enqueues those behind the upgrade reaction rather than
    /// calling them.
    ///
    /// The element's state is `precustomized` for the duration, which is not
    /// `custom`: every mutation performed here fails the "is custom" gate that
    /// attribute changes test, so a constructor that normalizes its own
    /// attributes is not reported back to itself.
    ///
    /// This is where a component attaches its shadow root. Check
    /// [`Document::shadow_root`] first — `attach_shadow` is crash-on-misuse
    /// when a root already exists, and one handler is shared by every element
    /// with its name.
    fn constructed(&self, document: &mut Document<T>, element: NodeId) {
        let _ = (document, element);
    }

    /// The element was inserted into a connected tree, or its definition
    /// landed while it was already connected.
    ///
    /// Not a reliable "I am connected" signal, and not once per element: the
    /// standard permits delivery to an element that an earlier reaction in the
    /// same drain has already disconnected, and it fires again on every
    /// re-insertion. Do not re-check connectedness and skip — that is a
    /// conformance bug, and it is why the standard tells authors to avoid
    /// mutating the tree from reactions.
    fn connected_callback(&self, document: &mut Document<T>, element: NodeId) {
        let _ = (document, element);
    }

    /// The element was removed from a connected tree.
    ///
    /// Gated on the **old parent's** connectedness sampled at unlink time, not
    /// on the element's own, which is already false by delivery. During
    /// [`Document::remove_subtree`] this runs while the subtree is unlinked but
    /// still allocated — the only window in which its nodes can be read at all,
    /// because the arena frees them immediately afterwards and recycles their
    /// ids.
    fn disconnected_callback(&self, document: &mut Document<T>, element: NodeId) {
        let _ = (document, element);
    }

    /// An observed attribute changed, was added, or was removed.
    ///
    /// `old` is `None` for an addition, `new` is `None` for a removal, and both
    /// are the values captured when the reaction was enqueued. `class`, `id`,
    /// and `style` are ordinary attributes here and fire like any other. An
    /// upgrade replays every observed attribute the element already carries, in
    /// the element's own attribute-list order, each with `old = None`.
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

/// Index of a definition in the document's append-only definition list.
///
/// `NonZeroU32` rather than `u32` so `Option<DefinitionId>` is four bytes and
/// lands in the primary node's existing tail padding — the stride assertion in
/// [`crate::tree::node`] holds it to that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DefinitionId(NonZeroU32);

impl DefinitionId {
    fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// The standard's custom element state, minus `failed`.
///
/// Four values rather than a boolean: `Precustomized` is what suppresses the
/// reactions a constructor's own mutations would otherwise raise, and it is
/// what makes the upgrade guard reject a re-entrant upgrade of an element whose
/// constructor is still running.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum CustomElementState {
    /// Every ordinary tag, including unknown hyphen-free ones. **Matches
    /// `:defined`** — this is the majority state, which is why
    /// [`Node::new_element`](crate::Node) seeds [`ElementState::DEFINED`].
    #[default]
    Uncustomized,
    /// A custom-element-name candidate with no definition, or one whose upgrade
    /// reaction has been enqueued and not yet run. Does not match `:defined`.
    Undefined,
    /// The constructor is on the stack.
    Precustomized,
    /// Upgraded. Matches `:defined`, and is the exact gate every lifecycle
    /// enqueue site tests.
    Custom,
}

/// One item of an element's custom element reaction queue. The element is the
/// map key, so it is not repeated here.
enum Reaction {
    /// Carries its definition because an element being upgraded is not yet
    /// [`CustomElementState::Custom`] and so cannot supply one.
    Upgrade(DefinitionId),
    Connected,
    Disconnected,
    AttributeChanged {
        name: LocalName,
        old: Option<String>,
        new: Option<String>,
    },
}

struct Definition<T> {
    /// Snapshotted and interned at definition time, so the per-mutation filter
    /// is atom comparison rather than string comparison.
    observed: Box<[LocalName]>,
    /// `Arc`, not the caller's `Box`: dispatch needs the handler and
    /// `&mut Document<T>` at the same time, and a reaction on this same tag
    /// raised from inside a callback needs a second handle.
    handler: Arc<dyn CustomElement<T>>,
}

/// Document-owned definitions plus the standard's two-level reaction storage.
///
/// `element_queue` is the stack of element queues, flattened: a scope records
/// `len()` on entry and truncates back to it on exit, so a nested scope costs
/// no allocation and cannot consume an outer scope's entries. `reactions` is
/// the per-element reaction queue, shared across scopes exactly as the
/// standard's is.
pub(crate) struct CustomElementRegistry<T> {
    definitions: Vec<Definition<T>>,
    by_name: FxHashMap<LocalName, DefinitionId>,
    element_queue: Vec<NodeId>,
    reactions: FxHashMap<NodeId, VecDeque<Reaction>>,
    /// Nodes an in-flight operation is still holding **by id** across a
    /// reaction drain.
    ///
    /// A `NodeId` is a slab key the arena recycles the moment it is freed, so
    /// it is an occupancy token and never an identity one. Without this, a
    /// callback that both destroys the node its caller is holding and creates
    /// a replacement hands that caller back a live id naming a *different*
    /// node — and every liveness assert around a drain passes while doing it.
    /// A callback may still detach a pinned node; it may not have the arena
    /// free it.
    pinned: SmallVec<[NodeId; 4]>,
    /// Shared by `Arc` so the depth token can restore it on the unwinding path
    /// without holding a borrow of the document — the same reason
    /// `FlushPhaseToken` shares the document node's flag.
    depth: Arc<AtomicUsize>,
    /// Set when a drain unwound out of a panicking callback, which is the one
    /// documented route to leftover queue entries. It is what lets the next
    /// mutation tell that case — recover silently, the document is already
    /// unspecified — from a scope in this crate that forgot to drain, which is
    /// an engine bug and must be reported.
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
    /// Zero definitions is the gate every insertion, removal, and attribute
    /// hook checks first, so a document that defines nothing pays one
    /// predictable branch — the deal [`Document::has_shadow_roots`] takes. The
    /// *creation* hook is deliberately not gated on it: `:defined` polarity
    /// must be right in a document with no definitions at all.
    fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    fn is_draining(&self) -> bool {
        self.depth.load(Ordering::Acquire) != 0
    }
}

impl<T> Document<T> {
    /// Holds `element`'s id against destruction for the duration of an
    /// operation that will still be naming it after a drain returns.
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

    fn assert_not_pinned(&self, element: NodeId) {
        assert!(
            !self.custom_elements.pinned.contains(&element),
            "a custom element lifecycle callback destroyed a node the mutation that called it is \
             still holding: detaching it is allowed, freeing it is not, because the arena would \
             recycle its id"
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
        // Constructed *before* the increment is checked, so the `Drop` below
        // balances it even when the assert fires. Incrementing first and
        // asserting second would leak the count on the panicking path, and
        // `is_draining()` would then answer `true` forever — wedging every
        // later style flush on a document a `catch_unwind` harness still holds.
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

/// The standard's *valid custom element name*, as the living standard reads it.
///
/// Used only to decide whether a **definition-less** element starts
/// [`CustomElementState::Undefined`] rather than
/// [`CustomElementState::Uncustomized`] — that is, whether `:not(:defined)` can
/// ever match it. [`Document::define`] deliberately does not apply it, so an
/// embedder may register a handler for a hyphen-free tag it owns.
///
/// This is the current five-clause predicate, not the retired
/// `PotentialCustomElementName` production; that grammar rejects names the
/// standard now accepts. Because the first code point must be an ASCII lower
/// alpha, the *valid element local name* substrate collapses to a blacklist of
/// exactly eight code points.
///
/// The eight reserved names are the standard's own, not tag vocabulary this
/// crate constructs — the same ground on which the shadow module owns `<slot>`.
pub(crate) fn is_valid_custom_element_name(name: &str) -> bool {
    const RESERVED: [&str; 8] = [
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    let mut chars = name.chars();
    if !chars.next().is_some_and(|first| first.is_ascii_lowercase()) {
        return false;
    }
    let mut hyphen = false;
    for character in chars {
        match character {
            '-' => hyphen = true,
            'A'..='Z' | '\0' | '\t' | '\n' | '\u{000C}' | '\r' | ' ' | '/' | '>' => return false,
            _ => {}
        }
    }
    hyphen && !RESERVED.contains(&name)
}

impl<T> Document<T> {
    /// Registers `element` as the behavior of every element whose tag is
    /// `local_name`, then upgrades the ones already in the tree.
    ///
    /// One handler per name, so a definition is per-tag rather than per
    /// instance; the callbacks identify their element by [`NodeId`]. The name
    /// is injected rather than known, the same way [`Document::new`] takes the
    /// document element's tag — this crate still owns no tag vocabulary.
    ///
    /// Panics on an empty name, and on a name that already has a definition:
    /// the standard throws `NotSupportedError` for a duplicate, and this core
    /// is crash-on-misuse.
    ///
    /// Every element already in the tree with that name is upgraded before this
    /// returns, in shadow-including tree order, and every observed attribute it
    /// already carries is replayed to it.
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

        // Read once, here, and never again: the standard converts
        // `observedAttributes` at definition time.
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

        // The candidate list is frozen before any reaction runs, so a
        // constructor that builds more elements of the same name does not
        // extend the sweep it is running inside — those are upgraded by their
        // own creation path instead, and the upgrade guard makes the two paths
        // converge on exactly one upgrade each.
        let base = self.begin_reactions();
        let mut candidates = SmallVec::<[NodeId; 8]>::new();
        self.collect_shadow_including_inclusive(DOCUMENT_NODE_ID, &mut candidates);
        for candidate in candidates {
            let matches = self
                .get(candidate)
                .is_some_and(|node| node.local_name.as_ref() == Some(&name));
            if !matches {
                continue;
            }
            let upgradable = self.live(candidate).custom_state;
            if matches!(
                upgradable,
                CustomElementState::Uncustomized | CustomElementState::Undefined
            ) {
                self.enqueue_reaction(candidate, Reaction::Upgrade(definition));
            }
        }
        self.drain_reactions(base);
    }

    /// Opens a reaction scope. The returned watermark is the element-queue
    /// length the matching [`Self::drain_reactions`] drains down to.
    pub(crate) fn begin_reactions(&mut self) -> usize {
        // A drain that unwound out of a panicking callback left its frame's
        // entries behind. The document is unspecified from that point (the
        // let-it-crash contract), but an unrelated later mutation must not
        // replay an abandoned frame's reactions against a tree that has moved.
        if !self.custom_elements.element_queue.is_empty() && !self.custom_elements.is_draining() {
            debug_assert!(
                self.custom_elements.abandoned.load(Ordering::Acquire),
                "custom element reactions outlived the mutation that raised them"
            );
            self.custom_elements.element_queue.clear();
            self.custom_elements.reactions.clear();
            self.custom_elements.pinned.clear();
        }
        // Cleared unconditionally: the leftovers are gone either way, so a
        // later, unrelated panic must not be excused by this one.
        self.custom_elements
            .abandoned
            .store(false, Ordering::Release);
        self.custom_elements.element_queue.len()
    }

    /// The standard's *invoke custom element reactions*, for one scope.
    pub(crate) fn drain_reactions(&mut self, base: usize) {
        if self.custom_elements.element_queue.len() == base {
            return;
        }
        let _depth = ReactionDepthToken::enter(
            &Arc::clone(&self.custom_elements.depth),
            &Arc::clone(&self.custom_elements.abandoned),
        );
        let mut budget = MAX_REACTIONS_PER_SCOPE;
        let mut cursor = base;
        // The outer loop re-reads `len()`: a callback that enqueues onto an
        // element past the cursor is picked up here, not deferred.
        while cursor < self.custom_elements.element_queue.len() {
            let element = self.custom_elements.element_queue[cursor];
            cursor += 1;
            // A live drain, not iteration over a snapshot. This is the
            // mechanism that makes upgrade work: the upgrade reaction runs
            // first, and its own steps append the attribute replay and the
            // connected reaction to this same element's queue, which this loop
            // then consumes in the same pass.
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
        // Duplicate queue entries are legal and harmless: the first dequeue
        // drains the element's queue to empty and a later one finds nothing.
        self.custom_elements.element_queue.push(element);
    }

    /// The one place a handler is called.
    ///
    /// `Arc::clone` ends the borrow of `self` on its own line, so the handler is
    /// an owned stack value and `&mut *self` is free to be the callback's
    /// document. Re-entering the same definition is a second refcount bump
    /// rather than a hole in the registry, and re-indexing by [`DefinitionId`]
    /// after every call is what makes a nested [`Self::define`] harmless.
    fn invoke(&mut self, element: NodeId, reaction: Reaction) {
        match reaction {
            Reaction::Upgrade(definition) => self.upgrade_element(element, definition),
            Reaction::Connected => {
                let Some(handler) = self.dispatch_target(element) else {
                    return;
                };
                handler.connected_callback(self, element);
            }
            Reaction::Disconnected => {
                let Some(handler) = self.dispatch_target(element) else {
                    return;
                };
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

    /// Re-resolves a queued reaction's target rather than trusting it. A
    /// reaction can outlive its element when an earlier callback in the same
    /// drain removes it; ids freed by [`Self::remove_subtree`] have their whole
    /// reaction queue dropped there, which is what keeps a recycled id from
    /// answering here.
    fn dispatch_target(&self, element: NodeId) -> Option<Arc<dyn CustomElement<T>>> {
        let definition = self.get(element)?.custom_definition?;
        Some(Arc::clone(
            &self.custom_elements.definitions[definition.index()].handler,
        ))
    }

    /// The standard's *upgrade an element*, minus the steps that police a
    /// JavaScript constructor.
    fn upgrade_element(&mut self, element: NodeId, definition: DefinitionId) {
        // The re-entrancy guard. A second upgrade reaction for the same element
        // is legal — `define` plus a re-insertion produces one — and this is
        // what makes it a no-op.
        let Some(node) = self.get(element) else {
            return;
        };
        if !matches!(
            node.custom_state,
            CustomElementState::Uncustomized | CustomElementState::Undefined
        ) {
            return;
        }
        {
            let node = self.live_node_mut(element);
            node.custom_definition = Some(definition);
            node.custom_state = CustomElementState::Precustomized;
        }

        // Every attribute in the element's ATTRIBUTE-LIST order, filtered by
        // the observed set — not the observed list's order.
        let replay: Vec<(LocalName, String)> = {
            let observed = &self.custom_elements.definitions[definition.index()].observed;
            self.live(element)
                .attrs
                .iter()
                .filter(|(name, _)| observed.contains(name))
                .cloned()
                .collect()
        };
        for (name, value) in replay {
            self.enqueue_reaction(
                element,
                Reaction::AttributeChanged {
                    name,
                    old: None,
                    new: Some(value),
                },
            );
        }
        // Connectedness is sampled before the constructor runs.
        if self.is_connected(element) {
            self.enqueue_reaction(element, Reaction::Connected);
        }

        // Both reactions above are now behind this call in the element's own
        // queue, so they run after it returns — and, exactly as in the
        // standard, they do not run inside it: a nested reaction scope started
        // by the constructor has a watermark above this element's queue entry,
        // so it cannot dequeue it.
        let handler = Arc::clone(&self.custom_elements.definitions[definition.index()].handler);
        self.pin_node(element);
        handler.constructed(self, element);
        self.unpin_node(element);

        // Keyed on the state this call wrote, not on `contains_node`: slab
        // occupancy would also answer `true` for a recycled id, and this would
        // then stamp `Custom` onto an unrelated node — which `define` would
        // afterwards refuse to upgrade, forever.
        let still_constructing = self
            .get(element)
            .is_some_and(|node| node.custom_state == CustomElementState::Precustomized);
        if still_constructing {
            self.live_node_mut(element).custom_state = CustomElementState::Custom;
            self.set_defined(element, true);
        }
    }

    /// The standard's *try to upgrade an element*. Never upgrades inline.
    fn try_upgrade(&mut self, element: NodeId) {
        let definition = self
            .live(element)
            .local_name
            .as_ref()
            .and_then(|name| self.custom_elements.by_name.get(name).copied());
        if let Some(definition) = definition {
            self.enqueue_reaction(element, Reaction::Upgrade(definition));
        }
    }

    /// Assigns a freshly created element's custom element state and `:defined`
    /// polarity, and enqueues its upgrade when its tag is already defined.
    ///
    /// Not gated on an empty registry: an element whose tag is a custom element
    /// name is `undefined` — and therefore matches `:not(:defined)` — whether
    /// or not this document has ever defined anything.
    pub(crate) fn note_custom_element_created(&mut self, element: NodeId, local_name: &LocalName) {
        let definition = self.custom_elements.by_name.get(local_name).copied();
        if definition.is_none() && !is_valid_custom_element_name(local_name.0.as_ref()) {
            // `Uncustomized`, which is the `Node::new_element` default, and
            // which is defined.
            return;
        }
        {
            let node = self.live_node_mut(element);
            node.custom_state = CustomElementState::Undefined;
            // Written directly rather than through `remove_element_state`: the
            // node was allocated one statement ago, so it has no snapshot to
            // take and no ancestors to dirty.
            node.element_state.remove(ElementState::DEFINED);
        }
        if definition.is_some() {
            self.try_upgrade(element);
        }
    }

    /// The insertion half of the lifecycle: every element in the inserted
    /// subtree either upgrades or, if it is already custom, connects.
    pub(crate) fn note_custom_elements_inserted(&mut self, root: NodeId, connected: bool) {
        if self.custom_elements.is_empty() {
            return;
        }
        let mut inserted = SmallVec::<[NodeId; 8]>::new();
        self.collect_shadow_including_inclusive(root, &mut inserted);
        for element in inserted {
            match self.live(element).custom_state {
                // Already upgraded: a plain connected reaction.
                CustomElementState::Custom => {
                    if connected {
                        self.enqueue_reaction(element, Reaction::Connected);
                    }
                }
                // Not yet upgraded: the upgrade enqueues its own connected
                // reaction, so this must never enqueue both.
                CustomElementState::Uncustomized | CustomElementState::Undefined => {
                    if connected {
                        self.try_upgrade(element);
                    }
                }
                CustomElementState::Precustomized => {}
            }
        }
    }

    /// The removal half. `was_connected` is the **old parent's** connectedness
    /// sampled before the unlink, because the removed node's own is already
    /// false by the time this runs.
    pub(crate) fn note_custom_elements_removed(&mut self, root: NodeId, was_connected: bool) {
        if self.custom_elements.is_empty() || !was_connected {
            return;
        }
        let mut removed = SmallVec::<[NodeId; 8]>::new();
        self.collect_shadow_including_inclusive(root, &mut removed);
        for element in removed {
            if self.live(element).custom_state == CustomElementState::Custom {
                self.enqueue_reaction(element, Reaction::Disconnected);
            }
        }
    }

    /// Whether a mutation of `name` on `element` could reach a handler at all.
    ///
    /// The gate every attribute setter takes first. Only a `custom` element
    /// reports — `Precustomized` is deliberately excluded, which is what stops
    /// a constructor's own attribute writes being reported back to it — and
    /// only an observed name, so an unobserved mutation never reads an old
    /// value and never allocates.
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

    /// The attribute half. Called **before** the write, so the old value is
    /// still readable on the node; `new` is `None` for a removal.
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

    /// The same reaction with both values supplied, for the setters whose new
    /// value is only knowable after the write (`add_class`, `remove_class`,
    /// which re-serialize the whole class list).
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

    /// Drops every queued reaction for a node the arena is about to free, so a
    /// recycled id cannot inherit them, and refuses to free one an in-flight
    /// operation is still holding.
    pub(crate) fn forget_reactions(&mut self, element: NodeId) {
        self.assert_not_pinned(element);
        self.custom_elements.reactions.remove(&element);
    }

    pub(crate) fn custom_elements_are_draining(&self) -> bool {
        self.custom_elements.is_draining()
    }

    /// Flips `:defined` through the ordinary element-state funnel, which
    /// snapshots and dirties the ancestor spine before writing — the order
    /// Stylo's state-change invalidation requires.
    fn set_defined(&mut self, element: NodeId, defined: bool) {
        let current = self
            .get(element)
            .is_some_and(|node| node.element_state().contains(ElementState::DEFINED));
        if current == defined {
            return;
        }
        if defined {
            self.add_element_state(element, ElementState::DEFINED);
        } else {
            self.remove_element_state(element, ElementState::DEFINED);
        }
    }

    /// Shadow-including preorder, depth-first: a host, then its whole shadow
    /// tree, then its light children. Pushing the shadow root last makes it pop
    /// first, which is what puts it immediately after its host.
    ///
    /// An explicit stack — this crate imposes no tree-depth cap, so recursion
    /// is not an option.
    fn collect_shadow_including_inclusive(&self, root: NodeId, out: &mut SmallVec<[NodeId; 8]>) {
        let mut stack: SmallVec<[NodeId; 8]> = SmallVec::new();
        stack.push(root);
        while let Some(current) = stack.pop() {
            let Some(node) = self.get(current) else {
                continue;
            };
            if node.is_element() {
                out.push(current);
            }
            stack.extend(node.child_ids().iter().rev().copied());
            if let Some(shadow_root) = node.shadow_root_id() {
                stack.push(shadow_root);
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::is_valid_custom_element_name as valid;

    #[test]
    fn a_custom_element_name_needs_a_hyphen_and_an_ascii_lower_first_code_point() {
        assert!(valid("x-foo"));
        assert!(valid("foo-"));
        assert!(!valid("foo"), "no hyphen");
        assert!(
            !valid("-foo"),
            "first code point is not an ASCII lower alpha"
        );
        assert!(!valid("X-foo"), "first code point is uppercase");
        assert!(!valid("1-foo"));
        assert!(!valid(""));
    }

    #[test]
    fn a_custom_element_name_rejects_uppercase_whitespace_slash_and_gt() {
        assert!(!valid("x-Foo"));
        assert!(!valid("x- foo"));
        assert!(!valid("x-foo/bar"));
        assert!(!valid("x-foo>bar"));
        assert!(!valid("x-foo\tbar"));
        assert!(!valid("x-foo\nbar"));
    }

    #[test]
    fn the_eight_reserved_names_are_not_custom_element_names() {
        for reserved in [
            "annotation-xml",
            "color-profile",
            "font-face",
            "font-face-src",
            "font-face-uri",
            "font-face-format",
            "font-face-name",
            "missing-glyph",
        ] {
            assert!(!valid(reserved), "`{reserved}` is reserved");
        }
        assert!(valid("font-faces"), "only the exact names are reserved");
    }

    /// The living standard's five-clause rule accepts names the retired
    /// `PotentialCustomElementName` production rejected — which is what makes
    /// this a predicate rather than a transcribed grammar.
    #[test]
    fn a_custom_element_name_accepts_astral_and_symbol_code_points() {
        assert!(valid("math-α"));
        assert!(valid("emotion-😍"));
        assert!(valid("arrow-→"), "U+2190 is outside the retired grammar");
    }
}
