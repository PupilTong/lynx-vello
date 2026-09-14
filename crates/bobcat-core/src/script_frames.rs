//! Host display opportunities shared by a view's independent JS event loops.
//! Only a timestamp and pending-demand count cross threads. No callback, JS
//! value, acknowledgement or realm borrow crosses this boundary.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use quickjs_rust_bridge::HostValue;
use tokio::sync::watch;

use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;
use crate::view::EventRequester;

#[derive(Clone, Copy, Default)]
pub(crate) struct FrameTick {
    generation: u64,
    milliseconds: f64,
}

#[derive(Clone)]
pub(crate) struct ScriptFrames(Arc<Clock>);
struct Clock {
    tick: watch::Sender<FrameTick>,
    pending: AtomicUsize,
    requester: Arc<dyn EventRequester>,
}

impl ScriptFrames {
    pub(crate) fn new(requester: Arc<dyn EventRequester>) -> Self {
        Self(Arc::new(Clock {
            tick: watch::channel(FrameTick::default()).0,
            pending: AtomicUsize::new(0),
            requester,
        }))
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.0.pending.load(Ordering::Relaxed) != 0
    }

    /// Called by the painter before entering either realm. A busy consumer
    /// observes only the latest opportunity when its event loop becomes free.
    pub(crate) fn tick(&self, milliseconds: f64) {
        self.0.tick.send_modify(|tick| {
            tick.generation += 1;
            tick.milliseconds = milliseconds;
        });
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<FrameTick> {
        self.0.tick.subscribe()
    }

    pub(crate) fn requests(&self) -> Rc<FrameRequests> {
        Rc::new(FrameRequests {
            clock: self.clone(),
            after: Cell::new(None),
        })
    }
}

/// One realm's contribution to display demand. The last owner dropping it
/// withdraws that contribution, including failed startup and worker GC.
pub(crate) struct FrameRequests {
    clock: ScriptFrames,
    after: Cell<Option<u64>>,
}

impl FrameRequests {
    pub(crate) fn set(&self, pending: bool) {
        if pending == self.after.get().is_some() {
            return;
        }
        let edge = if pending {
            // A callback registered after a missed tick waits for the next
            // opportunity, including requests made by another frame callback.
            self.after.set(Some(self.clock.0.tick.borrow().generation));
            self.clock.0.pending.fetch_add(1, Ordering::Relaxed) == 0
        } else {
            self.after.set(None);
            self.clock.0.pending.fetch_sub(1, Ordering::Relaxed) == 1
        };
        if edge {
            self.clock.0.requester.request_event();
        }
    }

    pub(crate) fn take(&self) -> Option<f64> {
        let after = self.after.get()?;
        let tick = *self.clock.0.tick.borrow();
        if tick.generation <= after {
            return None;
        }
        self.set(false);
        Some(tick.milliseconds)
    }
}

impl Drop for FrameRequests {
    fn drop(&mut self) {
        self.set(false);
    }
}

pub(crate) fn install(
    engine: &mut ScriptEngine,
    js: &mut ScriptRuntime,
    requests: &Rc<FrameRequests>,
) -> Result<(), ScriptError> {
    let requests = Rc::clone(requests);
    engine.register_host_module_function(
        js,
        crate::esm::HOST_MODULE_SPECIFIER,
        "requestScriptFrame",
        1,
        Box::new(move |arguments| {
            let Some(HostValue::Boolean(pending)) = arguments.first() else {
                return Err("requestScriptFrame expects a boolean".to_owned());
            };
            requests.set(*pending);
            Ok(HostValue::Undefined)
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realms_keep_independent_demand_and_observe_the_latest_opportunity() {
        let clock = ScriptFrames::new(Arc::new(crate::NoWakeup));
        let main = clock.requests();
        let background = clock.requests();
        main.set(true);
        background.set(true);
        clock.tick(10.0);
        assert_eq!(main.take(), Some(10.0));
        assert!(clock.is_pending(), "BTS has not consumed its opportunity");
        main.set(true);
        clock.tick(20.0);
        assert_eq!(main.take(), Some(20.0));
        assert_eq!(background.take(), Some(20.0), "missed frames coalesce");
        assert!(!clock.is_pending());
        background.set(true);
        assert_eq!(
            background.take(),
            None,
            "nested requests wait for a later tick"
        );
        clock.tick(30.0);
        assert_eq!(background.take(), Some(30.0));
    }

    #[test]
    fn cancellation_and_realm_release_withdraw_only_their_own_demand() {
        let clock = ScriptFrames::new(Arc::new(crate::NoWakeup));
        let main = clock.requests();
        let background = clock.requests();
        main.set(true);
        main.set(true);
        background.set(true);
        main.set(false);
        assert!(clock.is_pending());
        drop(background);
        assert!(!clock.is_pending());
        clock.tick(1.0);
        assert_eq!(main.take(), None);
    }
}
