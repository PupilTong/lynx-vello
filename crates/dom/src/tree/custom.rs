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

use crate::tree::document::{DOCUMENT_ELEMENT_NODE_ID, Document, NodeId};

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

    /// The standard's custom element constructor.
    ///
    /// Runs once per element, from [`Document::create_element`], before that
    /// call returns its id — so an element is fully built by the time its
    /// creator can do anything with it. There is no replay of attributes the
    /// element already carried, because the definition-before-creation
    /// contract means a brand-new element carries none.
    ///
    /// The element's state is `Constructing` for the duration, which is not
    /// `Custom`, and two things follow. Attribute writes performed here raise
    /// nothing, so a constructor that normalizes its own attributes is not
    /// reported back to itself. And an element this constructor inserts into
    /// the connected tree receives **no** [`Self::connected_callback`]:
    /// insertion enqueues only for a `Custom` element, and nothing re-checks
    /// at the transition. The standard forbids a constructor from gaining
    /// children or a parent anyway, so a definition that respects it never
    /// meets this.
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
    /// and `style` are ordinary attributes here and fire like any other.
    ///
    /// Only reports for a `Custom` element, so nothing arrives for an
    /// attribute written by [`Self::constructed`] on its own element, and
    /// nothing is replayed for attributes an element carried before its
    /// definition existed — the definition-before-creation contract means
    /// there are none.
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

/// Where an element sits in its definition's construction.
///
/// The standard's `undefined` and `failed` states have no representative here:
/// `undefined` exists because a script can define a tag after elements of it
/// already exist, which this crate's definition-before-creation contract makes
/// unreachable, and `failed` exists to police a constructor that can throw.
/// `Constructing` is the standard's `precustomized` under the name that
/// survives without upgrade, and it earns its place by suppressing the
/// reactions a constructor's own mutations would otherwise raise back at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum CustomElementState {
    /// Every element whose tag has no definition. The majority state, and
    /// every element matches `:defined` regardless — see the module's
    /// recorded limits.
    #[default]
    Uncustomized,
    /// The constructor is on the stack.
    Constructing,
    /// Constructed. The exact gate every lifecycle enqueue site tests.
    Custom,
}

/// One item of an element's custom element reaction queue. The element is the
/// map key, so it is not repeated here.
enum Reaction {
    /// The definition's constructor. Queued rather than called from
    /// [`Document::create_element`] directly, so it obeys the same
    /// drain-at-the-boundary rule as every other reaction.
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

    /// Refuses to free any node in `root`'s subtree that an in-flight
    /// operation is still holding.
    ///
    /// A read-only pass, run **before** the arena is touched. Asserting inside
    /// the destroy loop instead would fire only after `try_remove` had already
    /// freed slots, so a callback that catches the panic would be left holding
    /// a half-destroyed subtree and its caller an id whose node is gone —
    /// which is the exact failure the pin exists to prevent.
    ///
    /// Gated on there being any pin at all, so an ordinary removal walks
    /// nothing: pins exist only while a creation, a removal, or a constructor
    /// is on the stack.
    pub(crate) fn assert_subtree_not_pinned(&self, root: NodeId) {
        if self.custom_elements.pinned.is_empty() {
            return;
        }
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Some(node) = self.get(current) else {
                continue;
            };
            self.assert_not_pinned(current);
            stack.extend_from_slice(node.child_ids());
            if let Some(shadow_root) = node.shadow_root_id() {
                stack.push(shadow_root);
            }
        }
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

impl<T> Document<T> {
    /// Registers `element` as the behavior of every element whose tag is
    /// `local_name`, from this point on.
    ///
    /// One handler per name, so a definition is per-tag rather than per
    /// instance; the callbacks identify their element by [`NodeId`]. The name
    /// is injected rather than known, the same way [`Document::new`] takes the
    /// document element's tag — this crate still owns no tag vocabulary.
    ///
    /// **Every definition must be installed before any element with its tag is
    /// created.** A definition never reaches an element that already exists —
    /// there is no upgrade here — so violating that would leave the element
    /// silently unconstructed; it panics instead. The document element is the
    /// one exception, because `Document::new` creates it before any definition
    /// can exist: it is constructed here.
    ///
    /// Panics on an empty name, on a name that already has a definition (the
    /// standard throws `NotSupportedError` for a duplicate), and on a name that
    /// already has elements.
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

        // The contract, enforced rather than documented: an element created
        // before its definition would never be constructed, and nothing later
        // moves it into one. Scanning the arena rather than the document also
        // catches a detached element that is about to be inserted. `define` is
        // a setup-time call, so this is cold; `create_element` pays nothing
        // for it.
        let existing = self
            .tree()
            .iter()
            .find(|(id, node)| {
                *id != DOCUMENT_ELEMENT_NODE_ID && node.local_name.as_ref() == Some(&name)
            })
            .map(|(id, _)| id);
        assert!(
            existing.is_none(),
            "Document::define: `{local_name}` already has elements, and a definition never \
             reaches an element created before it — install every definition before building \
             the tree"
        );

        // The document element is the one node that cannot obey that contract:
        // `Document::new` creates it before any definition can exist. It is a
        // single known node rather than a general upgrade sweep, so it is
        // constructed here instead of being refused.
        let root_matches = self
            .get(DOCUMENT_ELEMENT_NODE_ID)
            .is_some_and(|node| node.local_name.as_ref() == Some(&name));
        if root_matches {
            let base = self.begin_reactions();
            {
                let root = self.live_node_mut(DOCUMENT_ELEMENT_NODE_ID);
                root.custom_definition = Some(definition);
                root.custom_state = CustomElementState::Constructing;
            }
            self.enqueue_reaction(DOCUMENT_ELEMENT_NODE_ID, Reaction::Constructed);
            self.enqueue_reaction(DOCUMENT_ELEMENT_NODE_ID, Reaction::Connected);
            self.drain_reactions(base);
        }
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
        // Cleared whenever set, so a later unrelated panic is not excused by
        // this one — but read first, because this runs on every mutation and
        // the flag is almost always already false.
        if self.custom_elements.abandoned.load(Ordering::Relaxed) {
            self.custom_elements
                .abandoned
                .store(false, Ordering::Release);
        }
        self.custom_elements.element_queue.len()
    }

    /// The standard's *invoke custom element reactions*, for one scope.
    pub(crate) fn drain_reactions(&mut self, base: usize) {
        if self.custom_elements.element_queue.len() == base {
            return;
        }
        let _depth =
            ReactionDepthToken::enter(&self.custom_elements.depth, &self.custom_elements.abandoned);
        let mut budget = MAX_REACTIONS_PER_SCOPE;
        let mut cursor = base;
        // The outer loop re-reads `len()`: a callback that enqueues onto an
        // element past the cursor is picked up here, not deferred.
        while cursor < self.custom_elements.element_queue.len() {
            let element = self.custom_elements.element_queue[cursor];
            cursor += 1;
            // A live drain, not iteration over a snapshot: the standard says
            // "repeat until reactions is empty", and a callback can append to
            // the queue of the very element being drained. Iterating a
            // snapshot would silently drop whatever it added.
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
            Reaction::Constructed => self.construct_element(element),
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

    /// Runs the definition's constructor for an element that was created with
    /// its tag already defined.
    ///
    /// This is the standard's *upgrade an element* with everything upgrade
    /// needed stripped out. There is no state guard, because an element
    /// reaches this exactly once — at creation, from the one site that sets
    /// `Constructing` — and no attribute replay, because a
    /// definition-before-creation contract means a brand-new element carries
    /// no attributes yet.
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
        // Pinned across the call: the id is one this element's creator will
        // still be naming when the drain returns, and slab keys are recycled.
        self.pin_node(element);
        handler.constructed(self, element);
        self.unpin_node(element);

        // Keyed on the state this path wrote, not on slab occupancy, which
        // would also answer for a recycled id.
        let still_constructing = self
            .get(element)
            .is_some_and(|node| node.custom_state == CustomElementState::Constructing);
        if still_constructing {
            self.live_node_mut(element).custom_state = CustomElementState::Custom;
        }
    }

    /// Binds a freshly created element to its definition and queues its
    /// constructor, when its tag is defined.
    ///
    /// The whole state machine is here: an element is `Custom` if and only if
    /// its tag had a definition at the moment it was created. Nothing later
    /// moves an element into a definition, which is what the
    /// definition-before-creation contract buys.
    pub(crate) fn note_custom_element_created(&mut self, element: NodeId, local_name: &LocalName) {
        let Some(definition) = self.custom_elements.by_name.get(local_name).copied() else {
            return;
        };
        {
            let node = self.live_node_mut(element);
            node.custom_definition = Some(definition);
            // Not `Custom` yet: the constructor's own attribute writes must
            // not be reported back to it, and `observes_attribute` gates on
            // exactly this.
            node.custom_state = CustomElementState::Constructing;
        }
        self.enqueue_reaction(element, Reaction::Constructed);
    }

    /// The insertion half of the lifecycle: every already-constructed element
    /// in the inserted subtree connects.
    ///
    /// No upgrade arm. An element that is not `Custom` here never will be, so
    /// insertion has nothing to do for it.
    pub(crate) fn note_custom_elements_inserted(&mut self, root: NodeId, connected: bool) {
        if self.custom_elements.is_empty() || !connected {
            return;
        }
        let mut inserted = SmallVec::<[NodeId; 8]>::new();
        self.collect_shadow_including_inclusive(root, &mut inserted);
        for element in inserted {
            if self.live(element).custom_state == CustomElementState::Custom {
                self.enqueue_reaction(element, Reaction::Connected);
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
    /// The gate every attribute setter takes first. Only a `Custom` element
    /// reports — `Constructing` is deliberately excluded, which is what stops
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
    /// recycled id cannot inherit them.
    ///
    /// The pin check is deliberately **not** here: by the time the destroy
    /// loop reaches this, the slot is already gone. It runs as a preflight
    /// instead — see [`Self::assert_subtree_not_pinned`].
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

    /// Whether any definition exists. The gate a caller takes **before**
    /// computing an argument for a lifecycle hook, not just before calling it:
    /// `is_connected` walks to the document node, so a definition-free document
    /// would otherwise pay a depth-proportional walk per structural mutation
    /// for an answer nothing reads.
    #[must_use]
    pub(crate) fn has_custom_element_definitions(&self) -> bool {
        !self.custom_elements.is_empty()
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
