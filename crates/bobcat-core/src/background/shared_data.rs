//! `lynx.setSharedData`'s store: one per group, on `bobcat-workers`.
//!
//! Native and web-core both keep it on the background thread's JS global —
//! lynx-core's `nativeGlobal.sharedData`, which every card of a group reads
//! because a group shares that one JS context (native's `LynxGroup`,
//! web-core's `lynx-group-id` worker). Here every view's BTS is a realm of
//! its own on the group's one worker runtime, and a realm value never
//! crosses the host boundary as itself, so the store holds what the boundary
//! carries: a primitive, or a structured clone. `getSharedData` answers a
//! fresh copy of what was set, in whichever of the group's realms asks — the
//! one that set it included.
//!
//! The store lives while any worker realm of the group does, and no longer:
//! the thread holds it weakly and every [`Worker`](super::thread) holds it
//! strongly. Native releases the shared context with the group's last view
//! (`SharedJSContextWrapper::Def`) and web-core terminates the group's worker
//! with its last card, so a view that starts after every earlier one is gone
//! finds the store empty there as well.
//!
//! An observer is the realm's own (`registerSharedDataObserver`), and the
//! realm calls those itself, synchronously, inside the `setSharedData` that
//! changed the value. What the store keeps is which *other* realms have at
//! least one, and how to reach each: a set queues one delivery into every
//! such sibling, carrying the copy, which that realm hands to its observers
//! in a job of its own after the setter's job has returned.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_rust_bridge::HostValue;
use rustc_hash::FxHashMap;

use super::WorkerKey;

/// How a change reaches one realm with observers: the key and the value as
/// the store holds it, handed over while the setter's job is still running,
/// so it may only queue.
pub(super) type Notify = Rc<dyn Fn(&str, &HostValue)>;

/// The group's shared data and the realms observing it.
#[derive(Default)]
pub(super) struct SharedData {
    values: RefCell<FxHashMap<String, HostValue>>,
    /// The realms with at least one observer, in the order each gained its
    /// first: the order lynx-core calls observers in, as far as a realm
    /// boundary keeps one.
    observers: RefCell<Vec<(WorkerKey, Notify)>>,
}

impl SharedData {
    /// Stores `value` under `key` for the whole group, and queues a delivery
    /// of it into every realm observing the store other than `from`, the
    /// realm that set it and notifies its own observers itself.
    pub(super) fn set(&self, from: WorkerKey, key: String, value: HostValue) {
        for (observer, notify) in self.observers.borrow().iter() {
            if *observer != from {
                notify(&key, &value);
            }
        }
        self.values.borrow_mut().insert(key, value);
    }

    /// A copy of what is stored under `key`: `undefined` for a key nothing
    /// set, as lynx-core's read of a missing property answers.
    pub(super) fn get(&self, key: &str) -> HostValue {
        self.values
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or(HostValue::Undefined)
    }

    /// Whether the realm `worker` hears what its siblings set: `Some` when
    /// its first observer arrives, `None` when its last one leaves or the
    /// worker ends.
    pub(super) fn observe(&self, worker: WorkerKey, notify: Option<Notify>) {
        let mut observers = self.observers.borrow_mut();
        observers.retain(|(observer, _)| *observer != worker);
        if let Some(notify) = notify {
            observers.push((worker, notify));
        }
    }
}
