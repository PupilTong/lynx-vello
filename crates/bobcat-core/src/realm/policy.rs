//! What a failure in a realm is reported as: one table per realm kind, read by
//! where the failure happened.
//!
//! Every failure the engine reports while it drives a realm is looked up here
//! — by the view's page for its MTS realm, and by a worker for its own, the
//! BTS's included — and nowhere else decides it. A table is a pure function
//! from a [`Scene`] to a [`Row`]: the event the failure becomes, whether
//! reporting it ends the owner, and the context its message is prefixed
//! with. [`report`] reads the owner's row and reports through it: through
//! [`owner::terminal`] for a row that ends the owner, straight to where the
//! owner's reports go otherwise.
//!
//! Two rows are not scenes. The Panic row of each table is a function of its
//! own, so [`report`] cannot be called with it: [`owner::trapped`] reports a
//! panic through it, behind the lifetime's panic latch, and so do the
//! thread-level reports an owner's own task cannot make — `finish_view` and
//! the panic hook on `bobcat-main`, `finish_worker_task`, `report_thread_trap`
//! and the panic hook on `bobcat-workers`. The Disposal row is the MTS
//! table's alone, given by [`main_thread_disposal`], so nothing that takes a
//! [`Scene`] can produce it.
//!
//! # Where each row comes from
//!
//! - [`Scene::Open`]: the realm, its entry, or the engine's own startup code could not be made
//!   ready. MTS: `Page::open_realm` (a runtime that was never built, the realm's construction,
//!   boot's own module), the entry's failed or non-script answer in `load_entry`, and a rejection
//!   of boot's own module seen by the epilogue — boot's own `__FlushElementTree` failing on a
//!   listed sheet included. Worker: a runtime that was never built or a realm that could not be
//!   opened in `Worker::boot`, and a dedicated worker's failed or non-script script answer in
//!   `consume_messages`. Each caller names what failed — the epilogue by the owner's
//!   `BOOT_REJECTION` context — so neither row adds a context. The MTS entry's answer is the one
//!   Open failure not reported through [`report`]: what failed there is the fetcher's own error, a
//!   `LynxViewError` that `load_entry` hands to `StartupFailed` as it is, where the row takes a
//!   script's error and converts it.
//! - [`Scene::Boot`]: the app's startup code threw. MTS: the entry's evaluation, a module it
//!   imports included, as `load_entry` completes it. Worker: the load of the root module in
//!   `Worker::boot`, and a dedicated worker's script as `complete_script` completes it.
//! - [`Scene::Module`]: completing a module an import was waiting for, in the driver's
//!   `load_module`.
//! - [`Scene::Future`]: handing a future its outcome, in the driver's `settle_future`.
//! - [`Scene::Timer`]: a timer callback, run by the driver's epilogue.
//! - [`Scene::Listener`]: an event delivered to the realm. MTS: an input event, a batch of
//!   `<image>` outcomes, and what a worker said, in `consume_worker_events`. Worker: a posted
//!   message.
//! - [`Scene::Frame`]: the realm's animation-frame callbacks, on the painter's vsync.
//! - [`Scene::HostCall`]: a call the host made into the realm. MTS: a page update, and a native
//!   module's answer to a call the realm made. Worker: a native module's answer.
//! - The Disposal row, MTS only: the JavaScript disposal the page runs after the view has ended and
//!   before its realm is released. The view has already ended, so the row does not end it.
//! - The Panic row: the engine panicked while it served the owner.
//!
//! The epilogue reports the rejection of a realm's root module on the MTS
//! alone, as Open — the page's `BOOT_REJECTION` — because the two root
//! modules are different code. The MTS boot module is the engine's: it
//! catches what the entry's evaluation throws, and the runtime's
//! `processData` and render hooks catch what those throw, so what rejects it
//! is `bobcat:runtime`, the document's construction, connecting the BTS, or
//! boot's own flush. A worker's root module is the module at its URL, and a
//! worker's `BOOT_REJECTION` is `None`: only the host holds that load's
//! promise, so the checkpoint that ends the entry the load settled in
//! reports the rejection under that entry's row — Boot for the boot job and
//! the script's completion, Module for a module it imports, Timer for a
//! timer — or drops it with the other leftovers of the one failure that
//! entry reported, and the epilogue reads the load only to learn that it has
//! settled. A BTS's root module, `bobcat:bts`, imports registered modules
//! only, and its entry is imported later, outside the root module. In both
//! tables, Boot is the app's startup code throwing, and Open is something
//! the engine could not make ready.
//!
//! A listed stylesheet has no row of its own. It is settled by the first
//! `__FlushElementTree`: boot's own, whose failure rejects boot and is
//! Open, or one app code called first, whose failure is reported under the
//! row of the entry that call ran in and is then thrown again by boot's own
//! flush.

use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

use super::owner::{self, RealmOwner};
use crate::background::WorkerPayload;
use crate::script::ScriptError;
use crate::threads::{panic_message, platform_script_error};
use crate::view::EngineEvent;

/// Where a failure the engine reports happened, which is what a realm kind's
/// table reads a [`Row`] by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scene {
    /// The realm, its entry or the engine's own startup code could not be
    /// made ready.
    Open,
    /// The app's startup code threw.
    Boot,
    /// Completing a module an import was waiting for.
    Module,
    /// Handing a future its outcome.
    Future,
    /// A timer callback.
    Timer,
    /// Delivering an event to the realm.
    Listener,
    /// Running the realm's animation-frame callbacks.
    Frame,
    /// A call the host made into the realm.
    HostCall,
}

/// One row of a realm kind's table: what a failure is reported as.
pub(crate) struct Row<E> {
    /// The event the failure becomes.
    pub(crate) event: fn(ScriptError) -> E,
    /// Whether reporting it ends the owner.
    pub(crate) ends: bool,
    /// What the failure's message is prefixed with, as `<context>: <message>`.
    /// `None` where the caller has already named what failed.
    pub(crate) context: Option<&'static str>,
}

impl<E> Row<E> {
    /// The event `error` is reported as under this row.
    pub(crate) fn event_for(&self, error: ScriptError) -> E {
        (self.event)(match self.context {
            Some(context) => context_of(context, error),
            None => error,
        })
    }

    /// The event a panic is reported as under this row: the panic's own
    /// message, after the row's context.
    pub(crate) fn event_for_panic(&self, payload: &(dyn Any + Send)) -> E {
        self.event_for(platform_script_error(panic_message(payload).to_owned()))
    }
}

/// Reports `error`, a failure in `scene`, as `owner`'s table says: through
/// [`owner::terminal`], and so once and ending the owner, where the row ends
/// it, and straight to where the owner's reports go otherwise.
pub(crate) fn report<O: RealmOwner>(owner: &Rc<O>, scene: Scene, error: ScriptError) {
    let row = O::row(scene);
    let event = row.event_for(error);
    if row.ends {
        owner::terminal(owner, event);
    } else {
        owner.send(event);
    }
}

/// The MTS table: what a failure in a view's main-thread realm is reported as.
///
/// What could not be made ready fails the view's startup and ends it. Every
/// other failure is the app's: reported, and the view goes on. The Open row's
/// event carries a `LynxViewError`, into which a script's error converts.
pub(crate) fn main_thread(scene: Scene) -> Row<EngineEvent> {
    let (event, ends, context): (fn(ScriptError) -> EngineEvent, _, _) = match scene {
        Scene::Open => (|error| EngineEvent::StartupFailed(error.into()), true, None),
        Scene::Boot | Scene::Frame | Scene::HostCall => (EngineEvent::ScriptRunError, false, None),
        Scene::Module => (
            EngineEvent::ScriptRunError,
            false,
            Some("loading an imported module"),
        ),
        Scene::Future => (
            EngineEvent::ScriptRunError,
            false,
            Some("settling a future"),
        ),
        Scene::Timer => (EngineEvent::TimerFailed, false, None),
        Scene::Listener => (EngineEvent::ListenerFailed, false, None),
    };
    Row {
        event,
        ends,
        context,
    }
}

/// The MTS table's Disposal row: a failure of the JavaScript disposal the
/// page runs once the view has ended, which is a listener's failure. It ends
/// nothing, because the view it would end is already over.
pub(crate) fn main_thread_disposal() -> Row<EngineEvent> {
    Row {
        event: EngineEvent::ListenerFailed,
        ends: false,
        context: None,
    }
}

/// The MTS table's Panic row: the engine panicked while it served the view,
/// which ends it.
///
/// Under `panic = "abort"` the panic hook reports the panic with this row's
/// event and a message of its own, worded from the hook's detail, because
/// nothing unwinds and there is no payload to read.
pub(crate) fn main_thread_panic() -> Row<EngineEvent> {
    Row {
        event: EngineEvent::from_panic,
        ends: true,
        context: Some("the Lynx main thread panicked"),
    }
}

/// The worker table: what a failure in a worker's realm, the BTS's included,
/// is reported as, to the realm that created it.
///
/// A worker whose realm or script could not be made ready is over, which is
/// `Failed`. Anything its realm threw after that is `Errored`: HTML reports an
/// uncaught exception at the worker and then at its parent and leaves both
/// running.
pub(crate) fn worker(scene: Scene) -> Row<WorkerPayload> {
    let context = match scene {
        Scene::Open => {
            return Row {
                event: WorkerPayload::Failed,
                ends: true,
                context: None,
            };
        }
        Scene::Boot => "running the worker's script",
        Scene::Module => "loading an imported worker module",
        Scene::Future => "settling a worker's future",
        Scene::Timer => "running a worker's timer callback",
        Scene::Listener => "delivering a message to a worker",
        Scene::Frame => "running animation callbacks",
        Scene::HostCall => "running a native module callback",
    };
    Row {
        event: WorkerPayload::Errored,
        ends: false,
        context: Some(context),
    }
}

/// The worker table's Panic row: a worker nothing will be heard from again,
/// which is `Failed`.
///
/// Under `panic = "abort"` the panic hook reports the thread's panic with
/// this row's event and a message of its own, as [`main_thread_panic`] says.
pub(crate) fn worker_panic() -> Row<WorkerPayload> {
    Row {
        event: WorkerPayload::Failed,
        ends: true,
        context: Some("the worker thread panicked"),
    }
}

/// Prefixes a failure with what the host was doing, in the format
/// `MainThreadError` gives the errors that reach an embedder through a view.
pub(crate) fn context_of(context: &str, mut error: ScriptError) -> ScriptError {
    error.message = Arc::from(format!("{context}: {}", error.message));
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::LynxViewError;

    const MESSAGE: &str = "the script threw";

    fn error() -> ScriptError {
        platform_script_error(MESSAGE.to_owned())
    }

    /// What one row is expected to be: its name, the row, whether an event is
    /// the variant it reports as, whether it ends the owner, and its context.
    type Expected<E> = (
        &'static str,
        Row<E>,
        fn(&E) -> bool,
        bool,
        Option<&'static str>,
    );

    /// Every row of the MTS table, as it is expected to be.
    fn main_thread_rows() -> [Expected<EngineEvent>; 10] {
        let startup_failed: fn(&EngineEvent) -> bool =
            |event| matches!(event, EngineEvent::StartupFailed(LynxViewError::Script(_)));
        let script_run_error: fn(&EngineEvent) -> bool =
            |event| matches!(event, EngineEvent::ScriptRunError(_));
        let timer_failed: fn(&EngineEvent) -> bool =
            |event| matches!(event, EngineEvent::TimerFailed(_));
        let listener_failed: fn(&EngineEvent) -> bool =
            |event| matches!(event, EngineEvent::ListenerFailed(_));
        let panicked: fn(&EngineEvent) -> bool = |event| matches!(event, EngineEvent::Panicked(_));
        [
            ("Open", main_thread(Scene::Open), startup_failed, true, None),
            (
                "Boot",
                main_thread(Scene::Boot),
                script_run_error,
                false,
                None,
            ),
            (
                "Module",
                main_thread(Scene::Module),
                script_run_error,
                false,
                Some("loading an imported module"),
            ),
            (
                "Future",
                main_thread(Scene::Future),
                script_run_error,
                false,
                Some("settling a future"),
            ),
            (
                "Timer",
                main_thread(Scene::Timer),
                timer_failed,
                false,
                None,
            ),
            (
                "Listener",
                main_thread(Scene::Listener),
                listener_failed,
                false,
                None,
            ),
            (
                "Frame",
                main_thread(Scene::Frame),
                script_run_error,
                false,
                None,
            ),
            (
                "HostCall",
                main_thread(Scene::HostCall),
                script_run_error,
                false,
                None,
            ),
            (
                "Disposal",
                main_thread_disposal(),
                listener_failed,
                false,
                None,
            ),
            (
                "Panic",
                main_thread_panic(),
                panicked,
                true,
                Some("the Lynx main thread panicked"),
            ),
        ]
    }

    /// Every row of the worker table, as it is expected to be.
    fn worker_rows() -> [Expected<WorkerPayload>; 9] {
        let failed: fn(&WorkerPayload) -> bool = |event| matches!(event, WorkerPayload::Failed(_));
        let errored: fn(&WorkerPayload) -> bool =
            |event| matches!(event, WorkerPayload::Errored(_));
        [
            ("Open", worker(Scene::Open), failed, true, None),
            (
                "Boot",
                worker(Scene::Boot),
                errored,
                false,
                Some("running the worker's script"),
            ),
            (
                "Module",
                worker(Scene::Module),
                errored,
                false,
                Some("loading an imported worker module"),
            ),
            (
                "Future",
                worker(Scene::Future),
                errored,
                false,
                Some("settling a worker's future"),
            ),
            (
                "Timer",
                worker(Scene::Timer),
                errored,
                false,
                Some("running a worker's timer callback"),
            ),
            (
                "Listener",
                worker(Scene::Listener),
                errored,
                false,
                Some("delivering a message to a worker"),
            ),
            (
                "Frame",
                worker(Scene::Frame),
                errored,
                false,
                Some("running animation callbacks"),
            ),
            (
                "HostCall",
                worker(Scene::HostCall),
                errored,
                false,
                Some("running a native module callback"),
            ),
            (
                "Panic",
                worker_panic(),
                failed,
                true,
                Some("the worker thread panicked"),
            ),
        ]
    }

    /// The message a report carries, which is the context and then the
    /// failure's own message, or the failure's own alone.
    fn expected_message(context: Option<&str>) -> String {
        context.map_or_else(
            || MESSAGE.to_owned(),
            |context| format!("{context}: {MESSAGE}"),
        )
    }

    fn engine_event_message(event: &EngineEvent) -> String {
        match event {
            EngineEvent::StartupFailed(LynxViewError::Script(error))
            | EngineEvent::ScriptRunError(error)
            | EngineEvent::TimerFailed(error)
            | EngineEvent::ListenerFailed(error)
            | EngineEvent::Panicked(error) => error.message.to_string(),
            _ => panic!("no row reports {event:?}"),
        }
    }

    #[test]
    fn every_row_of_the_main_thread_table() {
        for (name, row, is, ends, context) in main_thread_rows() {
            let event = row.event_for(error());
            assert!(is(&event), "the {name} row reports {event:?}");
            assert_eq!(row.ends, ends, "whether the {name} row ends the view");
            assert_eq!(row.context, context, "the {name} row's context");
            assert_eq!(
                engine_event_message(&event),
                expected_message(context),
                "the {name} row's message"
            );
        }
    }

    /// `LynxView::pump` ends a view on exactly the events `is_fatal` names,
    /// and the page ends its own side on exactly the rows that end it: the
    /// two must name the same rows.
    #[test]
    fn a_main_thread_row_ends_the_view_exactly_when_its_event_is_fatal() {
        for (name, row, ..) in main_thread_rows() {
            assert_eq!(
                row.event_for(error()).is_fatal(),
                row.ends,
                "the {name} row's event is fatal exactly when the row ends the view"
            );
        }
    }

    #[test]
    fn every_row_of_the_worker_table() {
        for (name, row, is, ends, context) in worker_rows() {
            let event = row.event_for(error());
            assert!(is(&event), "the {name} row reports the payload it names");
            assert_eq!(row.ends, ends, "whether the {name} row ends the worker");
            assert_eq!(row.context, context, "the {name} row's context");
            let (WorkerPayload::Errored(error) | WorkerPayload::Failed(error)) = event else {
                panic!("the {name} row reports a failure");
            };
            assert_eq!(
                &*error.message,
                expected_message(context),
                "the {name} row's message"
            );
        }
    }

    /// A panic reads as the Panic row's context and then what the panic said,
    /// on both threads.
    #[test]
    fn a_panic_is_worded_by_its_row() {
        let EngineEvent::Panicked(error) = main_thread_panic().event_for_panic(&"boom") else {
            panic!("a main-thread panic is Panicked");
        };
        assert_eq!(&*error.message, "the Lynx main thread panicked: boom");
        let WorkerPayload::Failed(error) = worker_panic().event_for_panic(&String::from("boom"))
        else {
            panic!("a worker panic is Failed");
        };
        assert_eq!(&*error.message, "the worker thread panicked: boom");
    }
}
