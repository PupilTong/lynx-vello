//! Data shared between the Lynx main thread and the presenting thread.

use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use dom::CommittedFrame;
use rustc_hash::FxHashSet;

#[derive(Debug)]
pub(crate) enum ListenerUpdate {
    Available(Arc<str>),
    Unavailable(Arc<str>),
}

#[derive(Debug)]
pub(crate) struct ListenerNames {
    names: FxHashSet<Arc<str>>,
    updates: mpsc::Receiver<ListenerUpdate>,
}

impl ListenerNames {
    pub(crate) fn new(updates: mpsc::Receiver<ListenerUpdate>) -> Self {
        Self {
            names: FxHashSet::default(),
            updates,
        }
    }

    pub(crate) fn sync(&mut self) {
        while let Ok(update) = self.updates.try_recv() {
            match update {
                ListenerUpdate::Available(name) => {
                    self.names.insert(name);
                }
                ListenerUpdate::Unavailable(name) => {
                    self.names.remove(&name);
                }
            }
        }
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

#[derive(Debug, Default)]
pub(crate) struct FrameHub {
    latest: Mutex<Option<Arc<CommittedFrame>>>,
    begin_frames: Mutex<BeginFrameLedger>,
    begin_frame_signal: Condvar,
}

#[derive(Debug, Default)]
struct BeginFrameLedger {
    serviced: u64,
    committer_gone: bool,
}

impl FrameHub {
    pub(crate) fn publish(&self, frame: Arc<CommittedFrame>) {
        *self
            .latest
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}")) = Some(frame);
    }

    pub(crate) fn latest(&self) -> Option<Arc<CommittedFrame>> {
        self.latest
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"))
            .clone()
    }

    pub(crate) fn note_begin_frame_serviced(&self, seq: u64) {
        let mut ledger = self
            .begin_frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
        ledger.serviced = seq.max(ledger.serviced);
        self.begin_frame_signal.notify_all();
    }

    pub(crate) fn note_committer_gone(&self) {
        let mut ledger = self
            .begin_frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
        ledger.committer_gone = true;
        self.begin_frame_signal.notify_all();
    }

    pub(crate) fn wait_begin_frame(&self, seq: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut ledger = self
            .begin_frames
            .lock()
            .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
        while ledger.serviced < seq {
            if ledger.committer_gone {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = self
                .begin_frame_signal
                .wait_timeout(ledger, deadline - now)
                .unwrap_or_else(|error| panic!("the frame hub is poisoned: {error}"));
            ledger = next;
        }
        true
    }
}
