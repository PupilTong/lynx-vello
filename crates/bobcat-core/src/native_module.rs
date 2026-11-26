//! Host-injected `NativeModules`, for whichever realm calls them.
//!
//! A Lynx application reaches host capabilities through
//! `NativeModules.<name>.<method>(...)`, and this is the whole of what that
//! call is: the calling realm hands the host a method name, the arguments as
//! JSON text, and one [`ModuleCallback`] per function argument; the embedder's
//! module answers by invoking a callback, whenever it likes and from wherever
//! it likes.
//!
//! The shape is native Lynx's, not a promise's. A method returns `undefined`
//! in JavaScript — there is no synchronous result to return, because the
//! module runs on the embedder's thread and the caller is on another one —
//! and every function argument becomes a single-shot callback the way
//! native's `ModuleCallback` does. A callback that is never invoked and never
//! dropped simply keeps its JavaScript function alive, as native's does.
//!
//! Every realm kind declares the host module this installs,
//! `bobcat-internal:native-modules`, and every realm kind can be answered: a
//! callback goes back to the view's MTS realm through its command FIFO and to
//! a worker realm through that worker's inbox (`ModuleReply`). Which modules
//! a `NativeModules` object names is data: the MTS realm reads the view's
//! table as one of its startup members and posts it to its BTS in the
//! `initialize` message. A plain `Worker` is posted no table, and the MTS
//! realm has no `NativeModules`.
//!
//! A module may also supply one ES module, as bytecode
//! ([`NativeModule::esm`], compiled by [`compile_native_module_esm`]): the
//! module `lynx:<name>`, which a worker realm of the view imports. The view
//! keeps the bytecode in one shared table ([`NativeModuleEsms`]) that travels
//! with its host outbox to every worker realm it opens; a worker realm whose
//! import of `lynx:<name>` is unanswered is completed from it on the worker
//! thread, and nothing is asked of the embedder or the fetcher. The module
//! table tells the BTS which modules have one, and which of those it imports
//! before the view's BTS entry.

use std::sync::Arc;

use bytes::Bytes;
use quickjs_rust_bridge::{HostArgument, HostValue};
use tokio::sync::mpsc;

use crate::background::{WorkerKey, WorkerMessage};
use crate::esm::{Export, NATIVE_MODULES_HOST_SPECIFIER};
use crate::link::{HostOutbox, ToMain, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::performance::PipelineRegistry;
use crate::script::ScriptError;

pub trait NativeModule {
    fn name(&self) -> &str;

    fn methods(&self) -> Vec<String>;

    fn invoke(&self, call: ModuleCall);

    fn esm(&self) -> Option<NativeModuleEsm> {
        None
    }
}

#[derive(Clone, Debug)]
pub struct NativeModuleEsm {
    pub bytecode: Bytes,
    pub import_before_entry: bool,
}

pub fn compile_native_module_esm(name: &str, source: &str) -> Result<Vec<u8>, ScriptError> {
    crate::main::quickjs::compile_module_framed(&native_module_esm_specifier(name), source)
}

pub(crate) fn native_module_esm_specifier(name: &str) -> String {
    format!("{}{name}", crate::esm::NATIVE_MODULE_ESM_PREFIX)
}

#[derive(Debug, Default)]
pub(crate) struct NativeModuleEsms(Vec<(String, Bytes)>);

impl NativeModuleEsms {
    pub(crate) const fn new(modules: Vec<(String, Bytes)>) -> Self {
        Self(modules)
    }

    pub(crate) fn get(&self, specifier: &str) -> Option<&Bytes> {
        self.0
            .iter()
            .find_map(|(name, bytecode)| (name == specifier).then_some(bytecode))
    }
}

#[derive(Debug)]
pub struct ModuleCall {
    pub method: String,
    pub arguments: String,
    pub callbacks: Vec<ModuleCallback>,
}

impl ModuleCall {
    pub(crate) fn assemble(
        call: u64,
        method: String,
        arguments: String,
        callbacks: &[u32],
        reply: &ModuleReply,
    ) -> Self {
        Self {
            method,
            arguments,
            callbacks: callbacks
                .iter()
                .map(|&index| ModuleCallback::new(call, index, reply.clone()))
                .collect(),
        }
    }
}

pub struct ModuleCallback {
    call: u64,
    index: u32,
    reply: ModuleReply,
    arguments: Option<String>,
}

impl std::fmt::Debug for ModuleCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModuleCallback")
            .field("argument_index", &self.argument_index())
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl ModuleCallback {
    pub(crate) const fn new(call: u64, index: u32, reply: ModuleReply) -> Self {
        Self {
            call,
            index,
            reply,
            arguments: None,
        }
    }

    #[must_use]
    pub const fn argument_index(&self) -> usize {
        self.index as usize
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.reply.is_closed()
    }

    pub fn invoke(mut self, arguments: String) {
        self.arguments = Some(arguments);
    }
}

impl Drop for ModuleCallback {
    fn drop(&mut self) {
        self.reply
            .send(self.call, self.index, self.arguments.take());
    }
}

#[derive(Clone)]
pub(crate) enum ModuleReply {
    Main(mpsc::WeakUnboundedSender<ToMain>),
    Worker(
        mpsc::WeakUnboundedSender<WorkerMessage>,
        Option<Arc<PipelineRegistry>>,
    ),
}

impl ModuleReply {
    fn send(&self, call: u64, index: u32, arguments: Option<String>) {
        match self {
            Self::Main(commands) => {
                if let Some(commands) = commands.upgrade() {
                    let _ = commands.send(ToMain::ModuleCallback {
                        call,
                        index,
                        arguments,
                    });
                }
            }
            Self::Worker(messages, registry) => {
                if let Some(messages) = messages.upgrade() {
                    let _ = messages.send(WorkerMessage::ModuleCallback {
                        call,
                        index,
                        arguments,
                        enqueued: registry
                            .as_deref()
                            .and_then(PipelineRegistry::js_blocking_stamp),
                    });
                }
            }
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Self::Main(commands) => commands.upgrade().is_none_or(|sender| sender.is_closed()),
            Self::Worker(messages, _) => messages.upgrade().is_none_or(|sender| sender.is_closed()),
        }
    }
}

pub(crate) fn install(
    engine: &mut ScriptEngine,
    host: &HostOutbox,
    caller: Option<WorkerKey>,
) -> Result<(), ScriptError> {
    const NAME: &str = "bobcat-internal:native-modules.invokeNativeModule";
    let host = host.clone();
    engine.register_host_module_function(
        NATIVE_MODULES_HOST_SPECIFIER,
        "invokeNativeModule",
        5,
        move |arguments| {
            let call = call_id(arguments)?;
            let module = string(arguments, 1)?.to_owned();
            let method = string(arguments, 2)?.to_owned();
            let call_arguments = string(arguments, 3)?.to_owned();
            let callbacks = string(arguments, 4)?
                .split(',')
                .filter(|index| !index.is_empty())
                .map(|index| {
                    index
                        .parse()
                        .map_err(|_| format!("{NAME} expects argument indices for argument 4"))
                })
                .collect::<Result<Vec<u32>, _>>()?;
            host.notify(ViewNotice::NativeModuleCall {
                caller,
                call,
                module,
                method,
                arguments: call_arguments,
                callbacks,
            });
            Ok(HostValue::Undefined)
        },
    )
}

pub(crate) fn deliver(
    engine: &ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    call: u64,
    index: u32,
    arguments: Option<&str>,
) -> Result<(), ScriptError> {
    #[allow(
        clippy::cast_precision_loss,
        reason = "the realm mints these counting up from one"
    )]
    let call = call as f64;
    engine
        .call_module_export(
            js_runtime,
            Export::NativeModuleCallback,
            &[
                HostArgument::Number(call),
                HostArgument::Number(f64::from(index)),
                arguments.map_or(HostArgument::Undefined, HostArgument::String),
            ],
        )
        .map(|_| ())
}

fn string<'a>(arguments: &[HostArgument<'a>], index: usize) -> Result<&'a str, String> {
    match arguments.get(index) {
        Some(&HostArgument::String(value)) => Ok(value),
        _ => Err(format!(
            "bobcat-internal:native-modules.invokeNativeModule expects string argument {index}"
        )),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the checks below leave a whole, representable call number"
)]
fn call_id(arguments: &[HostArgument<'_>]) -> Result<u64, String> {
    match arguments.first() {
        Some(&HostArgument::Number(value))
            if value.is_finite() && value >= 0.0 && value.fract() == 0.0 =>
        {
            Ok(value as u64)
        }
        _ => Err(
            "bobcat-internal:native-modules.invokeNativeModule expects a call number for argument 0"
                .to_owned(),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EsmLoad {
    None,
    OnImport,
    BeforeEntry,
}

impl EsmLoad {
    pub(crate) fn of(esm: Option<&NativeModuleEsm>) -> Self {
        match esm {
            None => Self::None,
            Some(esm) if esm.import_before_entry => Self::BeforeEntry,
            Some(_) => Self::OnImport,
        }
    }

    const fn field(self) -> &'static str {
        match self {
            Self::None => "",
            Self::OnImport => "esm",
            Self::BeforeEntry => "esm-before-entry",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TableEntry {
    pub(crate) name: String,
    pub(crate) methods: Vec<String>,
    pub(crate) esm: EsmLoad,
}

impl TableEntry {
    pub(crate) fn new(name: impl Into<String>, methods: Vec<String>) -> Self {
        Self {
            name: name.into(),
            methods,
            esm: EsmLoad::None,
        }
    }
}

pub(crate) type NativeModuleTable = Vec<TableEntry>;

pub(crate) fn encode_table(modules: &NativeModuleTable) -> String {
    let mut record = String::new();
    for module in modules {
        crate::main::record::write_record_field(&mut record, &module.name);
        crate::main::record::write_record_field(&mut record, &module.methods.join(","));
        crate::main::record::write_record_field(&mut record, module.esm.field());
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{CommandSender, FrameDemand};

    #[test]
    fn a_module_with_no_methods_and_no_esm_still_writes_its_three_fields() {
        let table: NativeModuleTable = vec![
            TableEntry::new("Echo", vec!["ping".to_owned(), "pong".to_owned()]),
            TableEntry::new("Bare", Vec::new()),
        ];
        assert_eq!(encode_table(&table), "4:Echo9:ping,pong0:4:Bare0:0:");
    }

    #[test]
    fn the_third_field_names_each_modules_es_module_load() {
        let esm = |import_before_entry| NativeModuleEsm {
            bytecode: Bytes::from_static(b"unread here"),
            import_before_entry,
        };
        let table: NativeModuleTable = vec![
            TableEntry {
                esm: EsmLoad::of(Some(&esm(true))),
                ..TableEntry::new("First", vec!["a".to_owned()])
            },
            TableEntry {
                esm: EsmLoad::of(Some(&esm(false))),
                ..TableEntry::new("Lazy", Vec::new())
            },
            TableEntry {
                esm: EsmLoad::of(None),
                ..TableEntry::new("Plain", Vec::new())
            },
            TableEntry {
                esm: EsmLoad::of(Some(&esm(true))),
                ..TableEntry::new("Ünï", Vec::new())
            },
        ];
        assert_eq!(
            encode_table(&table),
            "5:First1:a16:esm-before-entry4:Lazy0:3:esm5:Plain0:0:3:Ünï0:16:esm-before-entry"
        );
    }

    #[test]
    fn the_view_table_answers_by_specifier() {
        let esms = NativeModuleEsms::new(vec![(
            native_module_esm_specifier("Echo"),
            Bytes::from_static(b"bytes"),
        )]);
        assert_eq!(
            esms.get("lynx:Echo").map(Bytes::as_ref),
            Some(&b"bytes"[..])
        );
        assert!(esms.get("Echo").is_none());
        assert!(esms.get("lynx:Other").is_none());
    }

    #[test]
    fn a_compiled_esm_is_framed_and_a_syntax_error_is_refused() {
        let bytecode = compile_native_module_esm("Echo", "export const x = 1;").unwrap();
        assert!(quickjs_rust_bridge::is_framed_bytecode(&bytecode));
        let error = compile_native_module_esm("Echo", "export let let = 1;").unwrap_err();
        assert_eq!(
            error.kind,
            crate::script::ScriptErrorKind::Syntax,
            "{error}"
        );
    }

    #[test]
    fn a_released_callback_sends_no_arguments_and_an_invoked_one_sends_its_own() {
        let (sender, mut inbox) = mpsc::unbounded_channel();
        let call = ModuleCall::assemble(
            7,
            "ping".to_owned(),
            "[null,null]".to_owned(),
            &[2, 3],
            &ModuleReply::Worker(sender.downgrade(), None),
        );
        assert_eq!(
            call.callbacks
                .iter()
                .map(ModuleCallback::argument_index)
                .collect::<Vec<_>>(),
            [2, 3],
            "one callback per function argument, in argument order"
        );
        let [released, invoked] =
            <[_; 2]>::try_from(call.callbacks).expect("two function arguments");
        drop(released);
        invoked.invoke("[1]".to_owned());
        let sent: Vec<_> = std::iter::from_fn(|| inbox.try_recv().ok())
            .map(|message| match message {
                WorkerMessage::ModuleCallback {
                    call,
                    index,
                    arguments,
                    ..
                } => (call, index, arguments),
                _ => panic!("a module callback is the only message here"),
            })
            .collect();
        assert_eq!(
            sent,
            [(7, 2, None), (7, 3, Some("[1]".to_owned()))],
            "a release carries no arguments and an invocation carries its own"
        );

        let live = ModuleCallback::new(7, 4, ModuleReply::Worker(sender.downgrade(), None));
        assert!(!live.is_cancelled());
        drop(inbox);
        assert!(live.is_cancelled());
    }

    #[test]
    fn a_call_the_main_thread_made_is_answered_through_the_view_commands() {
        let (commands, mut incoming) = mpsc::unbounded_channel();
        let call = ModuleCall::assemble(
            3,
            "ping".to_owned(),
            "[null]".to_owned(),
            &[0],
            &ModuleReply::Main(commands.downgrade()),
        );
        let [callback] = <[_; 1]>::try_from(call.callbacks).expect("one function argument");
        assert!(!callback.is_cancelled());
        callback.invoke(r#"["pong"]"#.to_owned());
        let Ok(ToMain::ModuleCallback {
            call,
            index,
            arguments,
        }) = incoming.try_recv()
        else {
            panic!("the answer is a module callback command");
        };
        assert_eq!(
            (call, index, arguments),
            (3, 0, Some(r#"["pong"]"#.to_owned()))
        );

        let live = ModuleCallback::new(3, 1, ModuleReply::Main(commands.downgrade()));
        assert!(!live.is_cancelled());
        drop(incoming);
        assert!(
            live.is_cancelled(),
            "the view's task dropped the receiving end"
        );
    }

    #[test]
    fn an_answer_to_the_main_thread_is_sent_outside_the_command_count() {
        let (commands, mut incoming) = mpsc::unbounded_channel();
        let sender = CommandSender::new(commands);
        let reply = FrameDemand::default()
            .reply(None, &sender)
            .expect("a call the MTS realm made always has a reply handle");
        let call = ModuleCall::assemble(
            5,
            "ping".to_owned(),
            "[null,null]".to_owned(),
            &[0, 1],
            &reply,
        );
        let [invoked, released] =
            <[_; 2]>::try_from(call.callbacks).expect("two function arguments");
        invoked.invoke("[]".to_owned());
        drop(released);
        let answers: Vec<_> = std::iter::from_fn(|| incoming.try_recv().ok())
            .map(|command| match command {
                ToMain::ModuleCallback {
                    call,
                    index,
                    arguments,
                } => (call, index, arguments),
                _ => panic!("a module callback is the only command here"),
            })
            .collect();
        assert_eq!(
            answers,
            [(5, 0, Some("[]".to_owned())), (5, 1, None)],
            "both answers went into the view's command channel"
        );
        assert_eq!(
            sender.sent(),
            0,
            "neither answer is in the count a frame post takes its fence from"
        );

        sender
            .send(ToMain::Posted)
            .expect("the receiving end is held");
        assert_eq!(
            sender.sent(),
            1,
            "a command sent through the sender itself is counted"
        );
    }
}
