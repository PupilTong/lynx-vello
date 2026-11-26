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
//!   ([`Document::listeners_on_path`]), which hold a global one the path reaches
//!   ([`Document::global_registrants_on_path`]) and which hold a global one anywhere
//!   ([`Document::global_registrants`]), so a registrant with nothing to run is not called at all;
//! - a router's, per name: does anything hear `name` at all ([`Document::hears`]), and each change
//!   that adds the name's first registration or removes its last ([`RegistrationChange`]), which
//!   feeds the presenting thread's name filter.
//!
//! # Why a side table
//!
//! The listener list is per-node state in the standard, and this table keeps
//! it beside the tree rather than on [`Node`](crate::tree::node::Node) because
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
//! - `global_rows`: name → the (node, registrant) of each registration holding
//!   [`ListenerKinds::GLOBAL`], in the order they were made, so which global registrations a path
//!   reaches, and who holds one at all, is a scan of the name's few global rows rather than of the
//!   table.
//! - `by_name`: name → the name's id, with per-name path, capture and global counts in `names`. The
//!   ids keep the rows and `global_rows`' keys a few bytes wide and make a probe from a borrowed
//!   `&str` one string hash, done once per question. The counts are document-wide: they answer
//!   whether anything hears a name ([`Document::hears`]) and let a path or global probe of a name
//!   with no registration of that kind return at once. The capture count is what lets
//!   [`Document::would_reach_listener`] answer a non-bubbling, non-composed event from the target's
//!   own rows: with no global and no capture registration of the name, only a registration on the
//!   target itself runs on that event's path, so no path is built. A name's id outlives its last
//!   registration: the set of names is the page's event vocabulary, a few dozen strings for the
//!   life of the document, so ids are not recycled.
//!
//! Nothing is kept per registrant: which registrants a dispatch reaches is read from the rows of
//! the path's nodes and the name's global rows, and a registrant's removal
//! ([`Document::remove_listeners_of`]) scans the table.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ListenerKey(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ListenerKinds(u8);

impl ListenerKinds {
    pub const PATH_BUBBLE: Self = Self(1);
    pub const PATH_CAPTURE: Self = Self(1 << 1);
    pub const GLOBAL: Self = Self(1 << 2);

    const PATH: u8 = Self::PATH_BUBBLE.0 | Self::PATH_CAPTURE.0;
    const ALL: u8 = Self::PATH | Self::GLOBAL.0;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

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

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn on_path(self) -> bool {
        self.0 & Self::PATH != 0
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationChange {
    Unchanged,
    FirstAdded,
    LastRemoved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NameId(u32);

#[derive(Debug, Clone, Copy)]
struct Registration {
    name: NameId,
    key: ListenerKey,
    kinds: ListenerKinds,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    path: u32,
    capture: u32,
    global: u32,
}

impl Counts {
    fn add(&mut self, kinds: ListenerKinds) {
        self.path += u32::from(kinds.on_path());
        self.capture += u32::from(kinds.contains(ListenerKinds::PATH_CAPTURE));
        self.global += u32::from(kinds.global());
    }

    fn subtract(&mut self, kinds: ListenerKinds) {
        self.path -= u32::from(kinds.on_path());
        self.capture -= u32::from(kinds.contains(ListenerKinds::PATH_CAPTURE));
        self.global -= u32::from(kinds.global());
    }

    fn is_zero(self) -> bool {
        self.path == 0 && self.global == 0
    }
}

#[derive(Debug)]
struct NameSlot {
    spelling: Arc<str>,
    counts: Counts,
}

#[derive(Debug, Default)]
pub(crate) struct Listeners {
    by_target: FxHashMap<NodeId, SmallVec<[Registration; 2]>>,
    global_rows: FxHashMap<NameId, SmallVec<[(NodeId, ListenerKey); 2]>>,
    by_name: FxHashMap<Arc<str>, NameId>,
    names: Vec<NameSlot>,
    pending_last_removals: SmallVec<[(Arc<str>, RegistrationChange); 2]>,
}

impl Listeners {
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

    fn count_in(&mut self, id: NameId, kinds: ListenerKinds) {
        self.names[id.0 as usize].counts.add(kinds);
    }

    fn count_out(names: &mut [NameSlot], id: NameId, kinds: ListenerKinds) -> bool {
        let counts = &mut names[id.0 as usize].counts;
        counts.subtract(kinds);
        counts.is_zero()
    }

    fn global_in(
        global_rows: &mut FxHashMap<NameId, SmallVec<[(NodeId, ListenerKey); 2]>>,
        id: NameId,
        node: NodeId,
        key: ListenerKey,
    ) {
        global_rows.entry(id).or_default().push((node, key));
    }

    fn global_out(
        global_rows: &mut FxHashMap<NameId, SmallVec<[(NodeId, ListenerKey); 2]>>,
        id: NameId,
        node: NodeId,
        key: ListenerKey,
    ) {
        if let Some(rows) = global_rows.get_mut(&id) {
            rows.retain(|row| *row != (node, key));
            if rows.is_empty() {
                global_rows.remove(&id);
            }
        }
    }

    pub(crate) fn forget_node(&mut self, node: NodeId) {
        let Some(rows) = self.by_target.remove(&node) else {
            return;
        };
        for row in rows {
            if row.kinds.global() {
                Self::global_out(&mut self.global_rows, row.name, node, row.key);
            }
            if Self::count_out(&mut self.names, row.name, row.kinds) {
                let spelling = Arc::clone(&self.names[row.name.0 as usize].spelling);
                self.pending_last_removals
                    .push((spelling, RegistrationChange::LastRemoved));
            }
        }
    }

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
                Listeners::count_out(&mut table.names, id, old);
                table.count_in(id, kinds);
            }
            match (old.global(), kinds.global()) {
                (false, true) => Listeners::global_in(&mut table.global_rows, id, node, key),
                (true, false) => Listeners::global_out(&mut table.global_rows, id, node, key),
                _ => {}
            }
            return RegistrationChange::Unchanged;
        }
        rows.push(Registration {
            name: id,
            key,
            kinds,
        });
        table.count_in(id, kinds);
        if kinds.global() {
            Listeners::global_in(&mut table.global_rows, id, node, key);
        }
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
        if row.kinds.global() {
            Listeners::global_out(&mut table.global_rows, id, node, key);
        }
        let last_removed = Listeners::count_out(&mut table.names, id, row.kinds);
        if last_removed {
            RegistrationChange::LastRemoved
        } else {
            RegistrationChange::Unchanged
        }
    }

    pub fn remove_listeners_of(&mut self, key: ListenerKey) -> SmallVec<[Arc<str>; 4]> {
        let Listeners {
            by_target,
            global_rows,
            names,
            ..
        } = &mut self.listeners;
        global_rows.retain(|_, rows| {
            rows.retain(|(_, holder)| *holder != key);
            !rows.is_empty()
        });
        let mut last_removed = SmallVec::new();
        by_target.retain(|_, rows| {
            rows.retain(|row| {
                if row.key != key {
                    return true;
                }
                if Listeners::count_out(names, row.name, row.kinds) {
                    last_removed.push(Arc::clone(&names[row.name.0 as usize].spelling));
                }
                false
            });
            !rows.is_empty()
        });
        last_removed
    }

    pub fn take_pending_last_removals(&mut self) -> SmallVec<[(Arc<str>, RegistrationChange); 2]> {
        std::mem::take(&mut self.listeners.pending_last_removals)
    }

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

    #[must_use]
    pub fn hears(&self, name: &str) -> bool {
        let table = &self.listeners;
        table.id(name).is_some_and(|id| !table.counts(id).is_zero()) || self.observes_event(name)
    }

    #[must_use]
    pub fn has_listeners_for(&self, name: &str) -> bool {
        let table = &self.listeners;
        table.id(name).is_some_and(|id| !table.counts(id).is_zero())
    }

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
        if !bubbles && !composed && counts.capture == 0 {
            return table
                .by_target
                .get(&target)
                .is_some_and(|rows| rows.iter().any(|row| row.name == id));
        }
        let steps = self.event_steps(target, bubbles, composed);
        let mut reached = false;
        table.visit_on_path(&steps, id, |_| {
            reached = true;
            false
        });
        reached
    }

    #[must_use]
    pub fn global_registrants_on_path(
        &self,
        steps: &EventSteps,
        name: &str,
    ) -> SmallVec<[ListenerKey; 2]> {
        let table = &self.listeners;
        let mut reached = SmallVec::new();
        let Some(rows) = table.id(name).and_then(|id| table.global_rows.get(&id)) else {
            return reached;
        };
        for &(node, key) in rows {
            if reached.contains(&key) {
                continue;
            }
            let root = self.tree_root(node);
            if steps
                .steps()
                .iter()
                .any(|step| step.capture && step.node == root)
            {
                reached.push(key);
            }
        }
        reached
    }

    #[must_use]
    pub fn global_registrants(&self, name: &str) -> SmallVec<[ListenerKey; 2]> {
        let table = &self.listeners;
        let mut registrants = SmallVec::new();
        let Some(rows) = table.id(name).and_then(|id| table.global_rows.get(&id)) else {
            return registrants;
        };
        for &(_, key) in rows {
            if !registrants.contains(&key) {
                registrants.push(key);
            }
        }
        registrants
    }
}
