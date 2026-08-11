//! A small, safe Rust boundary around the repository's pinned `QuickJS` source.
//!
//! Host-function arguments exchange JavaScript primitives and one deliberately
//! narrow object type: [`HostValue::Object`]. Such an object is created only by
//! this bridge, carries an opaque `u32` payload, and is recognized by its
//! private `QuickJS` class rather than by properties JavaScript could forge.
//! Plain objects, arrays, functions, symbols, and proxies remain rejected.
//! [`HostFunctionOutput`] is the explicit return-only seam for preserving the
//! identity of an accepted argument or an existing rooted [`Value`].
//!
//! `QuickJS` owns each wrapper's reachability. Its class finalizer does not call
//! application code or re-enter `QuickJS`: it only appends the payload to a
//! cloneable [`HostObjectReleaseQueue`]. A host function such as a tree flush
//! can drain that queue while JavaScript is still on the stack. [`WeakValue`]
//! exposes a real `QuickJS` weak reference for host-side identity caches without
//! keeping the target alive.

mod ffi;

#[allow(
    unsafe_code,
    reason = "this private implementation module contains the audited QuickJS FFI call sites"
)]
mod implementation {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CString, c_void};
    use std::num::TryFromIntError;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr::{self, NonNull};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use std::{fmt, mem};

    use smallvec::SmallVec;

    use super::ffi;

    const HOST_INLINE_ARGS: usize = 8;

    const JS_EVAL_TYPE_GLOBAL: i32 = 0;
    const JS_EVAL_TYPE_MODULE: i32 = 1;
    const JS_EVAL_FLAG_STRICT: i32 = 1 << 3;
    const JS_EVAL_FLAG_BACKTRACE_BARRIER: i32 = 1 << 6;
    const JS_EVAL_FLAG_ASYNC: i32 = 1 << 7;
    const QJS_EVAL_FAILURE_COMPILE: i32 = 1;

    /// Limits and timeout applied when a realm is created.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct RealmOptions {
        pub memory_limit: Option<usize>,
        pub max_stack_size: Option<usize>,
        pub execution_timeout: Option<Duration>,
    }

    /// JavaScript source text and diagnostic metadata.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct EvalSource<'a> {
        pub text: &'a str,
        pub name: Option<&'a str>,
        pub line_offset: u32,
    }

    impl<'a> EvalSource<'a> {
        #[must_use]
        pub const fn new(text: &'a str) -> Self {
            Self {
                text,
                name: None,
                line_offset: 0,
            }
        }
    }

    /// Evaluation mode and `QuickJS` execution flags.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    #[allow(
        clippy::struct_excessive_bools,
        reason = "these independent flags map directly to QuickJS evaluation flags"
    )]
    pub struct EvalOptions {
        pub source_type: SourceType,
        pub strict: bool,
        pub backtrace_barrier: bool,
        pub top_level_await: bool,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum SourceType {
        #[default]
        Script,
        Module,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum ValueKind {
        Undefined,
        Null,
        Boolean,
        Number,
        BigInt,
        String,
        Symbol,
        Function,
        Object,
        Other,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum ErrorPhase {
        CreateRealm,
        ConstructValue,
        Evaluate,
        Call,
        ConvertValue,
        PendingJob,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    #[non_exhaustive]
    pub enum ErrorKind {
        Syntax,
        Exception,
        OutOfMemory,
        InvalidInput,
        WrongRealm,
        NotCallable,
        TypeMismatch,
        TooManyArguments,
        Interrupted,
        ExecutionTimeout,
        Engine,
    }

    /// Sanitized source coordinates from a JavaScript error.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SourceLocation {
        pub source: Option<String>,
        pub line: Option<u32>,
        pub column: Option<u32>,
    }

    /// A JavaScript or bridge failure detached from engine-owned values.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Error {
        pub kind: ErrorKind,
        pub phase: ErrorPhase,
        pub name: Option<String>,
        pub message: String,
        pub stack: Option<String>,
        pub location: Option<SourceLocation>,
    }

    impl fmt::Display for Error {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "{:?} during {:?}: {}",
                self.kind, self.phase, self.message
            )
        }
    }

    impl std::error::Error for Error {}

    impl Error {
        fn bridge(kind: ErrorKind, phase: ErrorPhase, message: impl Into<String>) -> Self {
            Self {
                kind,
                phase,
                name: None,
                message: message.into(),
                stack: None,
                location: None,
            }
        }
    }

    /// Result of a bounded pending-job drain.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct JobDrain {
        pub executed: usize,
        pub jobs_remaining: bool,
    }

    const INTERRUPT_REQUESTED: u64 = 1;
    const MAX_INTERRUPT_GENERATION: u64 = u64::MAX >> 1;

    #[derive(Debug)]
    struct InterruptShared {
        active: AtomicU64,
    }

    /// Thread-safe handle for interrupting the currently running JavaScript task.
    #[derive(Clone, Debug)]
    pub struct InterruptHandle {
        shared: Arc<InterruptShared>,
    }

    impl InterruptHandle {
        /// Requests interruption when JavaScript is currently running.
        #[must_use]
        pub fn request_interrupt_if_running(&self) -> bool {
            let active = self.shared.active.load(Ordering::Acquire);
            if active == 0 {
                return false;
            }
            if active & INTERRUPT_REQUESTED != 0 {
                return true;
            }

            match self.shared.active.compare_exchange(
                active,
                active | INTERRUPT_REQUESTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => true,
                Err(observed) => observed == active | INTERRUPT_REQUESTED,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum InterruptReason {
        HostRequest,
        Deadline,
        HandlerFailure,
    }

    struct InterruptState {
        shared: Arc<InterruptShared>,
        timeout: Option<Duration>,
        next_generation: Cell<u64>,
        active_token: Cell<u64>,
        deadline: Cell<Option<Instant>>,
        reason: Cell<Option<InterruptReason>>,
    }

    impl InterruptState {
        fn new(timeout: Option<Duration>) -> Self {
            Self {
                shared: Arc::new(InterruptShared {
                    active: AtomicU64::new(0),
                }),
                timeout,
                next_generation: Cell::new(0),
                active_token: Cell::new(0),
                deadline: Cell::new(None),
                reason: Cell::new(None),
            }
        }

        fn begin(self: &Rc<Self>) -> ExecutionGuard {
            debug_assert_eq!(
                self.shared.active.load(Ordering::Acquire),
                0,
                "QuickJS execution guards must not be nested"
            );
            let mut generation = self.next_generation.get() + 1;
            if generation > MAX_INTERRUPT_GENERATION {
                generation = 1;
            }
            self.next_generation.set(generation);
            let token = generation << 1;
            let now = Instant::now();
            let deadline = self
                .timeout
                .map(|timeout| now.checked_add(timeout).unwrap_or(now));

            self.reason.set(None);
            self.deadline.set(deadline);
            self.active_token.set(token);
            self.shared.active.store(token, Ordering::Release);
            ExecutionGuard {
                state: Rc::clone(self),
                token,
                armed: true,
            }
        }

        fn poll(&self) -> bool {
            let token = self.active_token.get();
            if token == 0 {
                return false;
            }
            if self.reason.get().is_some() {
                return true;
            }
            let active = self.shared.active.load(Ordering::Acquire);
            if active == token | INTERRUPT_REQUESTED {
                self.reason.set(Some(InterruptReason::HostRequest));
                return true;
            }
            if active != token {
                return false;
            }
            if self
                .deadline
                .get()
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                self.reason.set(Some(InterruptReason::Deadline));
                return true;
            }
            false
        }
    }

    struct ExecutionGuard {
        state: Rc<InterruptState>,
        token: u64,
        armed: bool,
    }

    impl ExecutionGuard {
        fn finish<T>(mut self, result: Result<T, Error>, phase: ErrorPhase) -> Result<T, Error> {
            let reason = self.state.reason.get();
            self.disarm();
            reason.map_or(result, |reason| Err(interrupt_error(reason, phase)))
        }

        fn disarm(&mut self) {
            if !self.armed {
                return;
            }
            let active = self.state.shared.active.swap(0, Ordering::AcqRel);
            debug_assert!(
                active == self.token || active == self.token | INTERRUPT_REQUESTED,
                "interrupt generation changed during one synchronous execution"
            );
            self.state.active_token.set(0);
            self.state.deadline.set(None);
            self.state.reason.set(None);
            self.armed = false;
        }
    }

    impl Drop for ExecutionGuard {
        fn drop(&mut self) {
            self.disarm();
        }
    }

    unsafe extern "C" fn interrupt_callback(opaque: *mut c_void) -> i32 {
        let state = unsafe { &*opaque.cast::<InterruptState>() };
        if let Ok(interrupted) = catch_unwind(AssertUnwindSafe(|| state.poll())) {
            i32::from(interrupted)
        } else {
            state.reason.set(Some(InterruptReason::HandlerFailure));
            1
        }
    }

    fn raw_to_utf16(
        context: *mut ffi::JSContext,
        raw: *mut ffi::QjsValue,
    ) -> Result<Vec<u16>, Error> {
        let mut bytes = ptr::null();
        let mut length = 0usize;
        let status =
            unsafe { ffi::qjs_value_to_cesu8(context, raw, &raw mut bytes, &raw mut length) };
        if status != 0 || bytes.is_null() {
            unsafe { ffi::qjs_discard_exception(context) };
            return Err(Error::bridge(
                ErrorKind::Engine,
                ErrorPhase::ConvertValue,
                "QuickJS could not convert the value to CESU-8",
            ));
        }
        let encoded = unsafe { std::slice::from_raw_parts(bytes, length) };
        let decoded = decode_cesu8(encoded);
        unsafe { ffi::qjs_cesu8_free(context, bytes) };
        decoded
            .map_err(|message| Error::bridge(ErrorKind::Engine, ErrorPhase::ConvertValue, message))
    }

    fn property_name(name: &str) -> Result<CString, Error> {
        CString::new(name).map_err(|_| {
            Error::bridge(
                ErrorKind::InvalidInput,
                ErrorPhase::ConstructValue,
                "property name contains a NUL byte",
            )
        })
    }

    fn read_host_arguments(
        count: usize,
        arguments: *const ffi::QjsHostArg,
    ) -> Result<SmallVec<[HostValue; HOST_INLINE_ARGS]>, HostFunctionError> {
        if count == 0 || arguments.is_null() {
            return Ok(SmallVec::new());
        }
        let raw = unsafe { std::slice::from_raw_parts(arguments, count) };
        raw.iter().map(read_host_argument).collect()
    }

    fn read_host_argument(argument: &ffi::QjsHostArg) -> Result<HostValue, HostFunctionError> {
        match argument.kind {
            ffi::HOST_ARG_UNDEFINED => Ok(HostValue::Undefined),
            ffi::HOST_ARG_NULL => Ok(HostValue::Null),
            ffi::HOST_ARG_BOOLEAN => Ok(HostValue::Boolean(argument.number != 0.0)),
            ffi::HOST_ARG_NUMBER => Ok(HostValue::Number(argument.number)),
            ffi::HOST_ARG_STRING => {
                let bytes = unsafe { std::slice::from_raw_parts(argument.text, argument.text_len) };
                if let Ok(text) = std::str::from_utf8(bytes) {
                    return Ok(HostValue::String(text.to_owned()));
                }
                let units = decode_cesu8(bytes).map_err(HostFunctionError::new)?;
                String::from_utf16(&units)
                    .map(HostValue::String)
                    .map_err(|_| {
                        HostFunctionError::new(
                            "an ill-formed UTF-16 string cannot cross the host-function boundary",
                        )
                    })
            }
            ffi::HOST_ARG_OBJECT => Ok(HostValue::Object(HostObject::new(argument.payload))),
            _ => Err(HostFunctionError::new(
                "host functions accept primitives and bridge-owned host objects only",
            )),
        }
    }

    fn throw_host_error(context: *mut ffi::JSContext, message: &str) {
        let sanitized: String = message.chars().filter(|&byte| byte != '\0').collect();
        let Ok(message) = CString::new(sanitized) else {
            return;
        };
        unsafe { ffi::qjs_throw_error(context, message.as_ptr()) };
    }

    unsafe extern "C" fn host_release(opaque: *mut c_void, handler: *mut c_void) {
        let table = unsafe { &*opaque.cast::<HostTable>() };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            table.note_released(handler.cast::<HostSlot>());
        }));
    }

    /// Fired by the garbage collector for a bridge-owned host object.
    ///
    /// The only observable action is appending the payload to the realm's
    /// release queue. No application callback runs here.
    unsafe extern "C" fn host_object_release(opaque: *mut c_void, payload: u32) {
        let table = unsafe { &*opaque.cast::<HostTable>() };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            table.host_object_releases.push(payload);
        }));
    }

    unsafe extern "C" fn host_dispatch(
        opaque: *mut c_void,
        handler: *mut c_void,
        argument_count: usize,
        arguments: *const ffi::QjsHostArg,
        result: *mut ffi::QjsHostResult,
    ) -> i32 {
        let table = unsafe { &*opaque.cast::<HostTable>() };
        let context = table.context.get();
        if context.is_null() {
            return 1;
        }
        let slot = unsafe { &*handler.cast::<HostSlot>() };

        let called = catch_unwind(AssertUnwindSafe(|| {
            let values = read_host_arguments(argument_count, arguments)?;
            let Ok(mut handler) = slot.handler.try_borrow_mut() else {
                return Err(HostFunctionError::new(
                    "this host function cannot be called while it is already running",
                ));
            };
            handler(&values)
        }));

        let returned = match called {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                throw_host_error(context, &error.message);
                return 1;
            }
            Err(_) => {
                throw_host_error(context, "the host function panicked");
                return 1;
            }
        };

        if let Err(error) = table.write_result(&returned, argument_count, unsafe { &mut *result }) {
            throw_host_error(context, &error.message);
            1
        } else {
            0
        }
    }

    fn interrupt_error(reason: InterruptReason, phase: ErrorPhase) -> Error {
        match reason {
            InterruptReason::HostRequest => Error::bridge(
                ErrorKind::Interrupted,
                phase,
                "QuickJS execution was interrupted by the host",
            ),
            InterruptReason::Deadline => Error::bridge(
                ErrorKind::ExecutionTimeout,
                phase,
                "QuickJS execution exceeded its configured timeout",
            ),
            InterruptReason::HandlerFailure => {
                Error::bridge(ErrorKind::Engine, phase, "QuickJS interrupt handler failed")
            }
        }
    }

    /// A Rust-backed object crossing the host-function boundary.
    ///
    /// [`HostObject::new`] is used only as a host-function return value. The
    /// bridge creates a private-class `QuickJS` object around it; JavaScript
    /// cannot forge an accepted object by copying properties. When that exact
    /// wrapper is later passed to a host function, its [`HostObject`] appears
    /// in [`HostValue::Object`].
    ///
    /// This type deliberately does not implement `Clone`. A handler receives
    /// arguments through a borrowed slice, so the wrapper value cannot escape
    /// the synchronous call or be returned as a second wrapper. Copy
    /// [`HostObject::payload`] if the host needs durable identity.
    #[derive(Debug, PartialEq, Eq)]
    pub struct HostObject {
        payload: u32,
    }

    impl HostObject {
        #[must_use]
        pub const fn new(payload: u32) -> Self {
            Self { payload }
        }

        #[must_use]
        pub const fn payload(&self) -> u32 {
            self.payload
        }
    }

    /// Payloads with no remaining bridge-owned JS wrapper.
    ///
    /// A payload arrives either when a completed wrapper becomes unreachable
    /// or when allocating that wrapper fails after the host has produced it.
    ///
    /// Clones share one owner-thread-bound queue. [`drain`](Self::drain) only
    /// takes a Rust `Vec<u32>`; it neither enters `QuickJS` nor invokes a user
    /// callback, so a leaf host function may safely call it while JavaScript is
    /// on the stack (notably immediately before committing a tree flush).
    #[derive(Clone)]
    pub struct HostObjectReleaseQueue {
        inner: Rc<RefCell<Vec<u32>>>,
    }

    impl HostObjectReleaseQueue {
        fn new() -> Self {
            Self {
                inner: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn push(&self, payload: u32) {
            self.inner.borrow_mut().push(payload);
        }

        #[must_use]
        pub fn drain(&self) -> Vec<u32> {
            mem::take(&mut *self.inner.borrow_mut())
        }

        #[must_use]
        pub fn is_empty(&self) -> bool {
            self.inner.borrow().is_empty()
        }

        #[must_use]
        pub fn len(&self) -> usize {
            self.inner.borrow().len()
        }
    }

    impl fmt::Debug for HostObjectReleaseQueue {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("HostObjectReleaseQueue")
                .field("pending", &self.len())
                .finish_non_exhaustive()
        }
    }

    /// A value crossing the host-function boundary.
    ///
    /// Arguments reach a handler without any per-argument allocation: the shim
    /// flattens `argv` into a stack array of tags and values, and the reader
    /// below collects them into a `SmallVec`. Only a string argument allocates,
    /// and only to own its decoded text.
    ///
    /// Host functions speak primitives plus [`HostObject`]. Ordinary objects,
    /// arrays, functions, symbols, and proxies are rejected rather than handed
    /// to the callback. Strings arrive already
    /// validated as well-formed UTF-16, so a lone surrogate is a rejected
    /// argument, not a lossy conversion.
    #[derive(Debug, PartialEq)]
    #[non_exhaustive]
    pub enum HostValue {
        Undefined,
        Null,
        Boolean(bool),
        Number(f64),
        String(String),
        Object(HostObject),
    }

    /// An identity-preserving host-function return value.
    ///
    /// Ordinary host functions return [`HostValue`] and keep the bridge's
    /// primitives-only contract. The explicit `*_with_output` APIs additionally
    /// allow a handler to return one of its original accepted arguments or an
    /// already-rooted same-realm [`Value`] without manufacturing another JS
    /// object.
    #[derive(Debug)]
    #[non_exhaustive]
    pub enum HostFunctionOutput {
        HostValue(HostValue),
        Argument(usize),
        Existing(Value),
    }

    impl HostFunctionOutput {
        /// Returns the exact JavaScript value passed at `index`.
        #[must_use]
        pub const fn argument(index: usize) -> Self {
            Self::Argument(index)
        }

        /// Returns the exact JavaScript value represented by `value`.
        #[must_use]
        pub fn existing(value: Value) -> Self {
            Self::Existing(value)
        }
    }

    impl From<HostValue> for HostFunctionOutput {
        fn from(value: HostValue) -> Self {
            Self::HostValue(value)
        }
    }

    /// Error returned by a Rust host function.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct HostFunctionError {
        pub message: String,
    }

    impl HostFunctionError {
        #[must_use]
        pub fn new(message: impl Into<String>) -> Self {
            Self {
                message: message.into(),
            }
        }
    }

    impl From<&str> for HostFunctionError {
        fn from(message: &str) -> Self {
            Self::new(message)
        }
    }

    impl From<String> for HostFunctionError {
        fn from(message: String) -> Self {
            Self::new(message)
        }
    }

    impl fmt::Display for HostFunctionError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.message)
        }
    }

    impl std::error::Error for HostFunctionError {}

    type HostHandler =
        Box<dyn FnMut(&[HostValue]) -> Result<HostFunctionOutput, HostFunctionError>>;

    struct HostSlot {
        handler: RefCell<HostHandler>,
    }

    struct HostTable {
        context: Cell<*mut ffi::JSContext>,
        pending_release: RefCell<Vec<*mut HostSlot>>,
        return_text: RefCell<Vec<u16>>,
        /// Receives payloads from the private-class finalizer or a failed
        /// wrapper construction. It is separate from closure reclamation so a
        /// host function can drain it during the same JavaScript evaluation
        /// that left a payload without an owner.
        host_object_releases: HostObjectReleaseQueue,
    }

    impl HostTable {
        fn new() -> Self {
            Self {
                context: Cell::new(ptr::null_mut()),
                pending_release: RefCell::new(Vec::new()),
                return_text: RefCell::new(Vec::new()),
                host_object_releases: HostObjectReleaseQueue::new(),
            }
        }

        fn write_result(
            &self,
            value: &HostFunctionOutput,
            argument_count: usize,
            out: &mut ffi::QjsHostResult,
        ) -> Result<(), HostFunctionError> {
            out.text = ptr::null();
            out.text_len = 0;
            out.number = 0.0;
            out.payload = 0;
            out.argument_index = 0;
            out.value = ptr::null_mut();
            match value {
                HostFunctionOutput::HostValue(value) => match value {
                    HostValue::Undefined => out.kind = ffi::HOST_ARG_UNDEFINED,
                    HostValue::Null => out.kind = ffi::HOST_ARG_NULL,
                    HostValue::Boolean(value) => {
                        out.kind = ffi::HOST_ARG_BOOLEAN;
                        out.number = if *value { 1.0 } else { 0.0 };
                    }
                    HostValue::Number(value) => {
                        out.kind = ffi::HOST_ARG_NUMBER;
                        out.number = *value;
                    }
                    HostValue::String(value) => {
                        let mut scratch = self.return_text.borrow_mut();
                        scratch.clear();
                        scratch.extend(value.encode_utf16());
                        out.kind = ffi::HOST_ARG_STRING;
                        out.text = scratch.as_ptr();
                        out.text_len = scratch.len();
                    }
                    HostValue::Object(object) => {
                        out.kind = ffi::HOST_ARG_OBJECT;
                        out.payload = object.payload();
                    }
                },
                HostFunctionOutput::Argument(index) => {
                    if *index >= argument_count {
                        return Err(HostFunctionError::new(format!(
                            "host return argument {index} is out of range for {argument_count} arguments"
                        )));
                    }
                    out.kind = ffi::HOST_RESULT_ARGUMENT;
                    out.argument_index = *index;
                }
                HostFunctionOutput::Existing(value) => {
                    let context = self.context.get();
                    if context != value.inner.owner.context.as_ptr() {
                        return Err(HostFunctionError::new(
                            "host return value belongs to a different QuickJS realm",
                        ));
                    }
                    out.kind = ffi::HOST_RESULT_VALUE;
                    out.value = unsafe { ffi::qjs_value_dup(context, value.inner.raw.as_ptr()) };
                    if out.value.is_null() {
                        return Err(HostFunctionError::new(
                            "QuickJS could not duplicate the host return value",
                        ));
                    }
                }
            }
            Ok(())
        }

        fn note_released(&self, slot: *mut HostSlot) {
            if let Ok(mut pending) = self.pending_release.try_borrow_mut() {
                pending.push(slot);
            }
        }

        fn reclaim(&self) {
            loop {
                let batch = {
                    let Ok(mut pending) = self.pending_release.try_borrow_mut() else {
                        return;
                    };
                    if pending.is_empty() {
                        return;
                    }
                    mem::take(&mut *pending)
                };
                for slot in batch {
                    drop(unsafe { Box::from_raw(slot) });
                }
            }
        }
    }

    struct RealmInner {
        runtime: NonNull<ffi::QjsRuntime>,
        context: NonNull<ffi::JSContext>,
        interrupt: Rc<InterruptState>,
        hosts: Box<HostTable>,
    }

    impl Drop for RealmInner {
        fn drop(&mut self) {
            unsafe {
                ffi::qjs_runtime_set_interrupt_handler(
                    self.runtime.as_ptr(),
                    None,
                    ptr::null_mut(),
                );
                self.interrupt.shared.active.store(0, Ordering::Release);
                self.hosts.context.set(ptr::null_mut());
                ffi::qjs_context_free(self.context.as_ptr());
                ffi::qjs_runtime_free(self.runtime.as_ptr());
                self.hosts.reclaim();
            }
        }
    }

    /// Owner-thread-bound `QuickJS` runtime and global realm.
    pub struct Realm {
        inner: Rc<RealmInner>,
    }

    impl fmt::Debug for Realm {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("Realm")
                .field(
                    "live_handles",
                    &Rc::strong_count(&self.inner).saturating_sub(1),
                )
                .finish_non_exhaustive()
        }
    }

    impl Realm {
        pub fn new() -> Result<Self, Error> {
            Self::with_options(RealmOptions::default())
        }

        pub fn with_options(options: RealmOptions) -> Result<Self, Error> {
            if options
                .execution_timeout
                .is_some_and(|timeout| Instant::now().checked_add(timeout).is_none())
            {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::CreateRealm,
                    "execution timeout exceeds this platform's monotonic-clock range",
                ));
            }

            unsafe {
                let runtime = NonNull::new(ffi::qjs_runtime_new()).ok_or_else(|| {
                    Error::bridge(
                        ErrorKind::OutOfMemory,
                        ErrorPhase::CreateRealm,
                        "QuickJS could not allocate a runtime",
                    )
                })?;
                if let Some(limit) = options.memory_limit {
                    ffi::qjs_runtime_set_memory_limit(runtime.as_ptr(), limit);
                }
                if let Some(size) = options.max_stack_size {
                    ffi::qjs_runtime_set_max_stack_size(runtime.as_ptr(), size);
                }
                let Some(context) = NonNull::new(ffi::qjs_context_new(runtime.as_ptr())) else {
                    ffi::qjs_runtime_free(runtime.as_ptr());
                    return Err(Error::bridge(
                        ErrorKind::OutOfMemory,
                        ErrorPhase::CreateRealm,
                        "QuickJS could not allocate a context",
                    ));
                };
                let interrupt = Rc::new(InterruptState::new(options.execution_timeout));
                ffi::qjs_runtime_set_interrupt_handler(
                    runtime.as_ptr(),
                    Some(interrupt_callback),
                    Rc::as_ptr(&interrupt).cast_mut().cast(),
                );
                let hosts = Box::new(HostTable::new());
                hosts.context.set(context.as_ptr());
                ffi::qjs_runtime_set_host_dispatch(
                    runtime.as_ptr(),
                    Some(host_dispatch),
                    Some(host_release),
                    Some(host_object_release),
                    (&raw const *hosts).cast_mut().cast(),
                );
                Ok(Self {
                    inner: Rc::new(RealmInner {
                        runtime,
                        context,
                        interrupt,
                        hosts,
                    }),
                })
            }
        }

        #[must_use]
        pub fn interrupt_handle(&self) -> InterruptHandle {
            InterruptHandle {
                shared: Arc::clone(&self.inner.interrupt.shared),
            }
        }

        /// A cloneable view of this realm's host-object release queue.
        ///
        /// A host function may capture the clone and drain it without calling
        /// back into the realm. This is stronger than waiting for `evaluate`
        /// to return: an object finalized (or whose wrapper allocation failed)
        /// earlier in one evaluation can be retired before a later host call
        /// in that same evaluation commits externally visible state.
        #[must_use]
        pub fn host_object_release_queue(&self) -> HostObjectReleaseQueue {
            self.inner.hosts.host_object_releases.clone()
        }

        fn reclaim(&mut self) {
            self.inner.hosts.reclaim();
        }

        /// Runs a full garbage collection and reclaims collected host closures.
        pub fn run_gc(&mut self) {
            unsafe { ffi::qjs_runtime_run_gc(self.inner.runtime.as_ptr()) };
            self.reclaim();
        }

        pub fn global_object(&self) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_global_object(context)
            })
        }

        /// Replaces a named property while enforcing realm affinity and interrupts.
        pub fn set_property(
            &mut self,
            target: &Value,
            name: &str,
            value: &Value,
        ) -> Result<(), Error> {
            self.reclaim();
            self.ensure_affinity(target, ErrorPhase::ConstructValue)?;
            self.ensure_affinity(value, ErrorPhase::ConstructValue)?;
            let name = property_name(name)?;
            let context = self.inner.context.as_ptr();
            let guard = self.inner.interrupt.begin();
            let status = unsafe {
                ffi::qjs_set_property(
                    context,
                    target.inner.raw.as_ptr(),
                    name.as_ptr(),
                    value.inner.raw.as_ptr(),
                )
            };
            let result = if status < 0 {
                Err(self.capture_exception(context, ErrorPhase::ConstructValue))
            } else {
                Ok(())
            };
            guard.finish(result, ErrorPhase::ConstructValue)
        }

        /// Creates a JavaScript callable backed by a Rust closure.
        pub fn function<F>(&mut self, name: &str, arity: u32, handler: F) -> Result<Value, Error>
        where
            F: FnMut(&[HostValue]) -> Result<HostValue, HostFunctionError> + 'static,
        {
            let mut handler = handler;
            self.function_with_output(name, arity, move |arguments| {
                handler(arguments).map(HostFunctionOutput::from)
            })
        }

        /// Creates a callable whose handler may preserve JavaScript identity in
        /// its return value.
        ///
        /// This is the same leaf callback seam as [`Realm::function`]; the only
        /// extension is [`HostFunctionOutput`]. Returning an existing [`Value`]
        /// duplicates its `QuickJS` root for the JavaScript caller. Capturing a
        /// same-realm `Value` directly in the handler therefore has the same
        /// reference-cycle caveat documented on [`Realm::function`].
        pub fn function_with_output<F>(
            &mut self,
            name: &str,
            arity: u32,
            handler: F,
        ) -> Result<Value, Error>
        where
            F: FnMut(&[HostValue]) -> Result<HostFunctionOutput, HostFunctionError> + 'static,
        {
            self.reclaim();
            let name = property_name(name)?;
            let arity = i32::try_from(arity).map_err(int_conversion_error)?;
            let slot = Box::into_raw(Box::new(HostSlot {
                handler: RefCell::new(Box::new(handler)),
            }));
            let context = self.inner.context.as_ptr();
            let raw =
                unsafe { ffi::qjs_new_host_function(context, name.as_ptr(), arity, slot.cast()) };
            match self.value_or_exception(raw, context, ErrorPhase::ConstructValue) {
                Ok(value) => Ok(value),
                Err(error) => {
                    drop(unsafe { Box::from_raw(slot) });
                    Err(error)
                }
            }
        }

        /// Installs a Rust-backed callable on the global object.
        pub fn define_global_function<F>(
            &mut self,
            name: &str,
            arity: u32,
            handler: F,
        ) -> Result<(), Error>
        where
            F: FnMut(&[HostValue]) -> Result<HostValue, HostFunctionError> + 'static,
        {
            let function = self.function(name, arity, handler)?;
            let global = self.global_object()?;
            self.set_property(&global, name, &function)
        }

        /// Defines a global callable backed by an identity-preserving handler —
        /// [`Realm::function_with_output`] plus [`Realm::set_property`].
        pub fn define_global_function_with_output<F>(
            &mut self,
            name: &str,
            arity: u32,
            handler: F,
        ) -> Result<(), Error>
        where
            F: FnMut(&[HostValue]) -> Result<HostFunctionOutput, HostFunctionError> + 'static,
        {
            let function = self.function_with_output(name, arity, handler)?;
            let global = self.global_object()?;
            self.set_property(&global, name, &function)
        }

        pub fn undefined(&self) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_undefined(context)
            })
        }

        pub fn null(&self) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_null(context)
            })
        }

        pub fn boolean(&self, value: bool) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_boolean(context, i32::from(value))
            })
        }

        pub fn number(&self, value: f64) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_number(context, value)
            })
        }

        pub fn big_int64(&self, value: i64) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_big_int64(context, value)
            })
        }

        pub fn big_uint64(&self, value: u64) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_big_uint64(context, value)
            })
        }

        pub fn big_int_decimal(&mut self, decimal: &str) -> Result<Value, Error> {
            if !is_canonical_big_int(decimal) {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::ConstructValue,
                    "BigInt text must be canonical signed decimal",
                ));
            }
            let mut source = String::new();
            source
                .try_reserve(decimal.len().saturating_add(1))
                .map_err(|_| {
                    Error::bridge(
                        ErrorKind::OutOfMemory,
                        ErrorPhase::ConstructValue,
                        "could not allocate BigInt source",
                    )
                })?;
            source.push_str(decimal);
            source.push('n');
            self.evaluate(
                EvalSource {
                    text: &source,
                    name: Some("<host bigint>"),
                    line_offset: 0,
                },
                EvalOptions::default(),
            )
            .map_err(|mut error| {
                error.phase = ErrorPhase::ConstructValue;
                error
            })
        }

        pub fn string(&self, value: &str) -> Result<Value, Error> {
            let utf16: Vec<u16> = value.encode_utf16().collect();
            self.string_utf16(&utf16)
        }

        pub fn string_utf16(&self, units: &[u16]) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_string_utf16(context, units.as_ptr(), units.len())
            })
        }

        /// Creates the private-class JavaScript wrapper for one host payload.
        ///
        /// The returned [`Value`] is a strong root. Once every strong JS/Rust
        /// reference is gone, the wrapper's finalizer appends `payload` to this
        /// realm's [`HostObjectReleaseQueue`].
        pub fn host_object(&mut self, payload: u32) -> Result<Value, Error> {
            self.reclaim();
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_host_object(context, payload)
            })
        }

        /// Creates a real `QuickJS` weak reference to `value`.
        ///
        /// [`WeakValue`] keeps the realm and its internal `WeakRef` alive, but it
        /// does not keep `value` alive. Only objects, functions, and weakly
        /// referenceable symbols are accepted by `QuickJS`.
        pub fn downgrade(&mut self, value: &Value) -> Result<WeakValue, Error> {
            self.reclaim();
            self.ensure_affinity(value, ErrorPhase::ConstructValue)?;
            let weak = self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_weak_value_new(context, value.inner.raw.as_ptr())
            })?;
            Ok(WeakValue { inner: weak })
        }

        pub fn evaluate(
            &mut self,
            source: EvalSource<'_>,
            options: EvalOptions,
        ) -> Result<Value, Error> {
            self.reclaim();
            if options.source_type == SourceType::Module && options.top_level_await {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::Evaluate,
                    "top_level_await is only a classic-script option",
                ));
            }
            let source_name = CString::new(source.name.unwrap_or("<eval>")).map_err(|_| {
                Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::Evaluate,
                    "source name contains a NUL byte",
                )
            })?;
            let prefix = usize::try_from(source.line_offset).map_err(int_conversion_error)?;
            let capacity = prefix
                .checked_add(source.text.len())
                .and_then(|length| length.checked_add(1))
                .ok_or_else(|| {
                    Error::bridge(
                        ErrorKind::OutOfMemory,
                        ErrorPhase::Evaluate,
                        "source plus line offset is too large",
                    )
                })?;
            let mut terminated = Vec::new();
            terminated.try_reserve_exact(capacity).map_err(|_| {
                Error::bridge(
                    ErrorKind::OutOfMemory,
                    ErrorPhase::Evaluate,
                    "could not allocate terminated source text",
                )
            })?;
            if prefix > 0 && source.text.starts_with("#!") {
                let hashbang_length = hashbang_end(source.text);
                terminated.extend_from_slice(&source.text.as_bytes()[..hashbang_length]);
                terminated.resize(terminated.len() + prefix, b'\n');
                terminated.extend_from_slice(&source.text.as_bytes()[hashbang_length..]);
            } else {
                terminated.resize(prefix, b'\n');
                terminated.extend_from_slice(source.text.as_bytes());
            }
            let source_length = terminated.len();
            terminated.push(0);

            let mut flags = if options.source_type == SourceType::Module {
                JS_EVAL_TYPE_MODULE
            } else {
                JS_EVAL_TYPE_GLOBAL
            };
            if options.strict {
                flags |= JS_EVAL_FLAG_STRICT;
            }
            if options.backtrace_barrier {
                flags |= JS_EVAL_FLAG_BACKTRACE_BARRIER;
            }
            if options.top_level_await {
                flags |= JS_EVAL_FLAG_ASYNC;
            }

            let mut failure_stage = 0;
            let guard = self.inner.interrupt.begin();
            let raw = unsafe {
                ffi::qjs_eval(
                    self.inner.context.as_ptr(),
                    terminated.as_ptr(),
                    source_length,
                    source_name.as_ptr(),
                    flags,
                    &raw mut failure_stage,
                )
            };
            let result = NonNull::new(raw)
                .map(|raw| Value::from_raw(Rc::clone(&self.inner), raw))
                .ok_or_else(|| {
                    self.capture_exception_with_syntax(
                        self.inner.context.as_ptr(),
                        ErrorPhase::Evaluate,
                        failure_stage == QJS_EVAL_FAILURE_COMPILE,
                    )
                });
            guard.finish(result, ErrorPhase::Evaluate)
        }

        pub fn call(
            &mut self,
            callable: &Value,
            this_value: Option<&Value>,
            arguments: &[Value],
        ) -> Result<Value, Error> {
            self.reclaim();
            self.ensure_affinity(callable, ErrorPhase::Call)?;
            if callable.kind() != ValueKind::Function {
                return Err(Error::bridge(
                    ErrorKind::NotCallable,
                    ErrorPhase::Call,
                    "value is not callable",
                ));
            }
            if let Some(value) = this_value {
                self.ensure_affinity(value, ErrorPhase::Call)?;
            }
            for argument in arguments {
                self.ensure_affinity(argument, ErrorPhase::Call)?;
            }
            if i32::try_from(arguments.len()).is_err() {
                return Err(Error::bridge(
                    ErrorKind::TooManyArguments,
                    ErrorPhase::Call,
                    "argument count exceeds QuickJS's signed-int ABI",
                ));
            }
            let raw_arguments: Vec<*const ffi::QjsValue> = arguments
                .iter()
                .map(|value| value.inner.raw.as_ptr().cast_const())
                .collect();
            let guard = self.inner.interrupt.begin();
            let raw = unsafe {
                ffi::qjs_call(
                    self.inner.context.as_ptr(),
                    callable.inner.raw.as_ptr(),
                    this_value.map_or(ptr::null(), |value| value.inner.raw.as_ptr()),
                    arguments.len(),
                    raw_arguments.as_ptr(),
                )
            };
            let result =
                self.value_or_exception(raw, self.inner.context.as_ptr(), ErrorPhase::Call);
            guard.finish(result, ErrorPhase::Call)
        }

        pub fn try_execute_pending_job(&mut self) -> Result<bool, Error> {
            self.reclaim();
            let guard = self.inner.interrupt.begin();
            let result = self.try_execute_pending_job_inner();
            guard.finish(result, ErrorPhase::PendingJob)
        }

        fn try_execute_pending_job_inner(&mut self) -> Result<bool, Error> {
            let mut job_context = ptr::null_mut();
            let result = unsafe {
                ffi::qjs_execute_pending_job(self.inner.runtime.as_ptr(), &raw mut job_context)
            };
            match result {
                0 => self.take_unhandled_rejection().map_or(Ok(false), Err),
                value if value > 0 => Ok(true),
                _ => {
                    let context = NonNull::new(job_context)
                        .map_or(self.inner.context.as_ptr(), NonNull::as_ptr);
                    Err(self.capture_exception(context, ErrorPhase::PendingJob))
                }
            }
        }

        #[must_use]
        pub fn has_pending_jobs(&self) -> bool {
            unsafe { ffi::qjs_has_pending_job(self.inner.runtime.as_ptr()) != 0 }
        }

        pub fn drain_pending_jobs(&mut self) -> Result<usize, Error> {
            self.reclaim();
            let guard = self.inner.interrupt.begin();
            let result = (|| {
                let mut executed = 0usize;
                while self.try_execute_pending_job_inner()? {
                    executed = executed.saturating_add(1);
                }
                Ok(executed)
            })();
            guard.finish(result, ErrorPhase::PendingJob)
        }

        /// Runs at most `budget` pending jobs and reports whether work remains.
        pub fn drain_pending_jobs_up_to(&mut self, budget: usize) -> Result<JobDrain, Error> {
            self.reclaim();
            let guard = self.inner.interrupt.begin();
            let result = (|| {
                let mut executed = 0usize;
                while executed < budget && self.try_execute_pending_job_inner()? {
                    executed += 1;
                }
                let jobs_remaining = self.has_pending_jobs();
                if !jobs_remaining && let Some(error) = self.take_unhandled_rejection() {
                    return Err(error);
                }
                Ok(JobDrain {
                    executed,
                    jobs_remaining,
                })
            })();
            guard.finish(result, ErrorPhase::PendingJob)
        }

        fn construct(
            &self,
            phase: ErrorPhase,
            constructor: impl FnOnce(*mut ffi::JSContext) -> *mut ffi::QjsValue,
        ) -> Result<Value, Error> {
            let raw = constructor(self.inner.context.as_ptr());
            self.value_or_exception(raw, self.inner.context.as_ptr(), phase)
        }

        fn value_or_exception(
            &self,
            raw: *mut ffi::QjsValue,
            context: *mut ffi::JSContext,
            phase: ErrorPhase,
        ) -> Result<Value, Error> {
            NonNull::new(raw)
                .map(|raw| Value::from_raw(Rc::clone(&self.inner), raw))
                .ok_or_else(|| self.capture_exception(context, phase))
        }

        fn ensure_affinity(&self, value: &Value, phase: ErrorPhase) -> Result<(), Error> {
            if Rc::ptr_eq(&self.inner, &value.inner.owner) {
                Ok(())
            } else {
                Err(Error::bridge(
                    ErrorKind::WrongRealm,
                    phase,
                    "value belongs to a different QuickJS realm",
                ))
            }
        }

        fn capture_exception(&self, context: *mut ffi::JSContext, phase: ErrorPhase) -> Error {
            self.capture_exception_with_syntax(context, phase, false)
        }

        fn capture_exception_with_syntax(
            &self,
            context: *mut ffi::JSContext,
            phase: ErrorPhase,
            syntax_is_parse_error: bool,
        ) -> Error {
            let raw = unsafe { ffi::qjs_take_exception(context) };
            let Some(raw) = NonNull::new(raw) else {
                return Error::bridge(
                    ErrorKind::OutOfMemory,
                    phase,
                    "could not allocate a box for the QuickJS exception",
                );
            };
            let exception = Value::from_raw(Rc::clone(&self.inner), raw);
            sanitize_exception(&exception, phase, syntax_is_parse_error)
        }

        fn take_unhandled_rejection(&self) -> Option<Error> {
            if unsafe { ffi::qjs_has_unhandled_rejection(self.inner.runtime.as_ptr()) } == 0 {
                return None;
            }
            let raw = unsafe { ffi::qjs_take_unhandled_rejection(self.inner.runtime.as_ptr()) };
            let Some(raw) = NonNull::new(raw) else {
                return Some(
                    self.capture_exception(self.inner.context.as_ptr(), ErrorPhase::PendingJob),
                );
            };
            let reason = Value::from_raw(Rc::clone(&self.inner), raw);
            Some(sanitize_exception(&reason, ErrorPhase::PendingJob, false))
        }
    }

    struct ValueInner {
        owner: Rc<RealmInner>,
        raw: NonNull<ffi::QjsValue>,
    }

    impl Drop for ValueInner {
        fn drop(&mut self) {
            unsafe { ffi::qjs_value_free(self.owner.context.as_ptr(), self.raw.as_ptr()) }
        }
    }

    /// Rooted JavaScript value tied to its owning realm.
    #[derive(Clone)]
    pub struct Value {
        inner: Rc<ValueInner>,
    }

    /// A `QuickJS` `WeakRef` that does not keep its target alive.
    ///
    /// Unlike [`std::rc::Weak`], this follows JavaScript reachability: JS
    /// references to the target keep it upgradeable even after every Rust
    /// [`Value`] root has been dropped. The weak handle itself remains bound to
    /// its owner realm and upgrades through [`WeakValue::upgrade`].
    #[derive(Clone)]
    pub struct WeakValue {
        inner: Value,
    }

    impl fmt::Debug for WeakValue {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.debug_struct("WeakValue").finish_non_exhaustive()
        }
    }

    impl WeakValue {
        /// Returns a new strong root while the JavaScript target is live, or
        /// `None` after it has been collected.
        ///
        /// This invokes only the bridge-captured native `WeakRef.prototype.deref`
        /// and cannot run application JavaScript, so an externally owned weak
        /// cache may upgrade from inside a leaf host callback. The weak handle
        /// itself keeps its realm alive; storing it directly in a host closure
        /// owned by that realm would therefore form a cycle. Keep the cache in
        /// external Rust state and capture [`std::rc::Weak`] to that state.
        pub fn upgrade(&self) -> Result<Option<Value>, Error> {
            let owner = Rc::clone(&self.inner.inner.owner);
            let context = owner.context.as_ptr();
            let raw =
                unsafe { ffi::qjs_weak_value_upgrade(context, self.inner.inner.raw.as_ptr()) };
            let value = if let Some(raw) = NonNull::new(raw) {
                Value::from_raw(owner, raw)
            } else {
                let realm = Realm { inner: owner };
                return Err(realm.capture_exception(context, ErrorPhase::ConstructValue));
            };
            if value.kind() == ValueKind::Undefined {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
    }

    impl fmt::Debug for Value {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("Value")
                .field("kind", &self.kind())
                .finish_non_exhaustive()
        }
    }

    impl Value {
        fn from_raw(owner: Rc<RealmInner>, raw: NonNull<ffi::QjsValue>) -> Self {
            Self {
                inner: Rc::new(ValueInner { owner, raw }),
            }
        }

        #[must_use]
        pub fn kind(&self) -> ValueKind {
            match unsafe {
                ffi::qjs_value_kind(self.inner.owner.context.as_ptr(), self.inner.raw.as_ptr())
            } {
                0 => ValueKind::Undefined,
                1 => ValueKind::Null,
                2 => ValueKind::Boolean,
                3 => ValueKind::Number,
                4 => ValueKind::BigInt,
                5 => ValueKind::String,
                6 => ValueKind::Symbol,
                7 => ValueKind::Function,
                8 => ValueKind::Object,
                _ => ValueKind::Other,
            }
        }

        #[must_use]
        pub fn as_boolean(&self) -> Option<bool> {
            if self.kind() != ValueKind::Boolean {
                return None;
            }
            let mut result = 0;
            let status = unsafe {
                ffi::qjs_value_get_boolean(
                    self.inner.owner.context.as_ptr(),
                    self.inner.raw.as_ptr(),
                    &raw mut result,
                )
            };
            (status == 0).then_some(result != 0)
        }

        #[must_use]
        pub fn as_number(&self) -> Option<f64> {
            if self.kind() != ValueKind::Number {
                return None;
            }
            let mut result = 0.0;
            let status = unsafe {
                ffi::qjs_value_get_number(
                    self.inner.owner.context.as_ptr(),
                    self.inner.raw.as_ptr(),
                    &raw mut result,
                )
            };
            (status == 0).then_some(result)
        }

        pub fn to_utf16(&self) -> Result<Vec<u16>, Error> {
            if self.kind() != ValueKind::String {
                return Err(Error::bridge(
                    ErrorKind::TypeMismatch,
                    ErrorPhase::ConvertValue,
                    "value is not a String",
                ));
            }
            self.to_utf16_coerced()
        }

        pub fn to_big_int_decimal(&self) -> Result<String, Error> {
            if self.kind() != ValueKind::BigInt {
                return Err(Error::bridge(
                    ErrorKind::TypeMismatch,
                    ErrorPhase::ConvertValue,
                    "value is not a BigInt",
                ));
            }
            let units = self.to_utf16_coerced()?;
            String::from_utf16(&units).map_err(|_| {
                Error::bridge(
                    ErrorKind::Engine,
                    ErrorPhase::ConvertValue,
                    "QuickJS produced non-Unicode BigInt text",
                )
            })
        }

        fn to_utf16_coerced(&self) -> Result<Vec<u16>, Error> {
            raw_to_utf16(self.inner.owner.context.as_ptr(), self.inner.raw.as_ptr())
        }

        fn property_string(&self, name: &str) -> Option<String> {
            let property = self.property(name)?;
            if property.kind() != ValueKind::String {
                return None;
            }
            String::from_utf16(&property.to_utf16().ok()?).ok()
        }

        fn property_u32(&self, name: &str) -> Option<u32> {
            let property = self.property(name)?;
            let number = property.as_number()?;
            if number.is_finite() && number >= 0.0 && number <= f64::from(u32::MAX) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(number as u32)
            } else {
                None
            }
        }

        fn property(&self, name: &str) -> Option<Self> {
            let name = CString::new(name).ok()?;
            let raw = unsafe {
                ffi::qjs_get_property(
                    self.inner.owner.context.as_ptr(),
                    self.inner.raw.as_ptr(),
                    name.as_ptr(),
                )
            };
            if let Some(raw) = NonNull::new(raw) {
                Some(Self::from_raw(Rc::clone(&self.inner.owner), raw))
            } else {
                unsafe { ffi::qjs_discard_exception(self.inner.owner.context.as_ptr()) }
                None
            }
        }
    }

    fn sanitize_exception(
        exception: &Value,
        phase: ErrorPhase,
        syntax_is_parse_error: bool,
    ) -> Error {
        let name = exception.property_string("name");
        let property_message = exception.property_string("message");
        let stack = exception.property_string("stack");
        let source = exception.property_string("fileName");
        let line = exception.property_u32("lineNumber");
        let column = exception.property_u32("columnNumber");
        let direct_message = if exception.kind() == ValueKind::String {
            exception
                .to_utf16()
                .ok()
                .map(|units| String::from_utf16_lossy(&units))
        } else {
            None
        };
        let message = property_message
            .or(direct_message)
            .unwrap_or_else(|| "JavaScript threw a non-Error value".to_owned());
        let kind = match name.as_deref() {
            Some("SyntaxError") if syntax_is_parse_error => ErrorKind::Syntax,
            Some("InternalError") if message.to_ascii_lowercase().contains("out of memory") => {
                ErrorKind::OutOfMemory
            }
            _ => ErrorKind::Exception,
        };
        let location = if source.is_some() || line.is_some() || column.is_some() {
            Some(SourceLocation {
                source,
                line,
                column,
            })
        } else {
            stack.as_deref().and_then(parse_stack_location)
        };
        Error {
            kind,
            phase,
            name,
            message,
            stack,
            location,
        }
    }

    fn parse_stack_location(stack: &str) -> Option<SourceLocation> {
        stack.lines().find_map(|line| {
            let line = line.trim();
            let candidate = line
                .rfind('(')
                .and_then(|open| line.strip_suffix(')').map(|closed| &closed[open + 1..]))
                .or_else(|| line.strip_prefix("at "))?;
            let (source_and_line, column) = candidate.rsplit_once(':')?;
            let (source, line) = source_and_line.rsplit_once(':')?;
            let line = line.parse().ok()?;
            let column = column.parse().ok()?;
            (!source.is_empty()).then(|| SourceLocation {
                source: Some(source.to_owned()),
                line: Some(line),
                column: Some(column),
            })
        })
    }

    fn hashbang_end(source: &str) -> usize {
        source
            .char_indices()
            .find_map(|(offset, character)| match character {
                '\r' => Some(
                    offset
                        + if source.as_bytes().get(offset + 1) == Some(&b'\n') {
                            2
                        } else {
                            1
                        },
                ),
                '\n' => Some(offset + 1),
                '\u{2028}' | '\u{2029}' => Some(offset + character.len_utf8()),
                _ => None,
            })
            .unwrap_or(source.len())
    }

    fn decode_cesu8(encoded: &[u8]) -> Result<Vec<u16>, &'static str> {
        let mut decoded = Vec::with_capacity(encoded.len());
        let mut offset = 0usize;
        while offset < encoded.len() {
            let first = encoded[offset];
            let (unit, width) = if first < 0x80 {
                (u16::from(first), 1)
            } else if first & 0xe0 == 0xc0 {
                if offset + 1 >= encoded.len() || encoded[offset + 1] & 0xc0 != 0x80 {
                    return Err("QuickJS returned malformed two-byte CESU-8");
                }
                let unit = (u16::from(first & 0x1f) << 6) | u16::from(encoded[offset + 1] & 0x3f);
                if unit < 0x80 {
                    return Err("QuickJS returned overlong CESU-8");
                }
                (unit, 2)
            } else if first & 0xf0 == 0xe0 {
                if offset + 2 >= encoded.len()
                    || encoded[offset + 1] & 0xc0 != 0x80
                    || encoded[offset + 2] & 0xc0 != 0x80
                {
                    return Err("QuickJS returned malformed three-byte CESU-8");
                }
                let unit = (u16::from(first & 0x0f) << 12)
                    | (u16::from(encoded[offset + 1] & 0x3f) << 6)
                    | u16::from(encoded[offset + 2] & 0x3f);
                if unit < 0x800 {
                    return Err("QuickJS returned overlong CESU-8");
                }
                (unit, 3)
            } else {
                return Err("QuickJS returned non-CESU-8 string data");
            };
            decoded.push(unit);
            offset += width;
        }
        Ok(decoded)
    }

    fn is_canonical_big_int(value: &str) -> bool {
        let digits = value.strip_prefix('-').unwrap_or(value);
        !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'))
            && value != "-0"
    }

    fn int_conversion_error(_: TryFromIntError) -> Error {
        Error::bridge(
            ErrorKind::InvalidInput,
            ErrorPhase::Evaluate,
            "line offset does not fit this platform",
        )
    }

    #[cfg(test)]
    mod tests {
        use std::sync::mpsc;
        use std::time::Instant;
        use std::{panic, thread};

        use super::*;

        const TEST_EXECUTION_TIMEOUT: Duration = Duration::from_millis(20);
        const TEST_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(3);

        fn timed_realm() -> Realm {
            Realm::with_options(RealmOptions {
                execution_timeout: Some(TEST_EXECUTION_TIMEOUT),
                ..RealmOptions::default()
            })
            .expect("timed realm should initialize")
        }

        fn run_with_watchdog<T: Send + 'static>(
            operation: impl FnOnce() -> T + Send + 'static,
        ) -> T {
            let (sender, receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                let outcome = panic::catch_unwind(panic::AssertUnwindSafe(operation));
                let _ = sender.send(outcome);
            });
            let outcome = receiver
                .recv_timeout(TEST_WATCHDOG_TIMEOUT)
                .unwrap_or_else(|error| panic!("QuickJS interrupt watchdog expired: {error}"));
            worker
                .join()
                .expect("watchdog worker should capture its own panic");
            match outcome {
                Ok(value) => value,
                Err(payload) => panic::resume_unwind(payload),
            }
        }

        fn run_with_external_interrupt(
            operation: impl FnOnce(&mut Realm, mpsc::SyncSender<InterruptHandle>) -> Error
            + Send
            + 'static,
        ) -> (ErrorKind, ErrorPhase, Option<f64>) {
            let (handle_sender, handle_receiver) = mpsc::sync_channel(1);
            let (result_sender, result_receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                let mut realm = Realm::new().expect("realm should initialize");
                let error = operation(&mut realm, handle_sender);
                let reused = realm
                    .evaluate(EvalSource::new("14 * 3"), EvalOptions::default())
                    .expect("realm should remain reusable")
                    .as_number();
                result_sender
                    .send((error.kind, error.phase, reused))
                    .expect("test should receive worker result");
            });
            let handle = handle_receiver
                .recv_timeout(TEST_WATCHDOG_TIMEOUT)
                .expect("worker should publish interrupt handle");
            let request_deadline = Instant::now() + TEST_WATCHDOG_TIMEOUT;
            while !handle.request_interrupt_if_running() {
                assert!(
                    Instant::now() < request_deadline,
                    "JavaScript operation never became interruptible"
                );
                thread::yield_now();
            }
            let result = result_receiver
                .recv_timeout(TEST_WATCHDOG_TIMEOUT)
                .expect("external interruption watchdog expired");
            worker.join().expect("worker should finish cleanly");
            assert!(!handle.request_interrupt_if_running());
            result
        }

        #[test]
        fn evaluates_and_calls_functions() {
            let mut realm = Realm::new().unwrap();
            let function = realm
                .evaluate(
                    EvalSource::new("(left, right) => left + right"),
                    EvalOptions::default(),
                )
                .unwrap();
            let left = realm.number(20.0).unwrap();
            let right = realm.number(22.0).unwrap();
            let result = realm.call(&function, None, &[left, right]).unwrap();

            assert_eq!(function.kind(), ValueKind::Function);
            assert_eq!(result.as_number(), Some(42.0));
        }

        #[test]
        fn times_out_infinite_evaluation_and_reuses_realm() {
            let (kind, phase, reused) = run_with_watchdog(|| {
                let mut realm = timed_realm();
                let error = realm
                    .evaluate(EvalSource::new("for (;;) {}"), EvalOptions::default())
                    .expect_err("infinite evaluation must time out");
                let reused = realm
                    .evaluate(EvalSource::new("21 * 2"), EvalOptions::default())
                    .expect("realm should remain reusable")
                    .as_number();
                (error.kind, error.phase, reused)
            });

            assert_eq!(kind, ErrorKind::ExecutionTimeout);
            assert_eq!(phase, ErrorPhase::Evaluate);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn times_out_infinite_call_and_reuses_realm() {
            let (kind, phase, reused) = run_with_watchdog(|| {
                let mut realm = timed_realm();
                let callable = realm
                    .evaluate(
                        EvalSource::new("() => { while (true) {} }"),
                        EvalOptions::default(),
                    )
                    .expect("callable should evaluate");
                let error = realm
                    .call(&callable, None, &[])
                    .expect_err("infinite call must time out");
                let reused = realm
                    .evaluate(EvalSource::new("6 * 7"), EvalOptions::default())
                    .expect("realm should remain reusable")
                    .as_number();
                (error.kind, error.phase, reused)
            });

            assert_eq!(kind, ErrorKind::ExecutionTimeout);
            assert_eq!(phase, ErrorPhase::Call);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn timeout_remains_active_while_sanitizing_exceptions() {
            let (kind, phase, reused) = run_with_watchdog(|| {
                let mut realm = timed_realm();
                let error = realm
                    .evaluate(
                        EvalSource::new("throw new Proxy({}, { get() { while (true) {} } })"),
                        EvalOptions::default(),
                    )
                    .expect_err("malicious exception accessors must time out");
                let reused = realm
                    .evaluate(EvalSource::new("40 + 2"), EvalOptions::default())
                    .expect("realm should remain reusable")
                    .as_number();
                (error.kind, error.phase, reused)
            });

            assert_eq!(kind, ErrorKind::ExecutionTimeout);
            assert_eq!(phase, ErrorPhase::Evaluate);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn times_out_an_infinite_pending_job() {
            let (kind, phase, reused) = run_with_watchdog(|| {
                let mut realm = timed_realm();
                realm
                    .evaluate(
                        EvalSource::new(
                            "Promise.resolve().then(() => { while (true) {} }); undefined",
                        ),
                        EvalOptions::default(),
                    )
                    .expect("job should be scheduled");
                let error = realm
                    .try_execute_pending_job()
                    .expect_err("infinite pending job must time out");
                let reused = realm
                    .evaluate(EvalSource::new("7 * 6"), EvalOptions::default())
                    .expect("realm should remain reusable")
                    .as_number();
                (error.kind, error.phase, reused)
            });

            assert_eq!(kind, ErrorKind::ExecutionTimeout);
            assert_eq!(phase, ErrorPhase::PendingJob);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn one_timeout_covers_an_entire_pending_job_drain() {
            let (kind, phase, reused) = run_with_watchdog(|| {
                let mut realm = timed_realm();
                realm
                    .evaluate(
                        EvalSource::new(
                            "globalThis.reschedule = () => { \
                             Promise.resolve().then(reschedule); \
                             }; reschedule()",
                        ),
                        EvalOptions::default(),
                    )
                    .expect("self-replenishing job chain should start");
                let error = realm
                    .drain_pending_jobs_up_to(usize::MAX)
                    .expect_err("the whole drain must share one deadline");
                let reused = realm
                    .evaluate(EvalSource::new("84 / 2"), EvalOptions::default())
                    .expect("realm should remain reusable")
                    .as_number();
                (error.kind, error.phase, reused)
            });

            assert_eq!(kind, ErrorKind::ExecutionTimeout);
            assert_eq!(phase, ErrorPhase::PendingJob);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn external_interrupt_is_scoped_to_the_active_generation() {
            let (kind, phase, reused) = run_with_external_interrupt(|realm, handle_sender| {
                handle_sender
                    .send(realm.interrupt_handle())
                    .expect("test should receive interrupt handle");
                realm
                    .evaluate(EvalSource::new("for (;;) {}"), EvalOptions::default())
                    .expect_err("host request must interrupt evaluation")
            });

            assert_eq!(kind, ErrorKind::Interrupted);
            assert_eq!(phase, ErrorPhase::Evaluate);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn external_interrupt_covers_calls_and_preserves_realm() {
            let (kind, phase, reused) = run_with_external_interrupt(|realm, handle_sender| {
                let callable = realm
                    .evaluate(
                        EvalSource::new("() => { while (true) {} }"),
                        EvalOptions::default(),
                    )
                    .expect("callable should evaluate");
                handle_sender
                    .send(realm.interrupt_handle())
                    .expect("test should receive interrupt handle");
                realm
                    .call(&callable, None, &[])
                    .expect_err("host request must interrupt call")
            });

            assert_eq!(kind, ErrorKind::Interrupted);
            assert_eq!(phase, ErrorPhase::Call);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn external_interrupt_covers_pending_jobs_and_preserves_realm() {
            let (kind, phase, reused) = run_with_external_interrupt(|realm, handle_sender| {
                realm
                    .evaluate(
                        EvalSource::new(
                            "Promise.resolve().then(() => { while (true) {} }); undefined",
                        ),
                        EvalOptions::default(),
                    )
                    .expect("job should be scheduled");
                handle_sender
                    .send(realm.interrupt_handle())
                    .expect("test should receive interrupt handle");
                realm
                    .try_execute_pending_job()
                    .expect_err("host request must interrupt pending job")
            });

            assert_eq!(kind, ErrorKind::Interrupted);
            assert_eq!(phase, ErrorPhase::PendingJob);
            assert_eq!(reused, Some(42.0));
        }

        #[test]
        fn idle_interrupt_requests_do_not_poison_later_execution() {
            fn assert_send_sync<T: Send + Sync>() {}

            assert_send_sync::<InterruptHandle>();
            let mut realm = Realm::new().expect("realm should initialize");
            let handle = realm.interrupt_handle();
            assert!(!handle.request_interrupt_if_running());
            let result = realm
                .evaluate(EvalSource::new("20 + 22"), EvalOptions::default())
                .expect("idle request must not affect evaluation");
            assert_eq!(result.as_number(), Some(42.0));
            drop(result);
            drop(realm);
            assert!(!handle.request_interrupt_if_running());
        }

        #[test]
        fn rejects_unrepresentable_execution_timeout() {
            let error = Realm::with_options(RealmOptions {
                execution_timeout: Some(Duration::MAX),
                ..RealmOptions::default()
            })
            .expect_err("an unrepresentable deadline must fail realm creation");

            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert_eq!(error.phase, ErrorPhase::CreateRealm);
        }

        #[test]
        fn round_trips_exact_utf16() {
            let realm = Realm::new().unwrap();
            let units = [0x0000, 0x0061, 0xd800, 0xdc00, 0xdfff, 0x20ac];
            let value = realm.string_utf16(&units).unwrap();

            assert_eq!(value.kind(), ValueKind::String);
            assert_eq!(value.to_utf16().unwrap(), units);
        }

        #[test]
        fn retains_runtime_after_realm_is_dropped() {
            let value = {
                let realm = Realm::new().unwrap();
                realm.string("still rooted").unwrap()
            };

            assert_eq!(
                value.to_utf16().unwrap(),
                "still rooted".encode_utf16().collect::<Vec<_>>()
            );
        }

        #[test]
        fn rejects_cross_realm_calls() {
            let mut first = Realm::new().unwrap();
            let second = Realm::new().unwrap();
            let function = first
                .evaluate(EvalSource::new("value => value"), EvalOptions::default())
                .unwrap();
            let foreign = second.number(1.0).unwrap();

            let error = first.call(&function, None, &[foreign]).unwrap_err();
            assert_eq!(error.kind, ErrorKind::WrongRealm);
        }

        #[test]
        fn drains_pending_jobs() {
            let mut realm = Realm::new().unwrap();
            realm
                .evaluate(
                    EvalSource::new(
                        "globalThis.answer = 0; Promise.resolve().then(() => answer = 42)",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            assert_eq!(realm.drain_pending_jobs().unwrap(), 1);
            let result = realm
                .evaluate(EvalSource::new("answer"), EvalOptions::default())
                .unwrap();
            assert_eq!(result.as_number(), Some(42.0));
        }

        #[test]
        fn reports_sanitized_source_location_with_offset() {
            let mut realm = Realm::new().unwrap();
            let error = realm
                .evaluate(
                    EvalSource {
                        text: "throw new Error('nope')",
                        name: Some("fixture.js"),
                        line_offset: 6,
                    },
                    EvalOptions::default(),
                )
                .unwrap_err();

            assert_eq!(error.kind, ErrorKind::Exception);
            assert_eq!(error.name.as_deref(), Some("Error"));
            assert_eq!(error.message, "nope");
            assert_eq!(
                error
                    .location
                    .as_ref()
                    .and_then(|location| location.source.as_deref()),
                Some("fixture.js")
            );
            assert_eq!(
                error.location.as_ref().and_then(|location| location.line),
                Some(7)
            );
        }

        #[test]
        fn distinguishes_parse_errors_from_thrown_syntax_errors() {
            let mut realm = Realm::new().unwrap();
            let parse_error = realm
                .evaluate(EvalSource::new("const = 1"), EvalOptions::default())
                .unwrap_err();
            let thrown_error = realm
                .evaluate(
                    EvalSource::new("throw new SyntaxError('runtime')"),
                    EvalOptions::default(),
                )
                .unwrap_err();

            assert_eq!(parse_error.kind, ErrorKind::Syntax);
            assert_eq!(
                thrown_error.kind,
                ErrorKind::Exception,
                "parse={parse_error:?}, thrown={thrown_error:?}"
            );
        }

        #[test]
        fn line_offset_preserves_hashbang_semantics() {
            for terminator in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
                let mut realm = Realm::new().unwrap();
                let text = format!("#!/usr/bin/env qjs{terminator}40 + 2");
                let result = realm
                    .evaluate(
                        EvalSource {
                            text: &text,
                            name: Some("hashbang.js"),
                            line_offset: 6,
                        },
                        EvalOptions::default(),
                    )
                    .unwrap();

                assert_eq!(result.as_number(), Some(42.0));
            }
        }

        #[test]
        fn arbitrary_big_int_is_canonical() {
            let mut realm = Realm::new().unwrap();
            let decimal = "1234567890123456789012345678901234567890";
            let value = realm.big_int_decimal(decimal).unwrap();

            assert_eq!(value.kind(), ValueKind::BigInt);
            assert_eq!(value.to_big_int_decimal().unwrap(), decimal);
            assert_eq!(
                realm.big_int_decimal("01").unwrap_err().kind,
                ErrorKind::InvalidInput
            );
        }

        #[test]
        fn reports_unhandled_promise_rejections_at_checkpoint() {
            let mut realm = Realm::new().unwrap();
            realm
                .evaluate(
                    EvalSource::new("void Promise.reject(new Error('unhandled'))"),
                    EvalOptions::default(),
                )
                .unwrap();

            let error = realm.drain_pending_jobs_up_to(8).unwrap_err();
            assert_eq!(error.phase, ErrorPhase::PendingJob);
            assert_eq!(error.name.as_deref(), Some("Error"));
            assert_eq!(error.message, "unhandled");
        }

        #[test]
        fn clears_rejections_handled_before_checkpoint_finishes() {
            let mut realm = Realm::new().unwrap();
            realm
                .evaluate(
                    EvalSource::new(
                        "const rejected = Promise.reject(new Error('handled')); \
                     Promise.resolve().then(() => rejected.catch(() => {}))",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            assert!(realm.drain_pending_jobs().unwrap() > 0);
        }

        #[test]
        fn preserves_multiple_unhandled_rejections() {
            let mut realm = Realm::new().unwrap();
            realm
                .evaluate(
                    EvalSource::new(
                        "void Promise.reject(new Error('first')); \
                     void Promise.reject(new Error('second'))",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            let first = realm.drain_pending_jobs_up_to(0).unwrap_err();
            let second = realm.drain_pending_jobs_up_to(0).unwrap_err();
            assert_eq!(first.message, "first");
            assert_eq!(second.message, "second");
        }

        #[test]
        fn bounded_drain_reports_remaining_jobs_precisely() {
            let mut realm = Realm::new().unwrap();
            realm
                .evaluate(
                    EvalSource::new(
                        "Promise.resolve().then(() => {}).then(() => {}).then(() => {})",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            let first = realm.drain_pending_jobs_up_to(1).unwrap();
            assert_eq!(first.executed, 1);
            assert!(first.jobs_remaining);
            assert!(realm.drain_pending_jobs().unwrap() > 0);
            assert!(!realm.has_pending_jobs());
        }

        fn number(realm: &mut Realm, source: &str) -> Option<f64> {
            realm
                .evaluate(EvalSource::new(source), EvalOptions::default())
                .expect("evaluation should succeed")
                .as_number()
        }

        #[test]
        fn a_host_function_is_callable_from_javascript() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("double", 1, |arguments| {
                    let HostValue::Number(value) = arguments[0] else {
                        return Err(HostFunctionError::new("expected a number"));
                    };
                    Ok(HostValue::Number(value * 2.0))
                })
                .unwrap();
            assert_eq!(number(&mut realm, "double(21)"), Some(42.0));
        }

        #[test]
        fn a_host_function_keeps_state_across_calls() {
            let mut realm = Realm::new().unwrap();
            let mut calls = 0.0;
            realm
                .define_global_function("tick", 0, move |_| {
                    calls += 1.0;
                    Ok(HostValue::Number(calls))
                })
                .unwrap();
            assert_eq!(number(&mut realm, "tick(); tick(); tick()"), Some(3.0));
        }

        #[test]
        fn every_primitive_crosses_the_host_boundary() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("describe", 1, |arguments| {
                    Ok(HostValue::String(match &arguments[0] {
                        HostValue::Undefined => "undefined".to_owned(),
                        HostValue::Null => "null".to_owned(),
                        HostValue::Boolean(value) => format!("boolean:{value}"),
                        HostValue::Number(value) => format!("number:{value}"),
                        HostValue::String(value) => format!("string:{value}"),
                        HostValue::Object(_) => "host-object".to_owned(),
                    }))
                })
                .unwrap();
            for (call, expected) in [
                ("describe(undefined)", "undefined"),
                ("describe(null)", "null"),
                ("describe(true)", "boolean:true"),
                ("describe(1.5)", "number:1.5"),
                ("describe('hi')", "string:hi"),
            ] {
                let value = realm
                    .evaluate(EvalSource::new(call), EvalOptions::default())
                    .unwrap();
                let units = value.to_utf16().unwrap();
                assert_eq!(String::from_utf16(&units).unwrap(), expected, "{call}");
            }
        }

        #[test]
        fn a_missing_argument_arrives_as_undefined() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("arity", 2, |arguments| {
                    #[allow(clippy::cast_precision_loss, reason = "an argument count is tiny")]
                    Ok(HostValue::Number(arguments.len() as f64))
                })
                .unwrap();
            assert_eq!(number(&mut realm, "arity()"), Some(0.0));
            assert_eq!(number(&mut realm, "arity(1, 2, 3)"), Some(3.0));
        }

        #[test]
        fn a_non_primitive_argument_is_rejected_rather_than_coerced() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("take", 1, |_| Ok(HostValue::Undefined))
                .unwrap();
            let error = realm
                .evaluate(EvalSource::new("take({})"), EvalOptions::default())
                .expect_err("an object argument");
            assert!(error.message.contains("bridge-owned"), "{error:?}");
        }

        #[test]
        fn a_host_error_becomes_a_catchable_javascript_exception() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("boom", 0, |_| {
                    Err(HostFunctionError::new("the host said no"))
                })
                .unwrap();
            let value = realm
                .evaluate(
                    EvalSource::new("(() => { try { boom(); return 'unreachable'; } catch (error) { return error.message; } })()"),
                    EvalOptions::default(),
                )
                .unwrap();
            let units = value.to_utf16().unwrap();
            assert_eq!(String::from_utf16(&units).unwrap(), "the host said no");
        }

        #[test]
        fn a_panicking_host_function_becomes_an_exception_not_an_unwind() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("explode", 0, |_| panic!("host bug"))
                .unwrap();
            let previous = panic::take_hook();
            panic::set_hook(Box::new(|_| {}));
            let error = realm.evaluate(EvalSource::new("explode()"), EvalOptions::default());
            panic::set_hook(previous);
            let error = error.expect_err("the panic should surface as an exception");
            assert!(error.message.contains("panicked"), "{error:?}");
        }

        #[test]
        fn a_panicking_host_function_does_not_poison_its_slot() {
            let mut realm = Realm::new().unwrap();
            let mut calls = 0.0;
            realm
                .define_global_function("flaky", 1, move |arguments| {
                    calls += 1.0;
                    if matches!(arguments[0], HostValue::Boolean(true)) {
                        panic!("host bug");
                    }
                    Ok(HostValue::Number(calls))
                })
                .unwrap();

            let previous = panic::take_hook();
            panic::set_hook(Box::new(|_| {}));
            let error = realm.evaluate(EvalSource::new("flaky(true)"), EvalOptions::default());
            panic::set_hook(previous);
            assert!(error.is_err());

            assert_eq!(number(&mut realm, "flaky(false)"), Some(2.0));
        }

        #[test]
        fn global_object_and_set_property_round_trip() {
            let mut realm = Realm::new().unwrap();
            let global = realm.global_object().unwrap();
            assert_eq!(global.kind(), ValueKind::Object);
            let value = realm.number(11.0).unwrap();
            realm.set_property(&global, "answer", &value).unwrap();
            assert_eq!(number(&mut realm, "answer"), Some(11.0));
        }

        #[test]
        fn set_property_rejects_a_value_from_another_realm() {
            let mut realm = Realm::new().unwrap();
            let other = Realm::new().unwrap();
            let global = realm.global_object().unwrap();
            let foreign = other.number(1.0).unwrap();
            let error = realm
                .set_property(&global, "x", &foreign)
                .expect_err("cross-realm value");
            assert_eq!(error.kind, ErrorKind::WrongRealm);
        }

        #[test]
        fn a_host_function_can_be_installed_on_an_ordinary_object() {
            let mut realm = Realm::new().unwrap();
            let namespace = realm
                .evaluate(
                    EvalSource::new("globalThis.lynx = {}; globalThis.lynx"),
                    EvalOptions::default(),
                )
                .unwrap();
            let function = realm
                .function("version", 0, |_| Ok(HostValue::Number(3.2)))
                .unwrap();
            realm
                .set_property(&namespace, "version", &function)
                .unwrap();
            assert_eq!(number(&mut realm, "lynx.version()"), Some(3.2));
        }

        #[test]
        fn a_host_function_reports_its_name_and_arity() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("named", 2, |_| Ok(HostValue::Undefined))
                .unwrap();
            assert_eq!(number(&mut realm, "named.length"), Some(2.0));
            let value = realm
                .evaluate(EvalSource::new("named.name"), EvalOptions::default())
                .unwrap();
            let units = value.to_utf16().unwrap();
            assert_eq!(String::from_utf16(&units).unwrap(), "named");
        }

        #[test]
        fn an_ill_formed_utf16_argument_is_rejected_at_the_boundary() {
            let mut realm = Realm::new().unwrap();
            realm
                .define_global_function("take", 1, |_| Ok(HostValue::Undefined))
                .unwrap();
            let error = realm
                .evaluate(EvalSource::new("take('\\uD800')"), EvalOptions::default())
                .expect_err("a lone surrogate");
            assert!(error.message.contains("ill-formed UTF-16"), "{error:?}");
        }

        #[test]
        fn a_property_name_with_a_nul_byte_is_rejected() {
            let mut realm = Realm::new().unwrap();
            let error = realm
                .function("bad\0name", 0, |_| Ok(HostValue::Undefined))
                .expect_err("a NUL in the name");
            assert_eq!(error.kind, ErrorKind::InvalidInput);
        }
    }
}

pub use implementation::{
    Error, ErrorKind, ErrorPhase, EvalOptions, EvalSource, HostFunctionError, HostFunctionOutput,
    HostObject, HostObjectReleaseQueue, HostValue, InterruptHandle, JobDrain, Realm, RealmOptions,
    SourceLocation, SourceType, Value, ValueKind, WeakValue,
};
