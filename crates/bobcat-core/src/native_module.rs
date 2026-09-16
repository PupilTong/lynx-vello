//! Host-injected `NativeModules` for the background realm.
//!
//! A Lynx application reaches host capabilities through
//! `NativeModules.<name>.<method>(...)`, and this is the whole of what that
//! call is: the BTS realm hands the host a method name, the arguments as JSON
//! text, and one [`ModuleCallback`] per function argument; the embedder's
//! module answers by invoking a callback, whenever it likes and from wherever
//! it likes.
//!
//! The shape is native Lynx's, not a promise's. A method returns `undefined`
//! in JavaScript — there is no synchronous result to return, because the
//! module runs on the embedder's thread and the caller is on another one —
//! and every function argument becomes a single-shot callback the way
//! native's `ModuleCallback` does. A callback that is never invoked and never
//! dropped simply keeps its JavaScript function alive, as native's does.

use tokio::sync::mpsc;

use crate::background::WorkerMessage;

/// One embedder-owned module, reachable as `NativeModules.<name>` in the BTS
/// realm.
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
    /// does not carry.
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
/// `Send`: it is built in the BTS realm on `bobcat-workers` and travels to the
/// embedder's thread inside a view notice, and an embedder may hand it on to a
/// worker thread of its own and answer from there.
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
    /// Builds one call out of what the realm said and the handle on the inbox
    /// that answers it.
    ///
    /// The two halves meet here rather than in the realm: what crosses from
    /// `bobcat-workers` is the call's text and the indices of its function
    /// arguments, and the reply handle is the one the view already registered
    /// for that worker. `LynxView::pump` is where both are in hand, and this
    /// is what it assembles them with.
    pub(crate) fn assemble(
        call: u64,
        method: String,
        arguments: String,
        callbacks: &[u32],
        reply: &mpsc::WeakUnboundedSender<WorkerMessage>,
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
/// The invocation itself is a message: it is sent to the BTS realm when the
/// handle drops, and runs there on the worker's own turn. That is why this is
/// `Send` and why invoking from any thread is fine.
pub struct ModuleCallback {
    /// The call this function argument belonged to, as the realm numbered it.
    call: u64,
    /// Which argument of that call it was.
    index: u32,
    /// The calling worker's own inbox, weakly: a callback a module never gets
    /// round to must not keep a worker realm alive. It is also the whole of
    /// what [`Self::is_cancelled`] reads — a worker's receiving end drops with
    /// its task, so a handle that no longer upgrades *is* the realm being
    /// gone.
    reply: mpsc::WeakUnboundedSender<WorkerMessage>,
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
    pub(crate) const fn new(
        call: u64,
        index: u32,
        reply: mpsc::WeakUnboundedSender<WorkerMessage>,
    ) -> Self {
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
    /// the view that owned it was released.
    ///
    /// Read off the reply handle itself, which is the same fact: the worker's
    /// receiving end drops with its task, so there is nothing left to answer
    /// exactly when there is nothing left to answer *through*.
    ///
    /// Invoking afterwards is a silent no-op rather than an error, the way a
    /// post to a terminated worker is, so this is an optimization for a module
    /// about to do expensive work rather than a check it has to make.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.reply.upgrade().is_none_or(|sender| sender.is_closed())
    }

    /// Calls the JavaScript function with `arguments`, JSON array text the
    /// realm parses and spreads.
    ///
    /// Consumes the handle, because a callback is invoked at most once. The
    /// call itself happens on the BTS thread, on that realm's next turn.
    pub fn invoke(mut self, arguments: String) {
        self.arguments = Some(arguments);
    }
}

/// Where the invocation is actually sent, so that invoking and releasing take
/// one path and a callback dropped in a panic still releases its function.
impl Drop for ModuleCallback {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.upgrade() {
            let _ = reply.send(WorkerMessage::ModuleCallback {
                call: self.call,
                index: self.index,
                arguments: self.arguments.take(),
            });
        }
    }
}

/// The modules a view was built with, as the realm is told about them: each
/// module's name followed by its method names joined with commas.
///
/// One `Vec` rather than a map because it is read once, in order, by the
/// encoder below — the realm builds the object, and duplicate names were
/// already refused where the view was constructed.
pub(crate) type NativeModuleTable = Vec<(String, Vec<String>)>;

/// Encodes a module table as the `<utf16Length>:<text>` record the MTS realm
/// decodes, two fields per module: the name, then the comma-joined method
/// list.
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
            &sender.downgrade(),
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
        let live = ModuleCallback::new(7, 4, sender.downgrade());
        assert!(!live.is_cancelled());
        drop(inbox);
        assert!(live.is_cancelled());
    }
}
