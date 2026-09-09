//! The main realm's Worker handle: name a context, ask the painter for its
//! source, and forward commands. Contexts and pending messages live on workers.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use quickjs_rust_bridge::HostValue;

use super::quickjs::{ScriptEngine, ScriptRuntime};
use super::{StartupControl, ToPainterSender};
use crate::background::{WorkerCommand, WorkerKey, WorkerStart};
use crate::esm::HOST_MODULE_SPECIFIER;
use crate::mailbox::Sender;
use crate::resource::{SourceCompletion, SourceRequest};
use crate::script::ScriptError;
use crate::view::{EventRequester, ToPainter, ViewId};

pub(super) const MODULE: &str = "bobcat-internal";
pub(super) const SOURCE: &str = include_str!("../../../../packages/bobcat-element/src/worker.mjs");

/// Issued on bobcat-main, once per group. No cross-thread allocator or lock.
#[derive(Clone)]
pub(super) struct WorkerFactory {
    commands: Sender<WorkerCommand>,
    next: Rc<Cell<u64>>,
}

impl WorkerFactory {
    pub(super) fn new(commands: Sender<WorkerCommand>) -> Self {
        Self {
            commands,
            next: Rc::new(Cell::new(1)),
        }
    }

    pub(super) fn install<R: EventRequester>(
        &self,
        engine: &mut ScriptEngine,
        runtime: &mut ScriptRuntime,
        notify: ToPainterSender<R>,
        base_url: &str,
        control: Arc<StartupControl>,
    ) -> Result<(), ScriptError> {
        // The native functions hold the owner until the realm is dropped,
        // including when entry boot fails after it constructed workers.
        let owner = Rc::new(WorkerOwner {
            factory: self.clone(),
            view: notify.view,
            control,
        });
        let creator = Rc::clone(&owner);
        let base_url = base_url.to_owned();
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "createWorker",
            2,
            Box::new(move |arguments| {
                let specifier = string(arguments, 0)?.to_owned();
                let name = string(arguments, 1)?.to_owned();
                let id = creator.factory.next.get();
                creator
                    .factory
                    .next
                    .set(id.checked_add(1).ok_or("worker ids exhausted")?);
                let key = WorkerKey::new(id);
                creator.send(WorkerCommand::Start(WorkerStart {
                    key,
                    view: creator.view,
                    name,
                }))?;
                // Start is enqueued before the painter can possibly answer.
                // The completion is weak: outstanding IO must not keep the
                // group's worker thread alive while its owner joins it.
                notify.send(ToPainter::RequestWorkerSource {
                    request: SourceRequest::Worker {
                        specifier,
                        base_url: base_url.clone(),
                    },
                    completion: SourceCompletion::worker(
                        creator.factory.commands.downgrade(),
                        key,
                        creator.view,
                        Arc::clone(&creator.control),
                    ),
                });
                Ok(HostValue::String(id.to_string()))
            }),
        )?;
        let sender = Rc::clone(&owner);
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "sendWorkerMessage",
            2,
            Box::new(move |arguments| {
                sender.send(WorkerCommand::Message {
                    key: key(arguments)?,
                    data: string(arguments, 1)?.to_owned(),
                })?;
                Ok(HostValue::Undefined)
            }),
        )?;
        engine.register_host_module_function(
            runtime,
            HOST_MODULE_SPECIFIER,
            "terminateWorker",
            1,
            Box::new(move |arguments| {
                owner.send(WorkerCommand::Terminate {
                    key: key(arguments)?,
                })?;
                Ok(HostValue::Undefined)
            }),
        )
    }
}

struct WorkerOwner {
    factory: WorkerFactory,
    view: ViewId,
    control: Arc<StartupControl>,
}

impl WorkerOwner {
    fn send(&self, command: WorkerCommand) -> Result<(), String> {
        self.factory
            .commands
            .send((None, command))
            .map_err(|_| "the worker thread has ended".to_owned())
    }
}

impl Drop for WorkerOwner {
    fn drop(&mut self) {
        self.control.cancel();
        let _ = self.send(WorkerCommand::ReleaseView(self.view));
    }
}

fn string(arguments: &[HostValue], index: usize) -> Result<&str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!(
            "Worker host operation expects string argument {index}"
        )),
    }
}

fn key(arguments: &[HostValue]) -> Result<WorkerKey, String> {
    string(arguments, 0)?
        .parse()
        .map(WorkerKey::new)
        .map_err(|_| "invalid worker key".to_owned())
}
