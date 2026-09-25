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
//! a realm is told about is data: the BTS is handed the view's table, and the
//! MTS realm and a plain `Worker` an empty one.

use quickjs_rust_bridge::{HostArgument, HostValue};
use tokio::sync::mpsc;

use crate::background::{WorkerKey, WorkerMessage};
use crate::esm::{
    NATIVE_MODULE_CALLBACK_EXPORT, NATIVE_MODULES_HOST_SPECIFIER, NATIVE_MODULES_MODULE_SPECIFIER,
};
use crate::link::{HostOutbox, ToMain, ViewNotice};
use crate::main::quickjs::{ScriptEngine, ScriptRuntime};
use crate::script::ScriptError;

/// One embedder-owned module, reachable as `NativeModules.<name>` in the BTS
/// realm, and through `bobcat:native-modules` in any realm of its view.
///
/// No `Send + Sync` supertrait, and none is wanted: the boxes live on the
/// [`LynxView`](crate::LynxView), on the thread that created it, and
/// [`Self::invoke`] is called only from inside
/// [`LynxView::pump`](crate::LynxView::pump) — the same place and the same
/// thread the host's [`ResourceFetcher`](crate::resource::ResourceFetcher) is
/// serviced on. A Wasm module may therefore hold `JsValue`s, and a native one
/// may hold `Rc`s.
///
/// The modules a view has are named once, at
/// [`LynxGroup::create_lynx_view`](crate::LynxGroup::create_lynx_view), and
/// never change: two modules with one name is a construction error, and
/// nothing adds or removes one afterwards.
pub trait NativeModule {
    /// The `NativeModules.<name>` key this module answers to.
    fn name(&self) -> &str;

    /// The method names JavaScript may call, read once when the view is
    /// built — [`CustomElement::observed_attributes`]'s shape, for the same
    /// reason: the declared surface crosses to the realm as data, so nothing
    /// asks the module a question while script is running.
    ///
    /// Only these exist on the JavaScript object. Anything else is
    /// `undefined`, which is the answer native gives for a method a module
    /// does not carry. A call naming any other method — which only a script
    /// calling `bobcat:native-modules` directly can make — never reaches
    /// [`Self::invoke`]: [`LynxView::pump`](crate::LynxView::pump) drops it,
    /// which releases its callbacks.
    ///
    /// [`CustomElement::observed_attributes`]: dom::CustomElement::observed_attributes
    fn methods(&self) -> Vec<String>;

    /// One `NativeModules.<name>.<method>(...)` call, on the embedder's
    /// thread inside [`LynxView::pump`](crate::LynxView::pump).
    ///
    /// Returning answers nothing: the call's [`ModuleCallback`]s are what
    /// speak back, and they may be kept for as long as the work takes. A call
    /// simply dropped releases them all uninvoked.
    fn invoke(&self, call: ModuleCall);
}

/// One `NativeModules.<module>.<method>(...)` call, as the module sees it.
///
/// `Send`: what the calling realm said travels from its engine thread to the
/// embedder's thread inside a view notice, where the call is built, and an
/// embedder may hand it on to a worker thread of its own and answer from
/// there.
#[derive(Debug)]
pub struct ModuleCall {
    /// The method JavaScript named. Always one of the names
    /// [`NativeModule::methods`] declared.
    pub method: String,
    /// The arguments, in order, as JSON array text — the whole of what the
    /// realm sent, unread by Rust, which is this engine's rule for anything
    /// only JavaScript owns. Each function argument is `null` here and
    /// appears in [`Self::callbacks`] instead.
    pub arguments: String,
    /// The function arguments, in argument order. Each knows which argument
    /// it was ([`ModuleCallback::argument_index`]), so a module with two
    /// function parameters — native's usual success/failure pair — can tell
    /// them apart without counting.
    pub callbacks: Vec<ModuleCallback>,
}

impl ModuleCall {
    /// Builds one call out of what the realm said and the handle on the
    /// channel that answers it.
    ///
    /// The two halves meet here rather than in the realm: what crosses from
    /// the calling realm's engine thread is the call's text and the indices
    /// of its function arguments, and the reply handle is one the view
    /// already holds — its own command sender for the MTS realm, the handle
    /// it registered from `WorkerCreated` for a worker. `LynxView::pump` is
    /// where both are in hand, and this is what it assembles them with.
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

/// The single-shot right to call one JavaScript function argument back.
///
/// Native's `ModuleCallback`, with its one-invocation rule made a property of
/// the type rather than a flag somebody has to check: [`Self::invoke`]
/// consumes the handle, so a second call cannot be written. Dropping it
/// without invoking releases the JavaScript function, which is what a module
/// that decided not to answer owes the realm — there is no error channel
/// here, exactly as there is none in native.
///
/// The invocation itself is a message: it is sent to the calling realm when
/// the handle drops, and runs there on that realm's own turn. That is why
/// this is `Send` and why invoking from any thread is fine.
pub struct ModuleCallback {
    /// The call this function argument belonged to, as the realm numbered it.
    call: u64,
    /// Which argument of that call it was.
    index: u32,
    /// The channel into the calling realm, weakly: a callback a module never
    /// gets round to must not keep a realm alive. It is also the whole of
    /// what [`Self::is_cancelled`] reads — the receiving end drops with the
    /// task that serves the realm, so a handle that no longer upgrades *is*
    /// the realm being gone.
    reply: ModuleReply,
    /// What [`Self::invoke`] left for [`Drop`] to send. `None` is a release.
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
    /// One function argument's right of reply, minted in the realm that made
    /// the call.
    pub(crate) const fn new(call: u64, index: u32, reply: ModuleReply) -> Self {
        Self {
            call,
            index,
            reply,
            arguments: None,
        }
    }

    /// Which argument of the call this function was, counting from zero.
    #[must_use]
    pub const fn argument_index(&self) -> usize {
        self.index as usize
    }

    /// Whether the realm that made the call is gone — the worker ended, or
    /// the view that owned it was released or failed.
    ///
    /// Read off the reply handle itself, which is the same fact: the
    /// receiving end drops with the task that serves the realm, so there is
    /// nothing left to answer exactly when there is nothing left to answer
    /// *through*.
    ///
    /// Invoking afterwards is a silent no-op rather than an error, the way a
    /// post to a terminated worker is, so this is an optimization for a module
    /// about to do expensive work rather than a check it has to make.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.reply.is_closed()
    }

    /// Calls the JavaScript function with `arguments`, JSON array text the
    /// realm parses and spreads.
    ///
    /// Consumes the handle, because a callback is invoked at most once. The
    /// call itself happens on the calling realm's engine thread, on that
    /// realm's next turn.
    pub fn invoke(mut self, arguments: String) {
        self.arguments = Some(arguments);
    }
}

/// Where the invocation is actually sent, so that invoking and releasing take
/// one path and a callback dropped in a panic still releases its function.
impl Drop for ModuleCallback {
    fn drop(&mut self) {
        self.reply
            .send(self.call, self.index, self.arguments.take());
    }
}

/// The channel one call is answered through: the calling realm's own, held
/// weakly.
///
/// One variant per realm kind that can call, because the two are served by
/// different tasks on different threads: the view's MTS realm reads the
/// view's command FIFO on `bobcat-main`, and a worker realm reads its own
/// inbox on `bobcat-workers`. The answer is the same three fields either way.
#[derive(Clone)]
pub(crate) enum ModuleReply {
    /// The view's command sender, for a call its MTS realm made.
    Main(mpsc::WeakUnboundedSender<ToMain>),
    /// The calling worker's inbox, as the view registered it from
    /// `WorkerCreated`.
    Worker(mpsc::WeakUnboundedSender<WorkerMessage>),
}

impl ModuleReply {
    /// Sends one function argument's answer, or its release when `arguments`
    /// is `None`. A realm that is gone receives nothing, and nothing here
    /// reports that.
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
            Self::Worker(messages) => {
                if let Some(messages) = messages.upgrade() {
                    let _ = messages.send(WorkerMessage::ModuleCallback {
                        call,
                        index,
                        arguments,
                    });
                }
            }
        }
    }

    /// Whether the channel no longer reaches a realm: every strong sender is
    /// gone, or the receiving end was dropped with its task.
    fn is_closed(&self) -> bool {
        match self {
            Self::Main(commands) => commands.upgrade().is_none_or(|sender| sender.is_closed()),
            Self::Worker(messages) => messages.upgrade().is_none_or(|sender| sender.is_closed()),
        }
    }
}

/// Installs a realm's `bobcat-internal:native-modules`: `invokeNativeModule`,
/// the one member a call reaches the embedder through, and
/// `nativeModuleTable`, which hands `table` over once and keeps nothing.
///
/// Every realm kind installs both (the MTS realm, the BTS and a plain
/// `Worker`), so `bobcat:native-modules` links in each. `caller` is what a
/// call names its realm by, and so which channel [`LynxView::pump`] answers
/// it through: `None` for a view's MTS realm, the worker's key for a worker.
/// `table` is the record [`encode_table`] wrote: the view's in a BTS, and
/// empty in the MTS realm and a plain `Worker`.
///
/// Everything crosses as text, because everything here is JavaScript's: the
/// arguments are the realm's own JSON, and the function arguments are named
/// by the indices they occupied rather than carried. Nothing is built here
/// but the notice itself — the view assembles the call, because the handle a
/// callback answers through is one the view already holds. It rides the
/// channel a source request uses, so a view that has ended assembles nothing
/// and the realm's functions are released.
///
/// [`LynxView::pump`]: crate::LynxView::pump
pub(crate) fn install(
    engine: &mut ScriptEngine,
    js_runtime: &mut ScriptRuntime,
    host: &HostOutbox,
    caller: Option<WorkerKey>,
    table: String,
) -> Result<(), ScriptError> {
    const NAME: &str = "bobcat-internal:native-modules.invokeNativeModule";
    let host = host.clone();
    engine.register_host_module_function(
        js_runtime,
        NATIVE_MODULES_HOST_SPECIFIER,
        "invokeNativeModule",
        5,
        Box::new(move |arguments| {
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
        }),
    )?;
    let mut table = Some(table);
    engine.register_host_module_function(
        js_runtime,
        NATIVE_MODULES_HOST_SPECIFIER,
        "nativeModuleTable",
        0,
        Box::new(move |_arguments| {
            Ok(table.take().map_or(HostValue::Undefined, HostValue::String))
        }),
    )
}

/// Hands one native module's answer to one function argument of one call
/// back to the realm that made it, through `bobcat:native-modules`.
///
/// `arguments` is the JSON array text the realm spreads; `None` releases the
/// function without calling it, which is what a module that dropped its
/// callback owes. A callback for a call the realm has forgotten is a no-op
/// over there, so nothing here has to know which calls are outstanding.
pub(crate) fn deliver(
    engine: &mut ScriptEngine,
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
            NATIVE_MODULES_MODULE_SPECIFIER,
            NATIVE_MODULE_CALLBACK_EXPORT,
            &[
                HostArgument::Number(call),
                HostArgument::Number(f64::from(index)),
                arguments.map_or(HostArgument::Undefined, HostArgument::String),
            ],
        )
        .map(|_| ())
}

fn string(arguments: &[HostValue], index: usize) -> Result<&str, String> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value),
        _ => Err(format!(
            "bobcat-internal:native-modules.invokeNativeModule expects string argument {index}"
        )),
    }
}

/// The call number the realm minted, which it counts up from one and spells
/// as a number.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the checks below leave a whole, representable call number"
)]
fn call_id(arguments: &[HostValue]) -> Result<u64, String> {
    match arguments.first() {
        Some(&HostValue::Number(value))
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

/// The modules a view was built with, as the realm is told about them: each
/// module's name followed by its method names joined with commas.
///
/// One `Vec` rather than a map because it is read once, in order, by the
/// encoder below — the realm builds the object, and duplicate names were
/// already refused where the view was constructed.
pub(crate) type NativeModuleTable = Vec<(String, Vec<String>)>;

/// Encodes a module table as the `<utf16Length>:<text>` record a realm's
/// `nativeModuleTable` answers and `bobcat:bts-runtime` decodes, two fields
/// per module: the name, then the comma-joined method list.
///
/// The same format `attributeNames` answers in, for the same reason — the
/// length is in UTF-16 code units, so `String.prototype.slice` reads each
/// field without a scan and a name may contain any character at all. A module
/// with no methods still writes both fields, the second empty.
pub(crate) fn encode_table(modules: &NativeModuleTable) -> String {
    let mut record = String::new();
    for (name, methods) in modules {
        crate::main::runtime::write_record_field(&mut record, name);
        crate::main::runtime::write_record_field(&mut record, &methods.join(","));
    }
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_with_no_methods_still_writes_both_of_its_fields() {
        let table: NativeModuleTable = vec![
            (
                "Echo".to_owned(),
                vec!["ping".to_owned(), "pong".to_owned()],
            ),
            ("Bare".to_owned(), Vec::new()),
        ];
        assert_eq!(encode_table(&table), "4:Echo9:ping,pong4:Bare0:");
    }

    #[test]
    fn a_released_callback_sends_no_arguments_and_an_invoked_one_sends_its_own() {
        let (sender, mut inbox) = mpsc::unbounded_channel();
        let call = ModuleCall::assemble(
            7,
            "ping".to_owned(),
            "[null,null]".to_owned(),
            &[2, 3],
            &ModuleReply::Worker(sender.downgrade()),
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
                } => (call, index, arguments),
                _ => panic!("a module callback is the only message here"),
            })
            .collect();
        assert_eq!(
            sent,
            [(7, 2, None), (7, 3, Some("[1]".to_owned()))],
            "a release carries no arguments and an invocation carries its own"
        );

        // The reply handle is what a module reads before doing work it would
        // only do to answer a realm that is still there: the worker's
        // receiving end drops with its task, and that is the whole of it.
        let live = ModuleCallback::new(7, 4, ModuleReply::Worker(sender.downgrade()));
        assert!(!live.is_cancelled());
        drop(inbox);
        assert!(live.is_cancelled());
    }

    /// A call the MTS realm made is answered through the view's command
    /// FIFO, with the same three fields a worker's answer carries, and is
    /// cancelled once the view's task has dropped the receiving end.
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
}
