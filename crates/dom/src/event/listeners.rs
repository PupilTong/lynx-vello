//! The document's event listener table: which registrant listens for which
//! event type on which node, minus the callback itself.
//!
//! The standard keeps an *event listener list* on every `EventTarget`. A
//! script listener is a JavaScript value in a realm this crate knows nothing
//! about, so the callback stays in the realm; what the realm reports here is
//! the rest of a registration — the node, the type, which pass it runs in,
//! and who holds it. That is enough for this crate to answer three questions
//! no layer above can answer without walking the tree:
//!
//! - a component's: does anything listen for `name` on me ([`Document::has_listener`]), or would an
//!   event I fire reach a listener anywhere ([`Document::would_reach_listener`]) — so an event that
//!   is costly to produce (a `<text>` `layout`, scroll metrics) is produced only when someone hears
//!   it;
//! - a host's, per event: which registrants hold a registration on this path
//!   ([`Document::listeners_on_path`]) and which hold a global one
//!   ([`Document::realm_has_global`]), so a realm with nothing to run is not called at all;
//! - a router's, per name: does anything hear `name` at all ([`Document::hears`]), and each change
//!   that adds the name's first registration or removes its last ([`RegistrationChange`]), which
//!   feeds the presenting thread's name filter.
//!
//! # Why a side table
//!
//! The listener list is per-node state in the standard, and this table keeps
//! it beside the tree rather than on [`Node`](crate::Node) because
//! registrations are sparse: the project's performance assumption is that a
//! page registers on a small fraction of its nodes, and very rarely every
//! listener on every node. A field on `Node` would make every node pay for an
//! empty list. Keyed by node, most probes miss on the first lookup, and the
//! free path ([`Document`]'s one `free_node`) costs one probe per freed node
//! while the table holds anything and no probe when it is empty.
//!
//! # Shape
//!
//! - `by_target`: node → its registrations, one per (name, registrant). Keyed by node alone rather
//!   than by (node, name) so a freed node's rows leave in one removal; a node carries one to three
//!   rows in practice, which the inline storage holds.
//! - `by_realm`: (registrant, name) → its path and global registration counts, which decide whether
//!   a realm takes part in one dispatch without scanning the path for it.
//! - `by_name`: name → the name's id, with per-name path and global counts in `names`. The ids keep
//!   the other two maps' keys a few bytes wide and make a probe from a borrowed `&str` one string
//!   hash, done once per question. A name's id outlives its last registration: the set of names is
//!   the page's event vocabulary, a few dozen strings for the life of the document, so ids are not
//!   recycled.
//!
//! # A name's registration count crossing zero
//!
//! A name is *heard* while at least one registration of any kind holds it.
//! [`Document::add_listener`] and [`Document::remove_listener`] return what
//! their own change did to that count — the first registration of the name
//! added, the last one removed, or neither ([`RegistrationChange`]) — and
//! [`Document::remove_listeners_of`] the names whose last registration it
//! removed. A node freed by a removal or a handle collection has no caller
//! to return that to, so the names whose last registration left with it wait
//! in a queue that [`Document::take_pending_last_removals`] drains. A last
//! removal waiting there that an add undoes before the drain is withdrawn
//! and the add reports [`RegistrationChange::Unchanged`], so a host that
//! applies every returned change as it gets it, and the drained ones
//! whenever it drains, holds the table's set of heard names after each drain.
//! The host drains after every mutation batch, as it drains
//! [`Document::take_observed_name_changes`].

use std::ops::BitOr;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::EventSteps;
use crate::tree::document::{Document, NodeId};

/// One registrant's opaque identity, allocated by the host. A realm in
/// `bobcat-core`; this crate compares it and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ListenerKey(pub u64);

/// Which passes one registration runs in: the standard's `capture` flag
/// split into its two values, plus Lynx's global registration, which hears
/// an event of its name wherever on the page the event is fired.
///
/// A registration holds at least one kind; an empty set is a removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ListenerKinds(u8);

impl ListenerKinds {
    /// A bubble-pass registration: `addEventListener(type, f)`, and Lynx's
    /// `bind`/`catch` forms. Runs at the target and, for a bubbling event,
    /// at every ancestor on the bubble pass.
    pub const PATH_BUBBLE: Self = Self(1);
    /// A capture-pass registration: `addEventListener(type, f, true)`, and
    /// Lynx's `capture-bind`/`capture-catch`. Runs at every node on the
    /// capture pass, whether the event bubbles or not.
    pub const PATH_CAPTURE: Self = Self(1 << 1);
    /// Lynx's `global-bindEvent`: heard for every event of the name, on any
    /// path.
    pub const GLOBAL: Self = Self(1 << 2);

    const PATH: u8 = Self::PATH_BUBBLE.0 | Self::PATH_CAPTURE.0;
    const ALL: u8 = Self::PATH | Self::GLOBAL.0;

    /// No kind: the set an add treats as a removal.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// The set the low three bits of `bits` name, in the constants' order;
    /// higher bits are ignored. The form a host receives from script.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every kind of `other` is in this set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether this set runs on an event path, in either pass.
    #[must_use]
    pub const fn on_path(self) -> bool {
        self.0 & Self::PATH != 0
    }

    /// Whether this set holds [`Self::GLOBAL`].
    #[must_use]
    pub const fn global(self) -> bool {
        self.0 & Self::GLOBAL.0 != 0
    }
}

impl BitOr for ListenerKinds {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// What one change did to a name's registration count — the number of
/// registrations of any kind, by any registrant, on any node, that hold it —
/// and whether that count crossed zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationChange {
    /// The count stayed above zero, or stayed at zero.
    Unchanged,
    /// The count went from zero to one: the first registration of the name
    /// was added.
    FirstAdded,
    /// The count went from one to zero: the last registration of the name
    /// was removed.
    LastRemoved,
}

/// A name's index into [`Listeners::names`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NameId(u32);

/// One registration on one node.
#[derive(Debug, Clone, Copy)]
struct Registration {
    name: NameId,
    key: ListenerKey,
    kinds: ListenerKinds,
}

/// How many registrations hold a path kind and how many hold
/// [`ListenerKinds::GLOBAL`]. A registration holding both counts in both;
/// every registration counts in at least one, so both zero means none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    path: u32,
    global: u32,
}

impl Counts {
    fn add(&mut self, kinds: ListenerKinds) {
        self.path += u32::from(kinds.on_path());
        self.global += u32::from(kinds.global());
    }

    fn subtract(&mut self, kinds: ListenerKinds) {
        self.path -= u32::from(kinds.on_path());
        self.global -= u32::from(kinds.global());
    }

    fn is_zero(self) -> bool {
        self.path == 0 && self.global == 0
    }
}

/// A name's spelling and its document-wide counts.
#[derive(Debug)]
struct NameSlot {
    spelling: Arc<str>,
    counts: Counts,
}

/// The table; see the module doc for its shape and why it is a side table.
#[derive(Debug, Default)]
pub(crate) struct Listeners {
    by_target: FxHashMap<NodeId, SmallVec<[Registration; 2]>>,
    by_realm: FxHashMap<(ListenerKey, NameId), Counts>,
    by_name: FxHashMap<Arc<str>, NameId>,
    names: Vec<NameSlot>,
    /// The names whose last registration [`Self::forget_node`] removed,
    /// waiting for [`Document::take_pending_last_removals`]. Holds a name
    /// only while no registration holds it, so each name at most once.
    pending_last_removals: SmallVec<[(Arc<str>, RegistrationChange); 2]>,
}

impl Listeners {
    /// Whether no node carries a registration: the free path's one test.
    pub(crate) fn is_empty(&self) -> bool {
        self.by_target.is_empty()
    }

    fn id(&self, name: &str) -> Option<NameId> {
        self.by_name.get(name).copied()
    }

    fn counts(&self, id: NameId) -> Counts {
        self.names[id.0 as usize].counts
    }

    fn intern(&mut self, name: &str) -> NameId {
        if let Some(id) = self.id(name) {
            return id;
        }
        let id = NameId(u32::try_from(self.names.len()).expect("fewer than 2^32 event names"));
        let spelling: Arc<str> = Arc::from(name);
        self.names.push(NameSlot {
            spelling: Arc::clone(&spelling),
            counts: Counts::default(),
        });
        self.by_name.insert(spelling, id);
        id
    }

    /// Counts `kinds` in for `key` and `name`.
    fn count_in(&mut self, id: NameId, key: ListenerKey, kinds: ListenerKinds) {
        self.names[id.0 as usize].counts.add(kinds);
        self.by_realm.entry((key, id)).or_default().add(kinds);
    }

    /// Counts `kinds` out for `key` and `name`, answering whether the name
    /// is now unheard. A realm's entry leaves with its last registration.
    fn count_out(
        names: &mut [NameSlot],
        by_realm: &mut FxHashMap<(ListenerKey, NameId), Counts>,
        id: NameId,
        key: ListenerKey,
        kinds: ListenerKinds,
    ) -> bool {
        if let Some(counts) = by_realm.get_mut(&(key, id)) {
            counts.subtract(kinds);
            if counts.is_zero() {
                by_realm.remove(&(key, id));
            }
        }
        let counts = &mut names[id.0 as usize].counts;
        counts.subtract(kinds);
        counts.is_zero()
    }

    /// Drops every registration on `node` — the node is being freed — and
    /// queues each name whose last registration that removed.
    pub(crate) fn forget_node(&mut self, node: NodeId) {
        let Some(rows) = self.by_target.remove(&node) else {
            return;
        };
        for row in rows {
            if Self::count_out(
                &mut self.names,
                &mut self.by_realm,
                row.name,
                row.key,
                row.kinds,
            ) {
                let spelling = Arc::clone(&self.names[row.name.0 as usize].spelling);
                self.pending_last_removals
                    .push((spelling, RegistrationChange::LastRemoved));
            }
        }
    }

    /// Visits, in capture order and each once, the registrants holding a
    /// registration of name `id` that runs on one of `steps`, until `visit`
    /// answers `false`.
    ///
    /// The capture pass names every node of the path once, so one probe per
    /// node covers both passes: a capture registration runs on that node's
    /// capture step, and a bubble registration on its bubble step, which the
    /// node has when it is an at-target entry or the event bubbles.
    fn visit_on_path(
        &self,
        steps: &EventSteps,
        id: NameId,
        mut visit: impl FnMut(ListenerKey) -> bool,
    ) {
        let mut seen: SmallVec<[ListenerKey; 2]> = SmallVec::new();
        for step in steps.steps().iter().filter(|step| step.capture) {
            let Some(rows) = self.by_target.get(&step.node) else {
                continue;
            };
            let has_bubble_step = steps.bubbles || step.node == step.target;
            for row in rows {
                if row.name != id || seen.contains(&row.key) {
                    continue;
                }
                let runs = row.kinds.contains(ListenerKinds::PATH_CAPTURE)
                    || (has_bubble_step && row.kinds.contains(ListenerKinds::PATH_BUBBLE));
                if runs {
                    seen.push(row.key);
                    if !visit(row.key) {
                        return;
                    }
                }
            }
        }
    }
}

impl<T> Document<T> {
    /// Records that `key` listens for `name` on `node` in the passes `kinds`
    /// names, and answers what that did to whether `name` is heard.
    ///
    /// One registration per (node, name, registrant): a second add for the
    /// same three **replaces** the kinds, so a registrant that adds a global
    /// form beside a bubble one reports the union again rather than a second
    /// registration, and the counts move by the difference. An empty `kinds`
    /// is [`Self::remove_listener`]. A node that is not live registers
    /// nothing, which is what keeps a freed node's id out of the table: its
    /// rows left with it ([`Self::take_pending_last_removals`]).
    ///
    /// [`RegistrationChange::FirstAdded`] when this is the name's first
    /// registration, unless the name's last one went with a freed node since
    /// the last [`Self::take_pending_last_removals`]: that waiting last
    /// removal is withdrawn instead and the answer is
    /// [`RegistrationChange::Unchanged`], because the host was never told the
    /// last registration was removed.
    pub fn add_listener(
        &mut self,
        node: NodeId,
        name: &str,
        kinds: ListenerKinds,
        key: ListenerKey,
    ) -> RegistrationChange {
        if kinds.is_empty() {
            return self.remove_listener(node, name, key);
        }
        if !self.contains_node(node) {
            return RegistrationChange::Unchanged;
        }
        let table = &mut self.listeners;
        let id = table.intern(name);
        let was_heard = !table.counts(id).is_zero();
        let rows = table.by_target.entry(node).or_default();
        if let Some(row) = rows.iter_mut().find(|row| row.name == id && row.key == key) {
            let old = std::mem::replace(&mut row.kinds, kinds);
            if old != kinds {
                Listeners::count_out(&mut table.names, &mut table.by_realm, id, key, old);
                table.count_in(id, key, kinds);
            }
            return RegistrationChange::Unchanged;
        }
        rows.push(Registration {
            name: id,
            key,
            kinds,
        });
        table.count_in(id, key, kinds);
        if was_heard {
            return RegistrationChange::Unchanged;
        }
        if let Some(index) = table
            .pending_last_removals
            .iter()
            .position(|(waiting, _)| **waiting == *name)
        {
            table.pending_last_removals.remove(index);
            return RegistrationChange::Unchanged;
        }
        RegistrationChange::FirstAdded
    }

    /// Drops `key`'s registration for `name` on `node`, whatever its kinds,
    /// and answers [`RegistrationChange::LastRemoved`] when it was the name's
    /// last. Removing what is not registered is
    /// [`RegistrationChange::Unchanged`].
    pub fn remove_listener(
        &mut self,
        node: NodeId,
        name: &str,
        key: ListenerKey,
    ) -> RegistrationChange {
        let table = &mut self.listeners;
        let Some(id) = table.id(name) else {
            return RegistrationChange::Unchanged;
        };
        let Some(rows) = table.by_target.get_mut(&node) else {
            return RegistrationChange::Unchanged;
        };
        let Some(index) = rows.iter().position(|row| row.name == id && row.key == key) else {
            return RegistrationChange::Unchanged;
        };
        let row = rows.remove(index);
        if rows.is_empty() {
            table.by_target.remove(&node);
        }
        let last_removed =
            Listeners::count_out(&mut table.names, &mut table.by_realm, id, key, row.kinds);
        if last_removed {
            RegistrationChange::LastRemoved
        } else {
            RegistrationChange::Unchanged
        }
    }

    /// Drops every registration `key` holds — the registrant is going away —
    /// and answers the names whose last registration that removed, each once.
    ///
    /// Scans the whole table: a registrant's rows are indexed by name, not by
    /// node, and a realm goes away rarely enough that a per-registrant node
    /// index would be paid for on every add to save work here.
    pub fn remove_listeners_of(&mut self, key: ListenerKey) -> SmallVec<[Arc<str>; 4]> {
        let Listeners {
            by_target,
            by_realm,
            names,
            ..
        } = &mut self.listeners;
        let mut last_removed = SmallVec::new();
        by_target.retain(|_, rows| {
            rows.retain(|row| {
                if row.key != key {
                    return true;
                }
                if Listeners::count_out(names, by_realm, row.name, row.key, row.kinds) {
                    last_removed.push(Arc::clone(&names[row.name.0 as usize].spelling));
                }
                false
            });
            !rows.is_empty()
        });
        debug_assert!(
            by_realm.keys().all(|(holder, _)| *holder != key),
            "a registrant's counts leave with its last registration"
        );
        last_removed
    }

    /// The names whose last registration a freed node removed since the last
    /// call, each name once, in the order it happened.
    ///
    /// A node freed by a removal ([`Self::drop_element`],
    /// [`Self::drop_subtree`]) takes its registrations with it, in the
    /// document's one free path, and a name whose last registration that was
    /// reaches zero there with no caller to return the change to. A last
    /// removal an add undid before this call is not here and that add
    /// reported [`RegistrationChange::Unchanged`] (see [`Self::add_listener`]),
    /// so applying these after the changes the calls returned leaves a host
    /// with the table's set of heard names. Every entry is
    /// [`RegistrationChange::LastRemoved`] today; the pair names the change so
    /// a host forwards it unchanged. Empty and allocation-free when no freed
    /// node held a registration.
    ///
    /// A host drains this after every mutation batch it runs, before it next
    /// trusts its own copy of the set.
    pub fn take_pending_last_removals(&mut self) -> SmallVec<[(Arc<str>, RegistrationChange); 2]> {
        std::mem::take(&mut self.listeners.pending_last_removals)
    }

    /// Whether any registrant registered for `name` on `node`, in any kind:
    /// a component's question before it produces an event only a listener
    /// on itself would hear. One name lookup and one node probe.
    ///
    /// Reads the table as it is now, so a registration a realm made earlier
    /// in the same turn is already visible.
    #[must_use]
    pub fn has_listener(&self, node: NodeId, name: &str) -> bool {
        let table = &self.listeners;
        let Some(id) = table.id(name) else {
            return false;
        };
        table
            .by_target
            .get(&node)
            .is_some_and(|rows| rows.iter().any(|row| row.name == id))
    }

    /// Whether anything hears `name`: a registration of any kind by any
    /// registrant on any node, or a definition that observes it
    /// ([`Self::observes_event`]). One name lookup and one set lookup — the
    /// whole cost of an event nothing hears, which a host checks before it
    /// builds a path or an [`EventWalk`](super::EventWalk) at all.
    #[must_use]
    pub fn hears(&self, name: &str) -> bool {
        let table = &self.listeners;
        table.id(name).is_some_and(|id| !table.counts(id).is_zero()) || self.observes_event(name)
    }

    /// Whether any registration of any kind, by any registrant, on any node
    /// holds `name`: the set whose membership changes [`RegistrationChange`]
    /// reports, as the name's registration count crosses zero.
    ///
    /// [`Self::hears`] without the definitions. A host that keeps its own
    /// router's name set as the union of this answer and a set of its own —
    /// the names a *live* element of an observing definition wants, which is
    /// narrower than the per-definition set [`Self::hears`] reads — asks
    /// this. One name lookup.
    #[must_use]
    pub fn has_listeners_for(&self, name: &str) -> bool {
        let table = &self.listeners;
        table.id(name).is_some_and(|id| !table.counts(id).is_zero())
    }

    /// The registrants a dispatch of `name` over `steps` reaches through a
    /// path registration, each once, in the order the capture pass first
    /// meets them (root inward).
    ///
    /// A capture registration counts on its node's capture step and a bubble
    /// registration on its node's bubble step, so for an event that does not
    /// bubble an ancestor's bubble registration is not reached — the target's
    /// is, since the target keeps its bubble turn. Global registrations are
    /// not path registrations; [`Self::realm_has_global`] answers those. One
    /// name lookup, then one probe per node of the path, most of which miss.
    #[must_use]
    pub fn listeners_on_path(&self, steps: &EventSteps, name: &str) -> SmallVec<[ListenerKey; 2]> {
        let table = &self.listeners;
        let mut reached = SmallVec::new();
        let Some(id) = table.id(name) else {
            return reached;
        };
        if table.counts(id).path == 0 {
            return reached;
        }
        table.visit_on_path(steps, id, |key| {
            reached.push(key);
            true
        });
        reached
    }

    /// Whether an event of `name` fired at `target` would reach any
    /// registration: a component's gate before it produces an event that
    /// someone other than itself may hear.
    ///
    /// No registration of the name is one lookup and `false`; a global
    /// registration anywhere is `true`, since a global registration hears
    /// the name on any path; otherwise the path is built and probed as
    /// [`Self::listeners_on_path`] probes it, stopping at the first hit. A
    /// target that is not live reaches nothing. Definitions are not
    /// consulted: a component asks this about script listeners.
    #[must_use]
    pub fn would_reach_listener(
        &self,
        target: NodeId,
        name: &str,
        bubbles: bool,
        composed: bool,
    ) -> bool {
        let table = &self.listeners;
        let Some(id) = table.id(name) else {
            return false;
        };
        let counts = table.counts(id);
        if counts.global > 0 {
            return true;
        }
        if counts.path == 0 || !self.contains_node(target) {
            return false;
        }
        let steps = self.event_steps(target, bubbles, composed);
        let mut reached = false;
        table.visit_on_path(&steps, id, |_| {
            reached = true;
            false
        });
        reached
    }

    /// Whether `key` holds a global registration for `name` anywhere: a
    /// realm that does is called for every event of the name, whatever its
    /// path. One lookup.
    #[must_use]
    pub fn realm_has_global(&self, key: ListenerKey, name: &str) -> bool {
        self.realm_registration_counts(key, name).1 > 0
    }

    /// How many of `key`'s registrations for `name` hold a path kind, and
    /// how many hold [`ListenerKinds::GLOBAL`]; a registration holding both
    /// counts in both. One lookup.
    #[must_use]
    pub fn realm_registration_counts(&self, key: ListenerKey, name: &str) -> (u32, u32) {
        let table = &self.listeners;
        table
            .id(name)
            .and_then(|id| table.by_realm.get(&(key, id)))
            .map_or((0, 0), |counts| (counts.path, counts.global))
    }
}
