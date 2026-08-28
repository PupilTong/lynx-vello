//! A small, safe Rust boundary around the repository's pinned `QuickJS` source.
//!
//! The two `QuickJS` objects are the two types here. A [`Runtime`] is one
//! JavaScript heap, atom table, promise-job queue, and set of execution
//! limits. A [`Context`] is one realm on it — a global object and the modules
//! loaded into it — and a runtime can carry as many realms as the host wants,
//! all on the owning thread. Realms share everything the runtime owns and
//! nothing else: a [`Value`] never crosses between them, module *source* is
//! registered once on the runtime but compiled per realm, and native host
//! modules are installed per realm.
//!
//! Every C heap allocation compiled into this bridge is routed through Rust's
//! global allocator. Its C formatting calls use one private, allocator-free
//! formatter on native and Wasm. Both targets compile against the same narrow
//! standard-library declaration facade; unsupported `FILE` diagnostics are
//! removed instead of becoming a host ABI. A runtime deliberately omits
//! JavaScript shared-memory primitives (`Atomics` and `SharedArrayBuffer`);
//! this does not disable Rust or host-side synchronization. A realm may also
//! preload exact-name UTF-8 modules and Rust-backed native host modules into
//! its synchronous loader, inspect their namespaces, and inspect a
//! module-evaluation `Promise` after driving the runtime's pending-job queue;
//! module graph and resource policy remain its caller's responsibility.

#[allow(
    unsafe_code,
    reason = "this private module implements the allocator ABI used by the C translation units"
)]
mod allocator;
mod ffi;
#[allow(
    unsafe_code,
    reason = "this private module exports the Rust standard-library ABI used by the C translation units"
)]
mod platform_stdlib;
#[allow(
    unsafe_code,
    reason = "this private module exports the C ABI used by QuickJS's time hooks"
)]
mod platform_time;

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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;
    #[cfg(not(target_arch = "wasm32"))]
    use std::time::Instant;
    use std::{fmt, mem};

    use smallvec::SmallVec;
    #[cfg(target_arch = "wasm32")]
    use web_time::Instant;

    use super::ffi;

    const HOST_INLINE_ARGS: usize = 8;

    /// `QuickJS`'s "no such atom" sentinel, returned when interning fails.
    const JS_ATOM_NULL: u32 = 0;

    const JS_EVAL_TYPE_GLOBAL: i32 = 0;
    const JS_EVAL_TYPE_MODULE: i32 = 1;
    const JS_EVAL_FLAG_STRICT: i32 = 1 << 3;
    const JS_EVAL_FLAG_BACKTRACE_BARRIER: i32 = 1 << 6;
    const JS_EVAL_FLAG_ASYNC: i32 = 1 << 7;
    const QJS_EVAL_FAILURE_COMPILE: i32 = 1;
    const QJS_PROMISE_PENDING: i32 = 0;
    const QJS_PROMISE_FULFILLED: i32 = 1;
    const QJS_PROMISE_REJECTED: i32 = 2;

    static HOST_OWNER_CLASS_ID: OnceLock<u32> = OnceLock::new();

    /// Limits and timeout applied when a runtime is created.
    ///
    /// All three are `QuickJS` runtime settings, so they bound every realm
    /// on the runtime together.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct RuntimeOptions {
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
        CreateRuntime,
        CreateContext,
        RegisterModule,
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
        /// How many guards are alive. Only the outermost one arms and
        /// disarms: a host function that calls into a sibling realm runs
        /// under the deadline of the operation that reached it, and an
        /// interrupt raised anywhere inside fails every frame on the way out.
        depth: Cell<u32>,
        deadline: Cell<Option<Instant>>,
        reason: Cell<Option<InterruptReason>>,
    }

    fn deadline_from_timeout(
        timeout: Option<Duration>,
        now: impl FnOnce() -> Instant,
    ) -> Option<Instant> {
        timeout.map(|timeout| {
            let now = now();
            now.checked_add(timeout).unwrap_or(now)
        })
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
                depth: Cell::new(0),
                deadline: Cell::new(None),
                reason: Cell::new(None),
            }
        }

        fn begin(self: &Rc<Self>) -> ExecutionGuard {
            let depth = self.depth.get();
            self.depth.set(depth + 1);
            if depth > 0 {
                return ExecutionGuard {
                    state: Rc::clone(self),
                    armed: true,
                };
            }
            let mut generation = self.next_generation.get() + 1;
            if generation > MAX_INTERRUPT_GENERATION {
                generation = 1;
            }
            self.next_generation.set(generation);
            let token = generation << 1;
            let deadline = deadline_from_timeout(self.timeout, Instant::now);

            self.reason.set(None);
            self.deadline.set(deadline);
            self.active_token.set(token);
            self.shared.active.store(token, Ordering::Release);
            ExecutionGuard {
                state: Rc::clone(self),
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
            self.armed = false;
            let depth = self.state.depth.get() - 1;
            self.state.depth.set(depth);
            if depth > 0 {
                return;
            }
            let token = self.state.active_token.get();
            let active = self.state.shared.active.swap(0, Ordering::AcqRel);
            debug_assert!(
                active == token || active == token | INTERRUPT_REQUESTED,
                "interrupt generation changed during one synchronous execution"
            );
            self.state.active_token.set(0);
            self.state.deadline.set(None);
            self.state.reason.set(None);
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
        context: *mut ffi::QjsContext,
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
            _ => Err(HostFunctionError::new(
                "host functions accept undefined, null, Boolean, Number, and String arguments only",
            )),
        }
    }

    fn throw_host_error(context: *mut ffi::QjsContext, message: &str) {
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

    unsafe extern "C" fn host_dispatch(
        opaque: *mut c_void,
        context: *mut ffi::QjsContext,
        handler: *mut c_void,
        argument_count: usize,
        arguments: *const ffi::QjsHostArg,
        result: *mut ffi::QjsHostResult,
    ) -> i32 {
        let table = unsafe { &*opaque.cast::<HostTable>() };
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

        table.write_result(returned, unsafe { &mut *result });
        0
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

    /// A primitive a host function receives or returns.
    ///
    /// Owned, because both are values the boundary has just produced: an
    /// argument was decoded out of the realm, a return value was built by the
    /// host. Text is always well-formed UTF-8, which is what lets an outbound
    /// string take `JS_NewStringLen` directly.
    ///
    /// [`HostArgument`] is the borrowed counterpart, for the other direction:
    /// text the host already owns and only lends for the length of a call.
    #[derive(Clone, Debug, PartialEq)]
    #[non_exhaustive]
    pub enum HostValue {
        Undefined,
        Null,
        Boolean(bool),
        Number(f64),
        String(String),
    }

    /// A primitive the host passes into the realm, borrowing its text.
    ///
    /// Deliberately not [`HostValue`]. A caller of [`Context::call_member`]
    /// already owns the strings it is passing — an event's name and its JSON
    /// detail, held for the whole walk — and needs them only for the length
    /// of the call, so it lends them. Owning them here would mean a copy per
    /// argument per call.
    ///
    /// The alternative was to make [`HostValue`] itself reference-counted so
    /// both directions could share one type. Borrowing is strictly less work
    /// than any refcount, and a refcount would ride on every argument of
    /// every *inbound* call as well, where nothing is ever shared — a realm
    /// is owner-thread-bound, so an atomic one buys nothing at all. Two types
    /// is the honest shape: one direction owns what it just produced, the
    /// other lends what it already had.
    #[derive(Clone, Copy, Debug, PartialEq)]
    #[non_exhaustive]
    pub enum HostArgument<'a> {
        Undefined,
        Null,
        Boolean(bool),
        Number(f64),
        String(&'a str),
    }

    impl HostArgument<'_> {
        /// Fills in one C-ABI argument descriptor borrowing this value.
        fn describe(self, slot: &mut ffi::QjsHostArg) {
            slot.number = 0.0;
            slot.text = ptr::null();
            slot.text_len = 0;
            match self {
                Self::Undefined => slot.kind = ffi::HOST_ARG_UNDEFINED,
                Self::Null => slot.kind = ffi::HOST_ARG_NULL,
                Self::Boolean(value) => {
                    slot.kind = ffi::HOST_ARG_BOOLEAN;
                    slot.number = if value { 1.0 } else { 0.0 };
                }
                Self::Number(value) => {
                    slot.kind = ffi::HOST_ARG_NUMBER;
                    slot.number = value;
                }
                Self::String(value) => {
                    slot.kind = ffi::HOST_ARG_STRING;
                    slot.text = value.as_ptr();
                    slot.text_len = value.len();
                }
            }
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

    type HostHandler = Box<dyn FnMut(&[HostValue]) -> Result<HostValue, HostFunctionError>>;

    struct HostSlot {
        handler: RefCell<HostHandler>,
    }

    struct HostTable {
        pending_release: RefCell<Vec<*mut HostSlot>>,
        /// Keeps a returned string alive across the ABI return.
        ///
        /// The descriptor the trampoline reads borrows the host function's
        /// return value, which would otherwise be dropped the moment
        /// `host_dispatch` returns — so the string is moved here instead, and
        /// the bytes C reads are its own. Nothing is copied and nothing is
        /// re-encoded; the previous return value, already consumed, is
        /// released by the same write.
        return_text: RefCell<Option<String>>,
    }

    impl HostTable {
        fn new() -> Self {
            Self {
                pending_release: RefCell::new(Vec::new()),
                return_text: RefCell::new(None),
            }
        }

        fn write_result(&self, value: HostValue, out: &mut ffi::QjsHostResult) {
            out.number = 0.0;
            out.text = ptr::null();
            out.text_len = 0;
            match value {
                HostValue::Undefined => out.kind = ffi::HOST_ARG_UNDEFINED,
                HostValue::Null => out.kind = ffi::HOST_ARG_NULL,
                HostValue::Boolean(value) => {
                    out.kind = ffi::HOST_ARG_BOOLEAN;
                    out.number = if value { 1.0 } else { 0.0 };
                }
                HostValue::Number(value) => {
                    out.kind = ffi::HOST_ARG_NUMBER;
                    out.number = value;
                }
                HostValue::String(text) => {
                    let mut parked = self.return_text.borrow_mut();
                    let text = parked.insert(text);
                    out.kind = ffi::HOST_ARG_STRING;
                    out.text = text.as_ptr();
                    out.text_len = text.len();
                }
            }
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

    struct RuntimeInner {
        raw: NonNull<ffi::QjsRuntime>,
        interrupt: Rc<InterruptState>,
        hosts: Box<HostTable>,
    }

    impl RuntimeInner {
        fn reclaim(&self) {
            self.hosts.reclaim();
        }
    }

    impl Drop for RuntimeInner {
        fn drop(&mut self) {
            unsafe {
                ffi::qjs_runtime_set_interrupt_handler(self.raw.as_ptr(), None, ptr::null_mut());
                self.interrupt.shared.active.store(0, Ordering::Release);
                ffi::qjs_runtime_free(self.raw.as_ptr());
                self.hosts.reclaim();
            }
        }
    }

    /// An owner-thread-bound `QuickJS` runtime.
    ///
    /// One JavaScript heap, one atom table, one promise-job queue, and one
    /// set of execution limits — shared by every [`Context`] created on it.
    /// Nothing here is `Send`: a runtime and everything reached through it
    /// stay on the thread that built them.
    pub struct Runtime {
        inner: Rc<RuntimeInner>,
    }

    impl fmt::Debug for Runtime {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("Runtime")
                .field(
                    "live_handles",
                    &Rc::strong_count(&self.inner).saturating_sub(1),
                )
                .finish_non_exhaustive()
        }
    }

    impl Runtime {
        pub fn new() -> Result<Self, Error> {
            Self::with_options(RuntimeOptions::default())
        }

        pub fn with_options(options: RuntimeOptions) -> Result<Self, Error> {
            if options
                .execution_timeout
                .is_some_and(|timeout| Instant::now().checked_add(timeout).is_none())
            {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::CreateRuntime,
                    "execution timeout exceeds this platform's monotonic-clock range",
                ));
            }

            unsafe {
                let host_owner_class_id =
                    *HOST_OWNER_CLASS_ID.get_or_init(|| ffi::qjs_host_owner_class_id_new());
                let raw =
                    NonNull::new(ffi::qjs_runtime_new(host_owner_class_id)).ok_or_else(|| {
                        Error::bridge(
                            ErrorKind::OutOfMemory,
                            ErrorPhase::CreateRuntime,
                            "QuickJS could not allocate a runtime",
                        )
                    })?;
                if let Some(limit) = options.memory_limit {
                    ffi::qjs_runtime_set_memory_limit(raw.as_ptr(), limit);
                }
                if let Some(size) = options.max_stack_size {
                    ffi::qjs_runtime_set_max_stack_size(raw.as_ptr(), size);
                }
                let interrupt = Rc::new(InterruptState::new(options.execution_timeout));
                ffi::qjs_runtime_set_interrupt_handler(
                    raw.as_ptr(),
                    Some(interrupt_callback),
                    Rc::as_ptr(&interrupt).cast_mut().cast(),
                );
                let hosts = Box::new(HostTable::new());
                ffi::qjs_runtime_set_host_dispatch(
                    raw.as_ptr(),
                    Some(host_dispatch),
                    Some(host_release),
                    (&raw const *hosts).cast_mut().cast(),
                );
                Ok(Self {
                    inner: Rc::new(RuntimeInner {
                        raw,
                        interrupt,
                        hosts,
                    }),
                })
            }
        }

        /// Creates one realm on this runtime.
        ///
        /// A realm has its own global object, its own instance of every
        /// module it loads, and its own native modules. What it shares with
        /// its sibling realms is everything the runtime owns: the heap, the
        /// interned property names, the job queue, and the execution limits.
        /// [`Value`]s never cross between realms — passing one to the wrong
        /// context is [`ErrorKind::WrongRealm`].
        pub fn create_context(&self) -> Result<Context, Error> {
            self.inner.reclaim();
            let raw = NonNull::new(unsafe { ffi::qjs_context_new(self.inner.raw.as_ptr()) })
                .ok_or_else(|| {
                    Error::bridge(
                        ErrorKind::OutOfMemory,
                        ErrorPhase::CreateContext,
                        "QuickJS could not allocate a context",
                    )
                })?;
            Ok(Context {
                inner: Rc::new(ContextInner {
                    raw,
                    runtime: Rc::clone(&self.inner),
                }),
            })
        }

        #[must_use]
        pub fn interrupt_handle(&self) -> InterruptHandle {
            InterruptHandle {
                shared: Arc::clone(&self.inner.interrupt.shared),
            }
        }

        /// Adds one UTF-8 source module to this runtime's synchronous loader.
        ///
        /// Module names are exact after `QuickJS`'s normal normalization. A
        /// module must be registered before any entry that imports it is
        /// compiled, and a name cannot be replaced after registration.
        ///
        /// The source is registered once and compiled per realm: every
        /// [`Context`] that imports the name gets its own module instance,
        /// with its own bindings.
        pub fn register_module_source(&mut self, name: &str, source: &str) -> Result<(), Error> {
            self.inner.reclaim();
            if name.is_empty() {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module name is empty",
                ));
            }
            let name = CString::new(name).map_err(|_| {
                Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module name contains a NUL byte",
                )
            })?;
            let status = unsafe {
                ffi::qjs_runtime_add_module(
                    self.inner.raw.as_ptr(),
                    name.as_ptr(),
                    source.as_ptr(),
                    source.len(),
                )
            };
            match status {
                0 => Ok(()),
                -1 => Err(Error::bridge(
                    ErrorKind::OutOfMemory,
                    ErrorPhase::RegisterModule,
                    "QuickJS could not retain the module source",
                )),
                -2 => Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module name is already registered",
                )),
                _ => Err(Error::bridge(
                    ErrorKind::Engine,
                    ErrorPhase::RegisterModule,
                    "QuickJS returned an unknown module-registration status",
                )),
            }
        }

        /// Runs a full garbage collection and reclaims host closures.
        pub fn run_gc(&mut self) {
            unsafe { ffi::qjs_runtime_run_gc(self.inner.raw.as_ptr()) };
            self.inner.reclaim();
        }

        #[must_use]
        pub fn has_pending_jobs(&self) -> bool {
            unsafe { ffi::qjs_has_pending_job(self.inner.raw.as_ptr()) != 0 }
        }

        pub fn try_execute_pending_job(&mut self) -> Result<bool, Error> {
            self.inner.reclaim();
            let guard = self.inner.interrupt.begin();
            let result = self.try_execute_pending_job_inner();
            guard.finish(result, ErrorPhase::PendingJob)
        }

        pub fn drain_pending_jobs(&mut self) -> Result<usize, Error> {
            self.inner.reclaim();
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
            self.inner.reclaim();
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

        /// Runs one job from the shared queue, whichever realm queued it.
        ///
        /// A job's exception is captured in the realm it ran in, which is not
        /// necessarily one the caller holds a handle to.
        fn try_execute_pending_job_inner(&mut self) -> Result<bool, Error> {
            let mut job_context = ptr::null_mut();
            let status = unsafe {
                ffi::qjs_execute_pending_job(self.inner.raw.as_ptr(), &raw mut job_context)
            };
            match status {
                0 => self.take_unhandled_rejection().map_or(Ok(false), Err),
                value if value > 0 => Ok(true),
                _ => Err(NonNull::new(job_context).map_or_else(
                    || {
                        Error::bridge(
                            ErrorKind::Engine,
                            ErrorPhase::PendingJob,
                            "a promise job failed in a realm the host has already released",
                        )
                    },
                    |context| capture_exception(context, ErrorPhase::PendingJob),
                )),
            }
        }

        fn take_unhandled_rejection(&self) -> Option<Error> {
            let mut context = ptr::null_mut();
            let mut reason = ptr::null_mut();
            let status = unsafe {
                ffi::qjs_take_unhandled_rejection(
                    self.inner.raw.as_ptr(),
                    &raw mut context,
                    &raw mut reason,
                )
            };
            if status == ffi::REJECTION_NONE {
                return None;
            }
            if status != ffi::REJECTION_TAKEN {
                return Some(Error::bridge(
                    ErrorKind::OutOfMemory,
                    ErrorPhase::PendingJob,
                    "QuickJS could not record an unhandled promise rejection",
                ));
            }
            let context = NonNull::new(context)?;
            // Boxing the reason is the one step that can fail, and it leaves
            // its own exception pending in that realm.
            let Some(reason) = NonNull::new(reason) else {
                return Some(capture_exception(context, ErrorPhase::PendingJob));
            };
            Some(sanitize_exception(
                &RawValue::new(context, reason),
                ErrorPhase::PendingJob,
                false,
            ))
        }
    }

    struct ContextInner {
        raw: NonNull<ffi::QjsContext>,
        runtime: Rc<RuntimeInner>,
    }

    impl Drop for ContextInner {
        fn drop(&mut self) {
            unsafe { ffi::qjs_context_free(self.raw.as_ptr()) };
            self.runtime.reclaim();
        }
    }

    /// One realm on a [`Runtime`]: a global object, the values reachable from
    /// it, and the modules loaded into it.
    pub struct Context {
        inner: Rc<ContextInner>,
    }

    impl fmt::Debug for Context {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("Context")
                .field(
                    "live_handles",
                    &Rc::strong_count(&self.inner).saturating_sub(1),
                )
                .finish_non_exhaustive()
        }
    }

    impl Context {
        fn reclaim(&self) {
            self.inner.runtime.reclaim();
        }

        fn raw(&self) -> NonNull<ffi::QjsContext> {
            self.inner.raw
        }

        fn begin(&self) -> ExecutionGuard {
            self.inner.runtime.interrupt.begin()
        }

        pub fn global_object(&self) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_global_object(context)
            })
        }

        /// Adds one Rust-backed named export to a native ESM module in this
        /// realm.
        ///
        /// The module and all of its exports must be registered before the
        /// graph first loads that specifier. Native modules are per-realm, so
        /// a sibling realm sees this specifier only if it registers its own;
        /// source modules and native modules share one exact-name namespace
        /// and neither kind can replace the other.
        pub fn register_host_module_function<F>(
            &mut self,
            module_name: &str,
            export_name: &str,
            arity: u32,
            handler: F,
        ) -> Result<(), Error>
        where
            F: FnMut(&[HostValue]) -> Result<HostValue, HostFunctionError> + 'static,
        {
            self.reclaim();
            if module_name.is_empty() {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module name is empty",
                ));
            }
            if export_name.is_empty() {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module export name is empty",
                ));
            }
            let module_name = CString::new(module_name).map_err(|_| {
                Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module name contains a NUL byte",
                )
            })?;
            let export_name_c = CString::new(export_name).map_err(|_| {
                Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module export name contains a NUL byte",
                )
            })?;
            let function = self
                .function(export_name, arity, handler)
                .map_err(|mut error| {
                    error.phase = ErrorPhase::RegisterModule;
                    error
                })?;
            let status = unsafe {
                ffi::qjs_context_add_host_module_export(
                    self.raw().as_ptr(),
                    module_name.as_ptr(),
                    export_name_c.as_ptr(),
                    function.inner.value.raw.as_ptr(),
                )
            };
            match status {
                0 => Ok(()),
                -1 => Err(Error::bridge(
                    ErrorKind::OutOfMemory,
                    ErrorPhase::RegisterModule,
                    "QuickJS could not retain the native module export",
                )),
                -2 => Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "module name is already registered as a source module",
                )),
                -3 => Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "native module export name is already registered",
                )),
                -4 => Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::RegisterModule,
                    "native module was already loaded",
                )),
                _ => Err(Error::bridge(
                    ErrorKind::Engine,
                    ErrorPhase::RegisterModule,
                    "QuickJS returned an unknown native-module registration status",
                )),
            }
        }

        /// Returns the namespace object of a module this realm has linked.
        pub fn module_namespace(&mut self, name: &str) -> Result<Value, Error> {
            self.reclaim();
            if name.is_empty() {
                return Err(Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::ConstructValue,
                    "module name is empty",
                ));
            }
            let name = CString::new(name).map_err(|_| {
                Error::bridge(
                    ErrorKind::InvalidInput,
                    ErrorPhase::ConstructValue,
                    "module name contains a NUL byte",
                )
            })?;
            let guard = self.begin();
            let raw = unsafe { ffi::qjs_module_namespace(self.raw().as_ptr(), name.as_ptr()) };
            let result = self.value_or_exception(raw, ErrorPhase::ConstructValue);
            guard.finish(result, ErrorPhase::ConstructValue)
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
            let guard = self.begin();
            let status = unsafe {
                ffi::qjs_set_property(
                    self.raw().as_ptr(),
                    target.inner.value.raw.as_ptr(),
                    name.as_ptr(),
                    value.inner.value.raw.as_ptr(),
                )
            };
            let result = if status < 0 {
                Err(capture_exception(self.raw(), ErrorPhase::ConstructValue))
            } else {
                Ok(())
            };
            guard.finish(result, ErrorPhase::ConstructValue)
        }

        /// Reads a named property, enforcing realm affinity and interrupts.
        ///
        /// An absent property is `undefined`, not an error: a host looking for
        /// a callback the realm may not have installed asks for it and checks
        /// what came back.
        pub fn property(&mut self, target: &Value, name: &str) -> Result<Value, Error> {
            self.reclaim();
            self.ensure_affinity(target, ErrorPhase::ConstructValue)?;
            let name = property_name(name)?;
            let guard = self.begin();
            let raw = unsafe {
                ffi::qjs_get_property(
                    self.raw().as_ptr(),
                    target.inner.value.raw.as_ptr(),
                    name.as_ptr(),
                )
            };
            let result = self.value_or_exception(raw, ErrorPhase::ConstructValue);
            guard.finish(result, ErrorPhase::ConstructValue)
        }

        /// Creates a JavaScript callable backed by a Rust closure.
        pub fn function<F>(&mut self, name: &str, arity: u32, handler: F) -> Result<Value, Error>
        where
            F: FnMut(&[HostValue]) -> Result<HostValue, HostFunctionError> + 'static,
        {
            self.reclaim();
            let name = property_name(name)?;
            let arity = i32::try_from(arity).map_err(int_conversion_error)?;
            let slot = Box::into_raw(Box::new(HostSlot {
                handler: RefCell::new(Box::new(handler)),
            }));
            let raw = unsafe {
                ffi::qjs_new_host_function(self.raw().as_ptr(), name.as_ptr(), arity, slot.cast())
            };
            match self.value_or_exception(raw, ErrorPhase::ConstructValue) {
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

        /// Constructs a JavaScript string from well-formed text.
        ///
        /// A Rust `str` is always well-formed UTF-8, so this hands the bytes
        /// straight to `QuickJS`'s own decoder — one pass, no host-side
        /// allocation, and a `memcpy` for the ASCII case.
        pub fn string(&self, value: &str) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                ffi::qjs_new_string_utf8(context, value.as_ptr(), value.len())
            })
        }

        /// Constructs a JavaScript string from UTF-16 code units, including
        /// ill-formed ones.
        ///
        /// Well-formed units transcode to UTF-8 and take the same path as
        /// [`Self::string`]. Only an unpaired surrogate — which has no UTF-8
        /// spelling at all — falls back to the escape-and-parse path that
        /// preserves it.
        pub fn string_utf16(&self, units: &[u16]) -> Result<Value, Error> {
            match String::from_utf16(units) {
                Ok(text) => self.string(&text),
                Err(_) => self.construct(ErrorPhase::ConstructValue, |context| unsafe {
                    ffi::qjs_new_string_utf16(context, units.as_ptr(), units.len())
                }),
            }
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
            let guard = self.begin();
            let raw = unsafe {
                ffi::qjs_eval(
                    self.raw().as_ptr(),
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
                    capture_exception_with_syntax(
                        self.raw(),
                        ErrorPhase::Evaluate,
                        failure_stage == QJS_EVAL_FAILURE_COMPILE,
                    )
                });
            guard.finish(result, ErrorPhase::Evaluate)
        }

        /// Returns a settled promise's result, or `None` while it is pending.
        ///
        /// A rejected promise is returned as a sanitized JavaScript error.
        pub fn settled_promise_result(&mut self, promise: &Value) -> Result<Option<Value>, Error> {
            self.reclaim();
            self.ensure_affinity(promise, ErrorPhase::Evaluate)?;
            let guard = self.begin();
            let result = match unsafe {
                ffi::qjs_value_promise_state(self.raw().as_ptr(), promise.inner.value.raw.as_ptr())
            } {
                QJS_PROMISE_PENDING => Ok(None),
                state @ (QJS_PROMISE_FULFILLED | QJS_PROMISE_REJECTED) => {
                    let raw = unsafe {
                        ffi::qjs_value_promise_result(
                            self.raw().as_ptr(),
                            promise.inner.value.raw.as_ptr(),
                        )
                    };
                    let value = self.value_or_exception(raw, ErrorPhase::Evaluate)?;
                    if state == QJS_PROMISE_REJECTED {
                        Err(sanitize_exception(
                            &value.inner.value,
                            ErrorPhase::Evaluate,
                            false,
                        ))
                    } else {
                        Ok(Some(value))
                    }
                }
                _ => Err(Error::bridge(
                    ErrorKind::TypeMismatch,
                    ErrorPhase::Evaluate,
                    "module evaluation did not return a Promise",
                )),
            };
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
                .map(|value| value.inner.value.raw.as_ptr().cast_const())
                .collect();
            let guard = self.begin();
            let raw = unsafe {
                ffi::qjs_call(
                    self.raw().as_ptr(),
                    callable.inner.value.raw.as_ptr(),
                    this_value.map_or(ptr::null(), |value| value.inner.value.raw.as_ptr()),
                    arguments.len(),
                    raw_arguments.as_ptr(),
                )
            };
            let result = self.value_or_exception(raw, ErrorPhase::Call);
            guard.finish(result, ErrorPhase::Call)
        }

        /// Interns a property name so repeated lookups cost no string work.
        ///
        /// `QuickJS` resolves a property by atom; a name-keyed lookup hashes
        /// and interns the name on every call. A host that calls the same
        /// member every event interns it once instead.
        ///
        /// Atoms belong to the runtime, so the returned [`Member`] resolves a
        /// property in every realm on this context's runtime — intern the
        /// name once, call it in as many realms as there are.
        pub fn member(&mut self, name: &str) -> Result<Member, Error> {
            self.reclaim();
            let atom = unsafe { ffi::qjs_atom_new(self.raw().as_ptr(), name.as_ptr(), name.len()) };
            if atom == JS_ATOM_NULL {
                return Err(capture_exception(self.raw(), ErrorPhase::ConstructValue));
            }
            Ok(Member {
                atom,
                owner: Rc::clone(&self.inner.runtime),
            })
        }

        /// Calls `target[member]` with primitive arguments, in one crossing.
        ///
        /// The arguments become `JSValue`s in a stack array inside the ABI,
        /// so a call allocates nothing per argument — no rooted [`Value`], no
        /// boxed `QuickJS` value, no UTF-16 buffer, and no copy of text the
        /// caller already holds.
        ///
        /// A target with no callable under that name is
        /// [`CallOutcome::MemberAbsent`], not an error: a realm that
        /// published no such member is a realm that has nothing to say.
        ///
        /// The member runs with `this` undefined, as [`Self::call`] with no
        /// receiver does — the target names where to look the callable up,
        /// not what to bind it to.
        pub fn call_member(
            &mut self,
            target: &Value,
            member: &Member,
            arguments: &[HostArgument<'_>],
        ) -> Result<CallOutcome, Error> {
            self.reclaim();
            self.ensure_affinity(target, ErrorPhase::Call)?;
            if !Rc::ptr_eq(&self.inner.runtime, &member.owner) {
                return Err(Error::bridge(
                    ErrorKind::WrongRealm,
                    ErrorPhase::Call,
                    "member name belongs to a different QuickJS runtime",
                ));
            }
            let mut described: SmallVec<[ffi::QjsHostArg; HOST_INLINE_ARGS]> =
                SmallVec::with_capacity(arguments.len());
            for argument in arguments {
                let mut slot = ffi::QjsHostArg {
                    kind: ffi::HOST_ARG_UNDEFINED,
                    number: 0.0,
                    text: ptr::null(),
                    text_len: 0,
                };
                argument.describe(&mut slot);
                described.push(slot);
            }

            let guard = self.begin();
            let mut raw = ptr::null_mut();
            let status = unsafe {
                ffi::qjs_call_member(
                    self.raw().as_ptr(),
                    target.inner.value.raw.as_ptr(),
                    member.atom,
                    described.len(),
                    described.as_ptr(),
                    &raw mut raw,
                )
            };
            // The descriptors borrow `arguments`, which outlives the call.
            drop(described);
            let result = match status {
                0 => self
                    .value_or_exception(raw, ErrorPhase::Call)
                    .map(CallOutcome::Called),
                1 => Ok(CallOutcome::MemberAbsent),
                _ => Err(capture_exception(self.raw(), ErrorPhase::Call)),
            };
            guard.finish(result, ErrorPhase::Call)
        }

        fn construct(
            &self,
            phase: ErrorPhase,
            constructor: impl FnOnce(*mut ffi::QjsContext) -> *mut ffi::QjsValue,
        ) -> Result<Value, Error> {
            let raw = constructor(self.raw().as_ptr());
            self.value_or_exception(raw, phase)
        }

        fn value_or_exception(
            &self,
            raw: *mut ffi::QjsValue,
            phase: ErrorPhase,
        ) -> Result<Value, Error> {
            NonNull::new(raw)
                .map(|raw| Value::from_raw(Rc::clone(&self.inner), raw))
                .ok_or_else(|| capture_exception(self.raw(), phase))
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
    }

    /// An interned property name, owned by the runtime that interned it.
    ///
    /// `QuickJS` interns atoms into the runtime, not the context, so one
    /// [`Member`] serves every realm on that runtime.
    pub struct Member {
        atom: u32,
        owner: Rc<RuntimeInner>,
    }

    impl fmt::Debug for Member {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.debug_struct("Member").finish_non_exhaustive()
        }
    }

    impl Drop for Member {
        fn drop(&mut self) {
            unsafe { ffi::qjs_atom_free(self.owner.raw.as_ptr(), self.atom) }
        }
    }

    /// What [`Context::call_member`] found where it looked.
    ///
    /// Deliberately closed: a lookup either found a callable and ran it or it
    /// did not, and a caller that has to handle both has handled everything.
    #[derive(Debug)]
    pub enum CallOutcome {
        /// The member ran and returned this value.
        Called(Value),
        /// The target has no callable under that name.
        MemberAbsent,
    }

    /// A `QuickJS` value and the realm pointer it takes to read one.
    ///
    /// Reading a value needs nothing but these two pointers, which is what
    /// lets an exception be sanitized straight out of a realm the host holds
    /// no [`Context`] for — the realm a failed promise job ran in.
    struct RawValue {
        context: NonNull<ffi::QjsContext>,
        raw: NonNull<ffi::QjsValue>,
    }

    impl Drop for RawValue {
        fn drop(&mut self) {
            unsafe { ffi::qjs_value_free(self.context.as_ptr(), self.raw.as_ptr()) }
        }
    }

    impl RawValue {
        const fn new(context: NonNull<ffi::QjsContext>, raw: NonNull<ffi::QjsValue>) -> Self {
            Self { context, raw }
        }

        fn kind(&self) -> ValueKind {
            match unsafe { ffi::qjs_value_kind(self.context.as_ptr(), self.raw.as_ptr()) } {
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

        fn as_boolean(&self) -> Option<bool> {
            if self.kind() != ValueKind::Boolean {
                return None;
            }
            let mut result = 0;
            let status = unsafe {
                ffi::qjs_value_get_boolean(
                    self.context.as_ptr(),
                    self.raw.as_ptr(),
                    &raw mut result,
                )
            };
            (status == 0).then_some(result != 0)
        }

        fn as_number(&self) -> Option<f64> {
            if self.kind() != ValueKind::Number {
                return None;
            }
            let mut result = 0.0;
            let status = unsafe {
                ffi::qjs_value_get_number(self.context.as_ptr(), self.raw.as_ptr(), &raw mut result)
            };
            (status == 0).then_some(result)
        }

        fn to_utf16_coerced(&self) -> Result<Vec<u16>, Error> {
            raw_to_utf16(self.context.as_ptr(), self.raw.as_ptr())
        }

        fn property(&self, name: &str) -> Option<Self> {
            let name = CString::new(name).ok()?;
            let raw = unsafe {
                ffi::qjs_get_property(self.context.as_ptr(), self.raw.as_ptr(), name.as_ptr())
            };
            if let Some(raw) = NonNull::new(raw) {
                Some(Self::new(self.context, raw))
            } else {
                unsafe { ffi::qjs_discard_exception(self.context.as_ptr()) }
                None
            }
        }

        fn property_string(&self, name: &str) -> Option<String> {
            let property = self.property(name)?;
            if property.kind() != ValueKind::String {
                return None;
            }
            String::from_utf16(&property.to_utf16_coerced().ok()?).ok()
        }

        fn property_u32(&self, name: &str) -> Option<u32> {
            let number = self.property(name)?.as_number()?;
            if number.is_finite() && number >= 0.0 && number <= f64::from(u32::MAX) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(number as u32)
            } else {
                None
            }
        }
    }

    struct ValueInner {
        // Declared before `owner` so the value is released while its realm is
        // still alive: fields drop in declaration order.
        value: RawValue,
        owner: Rc<ContextInner>,
    }

    /// Rooted JavaScript value tied to its owning realm.
    #[derive(Clone)]
    pub struct Value {
        inner: Rc<ValueInner>,
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
        fn from_raw(owner: Rc<ContextInner>, raw: NonNull<ffi::QjsValue>) -> Self {
            Self {
                inner: Rc::new(ValueInner {
                    value: RawValue::new(owner.raw, raw),
                    owner,
                }),
            }
        }

        #[must_use]
        pub fn kind(&self) -> ValueKind {
            self.inner.value.kind()
        }

        #[must_use]
        pub fn as_boolean(&self) -> Option<bool> {
            self.inner.value.as_boolean()
        }

        #[must_use]
        pub fn as_number(&self) -> Option<f64> {
            self.inner.value.as_number()
        }

        pub fn to_utf16(&self) -> Result<Vec<u16>, Error> {
            if self.kind() != ValueKind::String {
                return Err(Error::bridge(
                    ErrorKind::TypeMismatch,
                    ErrorPhase::ConvertValue,
                    "value is not a String",
                ));
            }
            self.inner.value.to_utf16_coerced()
        }

        pub fn to_big_int_decimal(&self) -> Result<String, Error> {
            if self.kind() != ValueKind::BigInt {
                return Err(Error::bridge(
                    ErrorKind::TypeMismatch,
                    ErrorPhase::ConvertValue,
                    "value is not a BigInt",
                ));
            }
            let units = self.inner.value.to_utf16_coerced()?;
            String::from_utf16(&units).map_err(|_| {
                Error::bridge(
                    ErrorKind::Engine,
                    ErrorPhase::ConvertValue,
                    "QuickJS produced non-Unicode BigInt text",
                )
            })
        }
    }

    fn capture_exception(context: NonNull<ffi::QjsContext>, phase: ErrorPhase) -> Error {
        capture_exception_with_syntax(context, phase, false)
    }

    fn capture_exception_with_syntax(
        context: NonNull<ffi::QjsContext>,
        phase: ErrorPhase,
        syntax_is_parse_error: bool,
    ) -> Error {
        let raw = unsafe { ffi::qjs_take_exception(context.as_ptr()) };
        let Some(raw) = NonNull::new(raw) else {
            return Error::bridge(
                ErrorKind::OutOfMemory,
                phase,
                "could not allocate a box for the QuickJS exception",
            );
        };
        sanitize_exception(&RawValue::new(context, raw), phase, syntax_is_parse_error)
    }

    fn sanitize_exception(
        exception: &RawValue,
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
                .to_utf16_coerced()
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
        #[cfg(not(target_arch = "wasm32"))]
        use std::time::Instant;
        use std::{panic, thread};

        #[cfg(target_arch = "wasm32")]
        use web_time::Instant;

        use super::*;

        const TEST_EXECUTION_TIMEOUT: Duration = Duration::from_millis(20);
        const TEST_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(3);

        /// A realm whose runtime nothing else in the test needs to name.
        fn single_realm() -> Context {
            Runtime::new()
                .expect("runtime should initialize")
                .create_context()
                .expect("realm should initialize")
        }

        fn runtime_and_realm() -> (Runtime, Context) {
            let runtime = Runtime::new().expect("runtime should initialize");
            let realm = runtime.create_context().expect("realm should initialize");
            (runtime, realm)
        }

        fn timed_realm() -> (Runtime, Context) {
            let runtime = Runtime::with_options(RuntimeOptions {
                execution_timeout: Some(TEST_EXECUTION_TIMEOUT),
                ..RuntimeOptions::default()
            })
            .expect("timed runtime should initialize");
            let realm = runtime.create_context().expect("realm should initialize");
            (runtime, realm)
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
            operation: impl FnOnce(
                &mut Runtime,
                &mut Context,
                mpsc::SyncSender<InterruptHandle>,
            ) -> Error
            + Send
            + 'static,
        ) -> (ErrorKind, ErrorPhase, Option<f64>) {
            let (handle_sender, handle_receiver) = mpsc::sync_channel(1);
            let (result_sender, result_receiver) = mpsc::sync_channel(1);
            let worker = thread::spawn(move || {
                let (mut runtime, mut realm) = runtime_and_realm();
                let error = operation(&mut runtime, &mut realm, handle_sender);
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
            let mut realm = single_realm();
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
        fn preloaded_modules_support_dynamic_import_and_top_level_await() {
            let (mut runtime, mut realm) = runtime_and_realm();
            runtime
                .register_module_source("bobcat:answer", "export const answer = 42;")
                .unwrap();
            runtime
                .register_module_source(
                    "app:///entry.js",
                    "import { answer } from 'bobcat:answer';\n\
                     globalThis.answer = await Promise.resolve(answer);",
                )
                .unwrap();

            let evaluation = realm
                .evaluate(
                    EvalSource {
                        text: "await import('app:///entry.js');",
                        name: Some("bobcat:boot"),
                        line_offset: 0,
                    },
                    EvalOptions {
                        source_type: SourceType::Module,
                        ..EvalOptions::default()
                    },
                )
                .expect("boot module should start");
            runtime
                .drain_pending_jobs()
                .expect("module jobs should settle");
            assert!(
                realm
                    .settled_promise_result(&evaluation)
                    .expect("module evaluation should fulfill")
                    .is_some()
            );
            let answer = realm
                .evaluate(EvalSource::new("globalThis.answer"), EvalOptions::default())
                .unwrap();
            assert_eq!(answer.as_number(), Some(42.0));
        }

        #[test]
        fn preloaded_module_names_are_unique() {
            let mut runtime = Runtime::new().expect("runtime should initialize");
            runtime
                .register_module_source("bobcat:element", "export {};")
                .unwrap();
            let error = runtime
                .register_module_source("bobcat:element", "export {};")
                .expect_err("a module name must not be replaced");

            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert_eq!(error.phase, ErrorPhase::RegisterModule);
        }

        #[test]
        fn native_host_modules_export_rust_functions_without_globals() {
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .register_host_module_function("bobcat-internal:host", "add", 2, |arguments| {
                    let left = match arguments.first() {
                        Some(HostValue::Number(value)) => *value,
                        _ => return Err(HostFunctionError::new("left must be a number")),
                    };
                    let right = match arguments.get(1) {
                        Some(HostValue::Number(value)) => *value,
                        _ => return Err(HostFunctionError::new("right must be a number")),
                    };
                    Ok(HostValue::Number(left + right))
                })
                .unwrap();
            runtime
                .register_module_source(
                    "app:///entry.js",
                    "import { add } from 'bobcat-internal:host';\n\
                     export function increment(value) { return add(value, 1); }",
                )
                .unwrap();

            let evaluation = realm
                .evaluate(
                    EvalSource {
                        text: "await import('app:///entry.js');",
                        name: Some("bobcat:boot"),
                        line_offset: 0,
                    },
                    EvalOptions {
                        source_type: SourceType::Module,
                        ..EvalOptions::default()
                    },
                )
                .expect("boot module should start");
            runtime
                .drain_pending_jobs()
                .expect("module jobs should settle");
            assert!(
                realm
                    .settled_promise_result(&evaluation)
                    .expect("module evaluation should fulfill")
                    .is_some()
            );

            let namespace = realm
                .module_namespace("app:///entry.js")
                .expect("loaded module namespace");
            let increment = realm.property(&namespace, "increment").unwrap();
            let input = realm.number(41.0).unwrap();
            let result = realm.call(&increment, None, &[input]).unwrap();
            assert_eq!(result.as_number(), Some(42.0));

            let host_namespace = realm
                .module_namespace("bobcat-internal:host")
                .expect("loaded native module namespace");
            let add = realm.property(&host_namespace, "add").unwrap();
            let left = realm.number(20.0).unwrap();
            let right = realm.number(22.0).unwrap();
            assert_eq!(
                realm.call(&add, None, &[left, right]).unwrap().as_number(),
                Some(42.0)
            );

            let late_export = realm
                .register_host_module_function("bobcat-internal:host", "late", 0, |_| {
                    Ok(HostValue::Undefined)
                })
                .expect_err("a loaded native module cannot gain an export");
            assert_eq!(late_export.kind, ErrorKind::InvalidInput);
            assert_eq!(late_export.phase, ErrorPhase::RegisterModule);

            let leaked = realm
                .evaluate(
                    EvalSource::new("typeof globalThis.add"),
                    EvalOptions::default(),
                )
                .unwrap();
            assert_eq!(
                String::from_utf16(&leaked.to_utf16().unwrap()).unwrap(),
                "undefined"
            );
        }

        #[test]
        fn native_module_exports_are_unique_and_cannot_collide_with_source() {
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .register_host_module_function("bobcat-internal:host", "call", 0, |_| {
                    Ok(HostValue::Undefined)
                })
                .unwrap();
            let duplicate = realm
                .register_host_module_function("bobcat-internal:host", "call", 0, |_| {
                    Ok(HostValue::Undefined)
                })
                .expect_err("an export must not be replaced");
            assert_eq!(duplicate.kind, ErrorKind::InvalidInput);
            assert_eq!(duplicate.phase, ErrorPhase::RegisterModule);

            let collision = runtime
                .register_module_source("bobcat-internal:host", "export {};")
                .expect_err("source and native modules share one namespace");
            assert_eq!(collision.kind, ErrorKind::InvalidInput);
            assert_eq!(collision.phase, ErrorPhase::RegisterModule);

            runtime
                .register_module_source("bobcat:source", "export {};")
                .unwrap();
            let reverse_collision = realm
                .register_host_module_function("bobcat:source", "call", 0, |_| {
                    Ok(HostValue::Undefined)
                })
                .expect_err("a native module cannot replace source");
            assert_eq!(reverse_collision.kind, ErrorKind::InvalidInput);
            assert_eq!(reverse_collision.phase, ErrorPhase::RegisterModule);
        }

        #[test]
        fn date_timezone_and_random_intrinsics_are_operational() {
            let mut realm = single_realm();
            let result = realm
                .evaluate(
                    EvalSource::new(
                        "(() => { \
                         const random = Math.random(); \
                         return Number.isFinite(Date.now()) && \
                           Number.isFinite(new Date(0).getTimezoneOffset()) && \
                           random >= 0 && random < 1; \
                         })()",
                    ),
                    EvalOptions::default(),
                )
                .expect("time and random intrinsics should execute");

            assert_eq!(result.as_boolean(), Some(true));
        }

        #[test]
        fn rust_stdlib_math_hooks_are_operational() {
            let mut realm = single_realm();
            let atanh = realm
                .evaluate(EvalSource::new("Math.atanh(0.5)"), EvalOptions::default())
                .expect("Math.atanh should execute through its Rust hook")
                .as_number()
                .expect("Math.atanh should return a number");
            let clamped = realm
                .evaluate(
                    EvalSource::new(
                        "(() => { \
                         const values = new Uint8ClampedArray([0.5, 1.5, 2.5, 3.5]); \
                         return values[0] * 1000 + values[1] * 100 + values[2] * 10 + values[3]; \
                         })()",
                    ),
                    EvalOptions::default(),
                )
                .expect("Uint8ClampedArray conversion should execute through its Rust hook");

            assert_eq!(atanh.to_bits(), 0.5_f64.atanh().to_bits());
            assert_eq!(clamped.as_number(), Some(224.0));
        }

        #[test]
        fn times_out_infinite_evaluation_and_reuses_realm() {
            let (kind, phase, reused) = run_with_watchdog(|| {
                let (_runtime, mut realm) = timed_realm();
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
                let (_runtime, mut realm) = timed_realm();
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
                let (_runtime, mut realm) = timed_realm();
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
                let (mut runtime, mut realm) = timed_realm();
                realm
                    .evaluate(
                        EvalSource::new(
                            "Promise.resolve().then(() => { while (true) {} }); undefined",
                        ),
                        EvalOptions::default(),
                    )
                    .expect("job should be scheduled");
                let error = runtime
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
                let (mut runtime, mut realm) = timed_realm();
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
                let error = runtime
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
            let (kind, phase, reused) =
                run_with_external_interrupt(|runtime, realm, handle_sender| {
                    handle_sender
                        .send(runtime.interrupt_handle())
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
            let (kind, phase, reused) =
                run_with_external_interrupt(|runtime, realm, handle_sender| {
                    let callable = realm
                        .evaluate(
                            EvalSource::new("() => { while (true) {} }"),
                            EvalOptions::default(),
                        )
                        .expect("callable should evaluate");
                    handle_sender
                        .send(runtime.interrupt_handle())
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
            let (kind, phase, reused) =
                run_with_external_interrupt(|runtime, realm, handle_sender| {
                    realm
                        .evaluate(
                            EvalSource::new(
                                "Promise.resolve().then(() => { while (true) {} }); undefined",
                            ),
                            EvalOptions::default(),
                        )
                        .expect("job should be scheduled");
                    handle_sender
                        .send(runtime.interrupt_handle())
                        .expect("test should receive interrupt handle");
                    runtime
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
            let (runtime, mut realm) = runtime_and_realm();
            let handle = runtime.interrupt_handle();
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
        fn disabled_timeout_does_not_read_the_monotonic_clock() {
            let deadline = deadline_from_timeout(None, || {
                panic!("the clock must remain unused when execution timeout is disabled")
            });

            assert!(deadline.is_none());
        }

        #[test]
        fn rejects_unrepresentable_execution_timeout() {
            let error = Runtime::with_options(RuntimeOptions {
                execution_timeout: Some(Duration::MAX),
                ..RuntimeOptions::default()
            })
            .expect_err("an unrepresentable deadline must fail runtime creation");

            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert_eq!(error.phase, ErrorPhase::CreateRuntime);
        }

        #[test]
        fn round_trips_exact_utf16() {
            let realm = single_realm();
            let units = [0x0000, 0x0061, 0xd800, 0xdc00, 0xdfff, 0x20ac];
            let value = realm.string_utf16(&units).unwrap();

            assert_eq!(value.kind(), ValueKind::String);
            assert_eq!(value.to_utf16().unwrap(), units);
        }

        #[test]
        fn retains_runtime_after_realm_is_dropped() {
            let value = {
                let realm = single_realm();
                realm.string("still rooted").unwrap()
            };

            assert_eq!(
                value.to_utf16().unwrap(),
                "still rooted".encode_utf16().collect::<Vec<_>>()
            );
        }

        #[test]
        fn rejects_cross_realm_calls() {
            let (runtime, mut first) = runtime_and_realm();
            let second = runtime.create_context().unwrap();
            let function = first
                .evaluate(EvalSource::new("value => value"), EvalOptions::default())
                .unwrap();
            let sibling = second.number(1.0).unwrap();
            let stranger = single_realm().number(1.0).unwrap();

            // Sharing a runtime is not sharing a realm: a `JSValue` belongs to
            // the context that made it either way.
            assert_eq!(
                first.call(&function, None, &[sibling]).unwrap_err().kind,
                ErrorKind::WrongRealm
            );
            assert_eq!(
                first.call(&function, None, &[stranger]).unwrap_err().kind,
                ErrorKind::WrongRealm
            );
        }

        #[test]
        fn sibling_realms_have_separate_globals() {
            let (runtime, mut first) = runtime_and_realm();
            let mut second = runtime.create_context().unwrap();
            first
                .evaluate(
                    EvalSource::new("globalThis.answer = 42"),
                    EvalOptions::default(),
                )
                .unwrap();
            second
                .define_global_function("onlyHere", 0, |_| Ok(HostValue::Undefined))
                .unwrap();

            assert_eq!(
                text_of(&mut second, "typeof globalThis.answer"),
                "undefined"
            );
            assert_eq!(text_of(&mut first, "typeof onlyHere"), "undefined");
            assert_eq!(number(&mut first, "answer"), Some(42.0));
        }

        #[test]
        fn one_module_source_instantiates_once_per_realm() {
            let (mut runtime, mut first) = runtime_and_realm();
            let mut second = runtime.create_context().unwrap();
            runtime
                .register_module_source(
                    "app:///counter.js",
                    "let count = 0;\n\
                     export function bump() { return ++count; }",
                )
                .unwrap();
            runtime
                .register_module_source(
                    "app:///entry.js",
                    "import { bump } from 'app:///counter.js';\n\
                     export const first = bump();\n\
                     export const second = bump();",
                )
                .unwrap();

            for realm in [&mut first, &mut second] {
                let evaluation = realm
                    .evaluate(
                        EvalSource {
                            text: "await import('app:///entry.js');",
                            name: Some("bobcat:boot"),
                            line_offset: 0,
                        },
                        EvalOptions {
                            source_type: SourceType::Module,
                            ..EvalOptions::default()
                        },
                    )
                    .expect("boot module should start");
                runtime
                    .drain_pending_jobs()
                    .expect("module jobs should settle");
                assert!(
                    realm
                        .settled_promise_result(&evaluation)
                        .expect("module evaluation should fulfill")
                        .is_some()
                );
                // One registered source, but each realm counts from zero: the
                // text is shared, the module instance is not.
                let namespace = realm.module_namespace("app:///entry.js").unwrap();
                assert_eq!(
                    realm.property(&namespace, "first").unwrap().as_number(),
                    Some(1.0)
                );
                assert_eq!(
                    realm.property(&namespace, "second").unwrap().as_number(),
                    Some(2.0)
                );
            }
        }

        #[test]
        fn interned_member_names_serve_every_realm_on_a_runtime() {
            let (runtime, mut first) = runtime_and_realm();
            let mut second = runtime.create_context().unwrap();
            let answer = first.member("answer").unwrap();
            let here = first
                .evaluate(
                    EvalSource::new("({ answer: () => 20 })"),
                    EvalOptions::default(),
                )
                .unwrap();
            let there = second
                .evaluate(
                    EvalSource::new("({ answer: () => 22 })"),
                    EvalOptions::default(),
                )
                .unwrap();

            // Atoms live on the runtime, so one interning serves both realms.
            assert_eq!(
                called_number(first.call_member(&here, &answer, &[])),
                Some(20.0)
            );
            assert_eq!(
                called_number(second.call_member(&there, &answer, &[])),
                Some(22.0)
            );

            let stranger = single_realm().member("answer").unwrap();
            assert_eq!(
                first.call_member(&here, &stranger, &[]).unwrap_err().kind,
                ErrorKind::WrongRealm
            );
        }

        #[test]
        fn native_modules_are_per_realm_under_one_specifier_namespace() {
            let (mut runtime, mut first) = runtime_and_realm();
            let mut second = runtime.create_context().unwrap();
            for (realm, answer) in [(&mut first, 20.0), (&mut second, 22.0)] {
                realm
                    .register_host_module_function("bobcat-internal:host", "answer", 0, move |_| {
                        Ok(HostValue::Number(answer))
                    })
                    .unwrap();
            }
            runtime
                .register_module_source(
                    "app:///entry.js",
                    "import { answer } from 'bobcat-internal:host';\n\
                     export const value = answer();",
                )
                .unwrap();

            for (realm, answer) in [(&mut first, 20.0), (&mut second, 22.0)] {
                let evaluation = realm
                    .evaluate(
                        EvalSource {
                            text: "await import('app:///entry.js');",
                            name: Some("bobcat:boot"),
                            line_offset: 0,
                        },
                        EvalOptions {
                            source_type: SourceType::Module,
                            ..EvalOptions::default()
                        },
                    )
                    .expect("boot module should start");
                runtime
                    .drain_pending_jobs()
                    .expect("module jobs should settle");
                assert!(
                    realm
                        .settled_promise_result(&evaluation)
                        .expect("module evaluation should fulfill")
                        .is_some()
                );
                let namespace = realm.module_namespace("app:///entry.js").unwrap();
                assert_eq!(
                    realm.property(&namespace, "value").unwrap().as_number(),
                    Some(answer)
                );
            }

            // Both realms claim the specifier, so no source module may take it.
            let collision = runtime
                .register_module_source("bobcat-internal:host", "export {};")
                .expect_err("a native specifier is reserved runtime-wide");
            assert_eq!(collision.kind, ErrorKind::InvalidInput);

            drop(first);
            drop(second);
            runtime
                .register_module_source("bobcat-internal:host", "export {};")
                .expect("the specifier is free once every realm holding it is gone");
        }

        #[test]
        fn pending_jobs_from_every_realm_drain_through_the_runtime() {
            let (mut runtime, mut first) = runtime_and_realm();
            let mut second = runtime.create_context().unwrap();
            for (realm, answer) in [(&mut first, 20), (&mut second, 22)] {
                realm
                    .evaluate(
                        EvalSource::new(&format!(
                            "globalThis.answer = 0; \
                             Promise.resolve().then(() => {{ answer = {answer}; }})"
                        )),
                        EvalOptions::default(),
                    )
                    .unwrap();
            }

            assert_eq!(runtime.drain_pending_jobs().unwrap(), 2);
            assert_eq!(number(&mut first, "answer"), Some(20.0));
            assert_eq!(number(&mut second, "answer"), Some(22.0));
        }

        #[test]
        fn a_host_function_may_call_into_a_sibling_realm() {
            let (runtime, mut first) = runtime_and_realm();
            let second = Rc::new(RefCell::new(runtime.create_context().unwrap()));
            let sibling = Rc::clone(&second);
            first
                .define_global_function("askSibling", 0, move |_| {
                    sibling
                        .borrow_mut()
                        .evaluate(EvalSource::new("21 * 2"), EvalOptions::default())
                        .map_err(|error| HostFunctionError::new(error.message))?
                        .as_number()
                        .map(HostValue::Number)
                        .ok_or_else(|| HostFunctionError::new("the sibling returned no number"))
                })
                .unwrap();

            // The sibling call runs inside this one, so its execution guard
            // nests inside the outer realm's.
            let result = first
                .evaluate(EvalSource::new("askSibling()"), EvalOptions::default())
                .expect("a host function may reach a sibling realm");

            assert_eq!(result.as_number(), Some(42.0));
        }

        #[test]
        fn execution_limits_cover_realms_created_later() {
            let (kind, phase) = run_with_watchdog(|| {
                let (runtime, _first) = timed_realm();
                let mut second = runtime.create_context().expect("realm should initialize");
                let error = second
                    .evaluate(EvalSource::new("for (;;) {}"), EvalOptions::default())
                    .expect_err("a later realm shares the runtime's deadline");
                (error.kind, error.phase)
            });

            assert_eq!(kind, ErrorKind::ExecutionTimeout);
            assert_eq!(phase, ErrorPhase::Evaluate);
        }

        fn called_number(outcome: Result<CallOutcome, Error>) -> Option<f64> {
            match outcome.expect("the member should be callable") {
                CallOutcome::Called(value) => value.as_number(),
                CallOutcome::MemberAbsent => None,
            }
        }

        #[test]
        fn drains_pending_jobs() {
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .evaluate(
                    EvalSource::new(
                        "globalThis.answer = 0; Promise.resolve().then(() => answer = 42)",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            assert_eq!(runtime.drain_pending_jobs().unwrap(), 1);
            let result = realm
                .evaluate(EvalSource::new("answer"), EvalOptions::default())
                .unwrap();
            assert_eq!(result.as_number(), Some(42.0));
        }

        #[test]
        fn reports_sanitized_source_location_with_offset() {
            let mut realm = single_realm();
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
            let mut realm = single_realm();
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
                let mut realm = single_realm();
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
            let mut realm = single_realm();
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
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .evaluate(
                    EvalSource::new("void Promise.reject(new Error('unhandled'))"),
                    EvalOptions::default(),
                )
                .unwrap();

            let error = runtime.drain_pending_jobs_up_to(8).unwrap_err();
            assert_eq!(error.phase, ErrorPhase::PendingJob);
            assert_eq!(error.name.as_deref(), Some("Error"));
            assert_eq!(error.message, "unhandled");
        }

        #[test]
        fn clears_rejections_handled_before_checkpoint_finishes() {
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .evaluate(
                    EvalSource::new(
                        "const rejected = Promise.reject(new Error('handled')); \
                     Promise.resolve().then(() => rejected.catch(() => {}))",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            assert!(runtime.drain_pending_jobs().unwrap() > 0);
        }

        #[test]
        fn preserves_multiple_unhandled_rejections() {
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .evaluate(
                    EvalSource::new(
                        "void Promise.reject(new Error('first')); \
                     void Promise.reject(new Error('second'))",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            let first = runtime.drain_pending_jobs_up_to(0).unwrap_err();
            let second = runtime.drain_pending_jobs_up_to(0).unwrap_err();
            assert_eq!(first.message, "first");
            assert_eq!(second.message, "second");
        }

        #[test]
        fn bounded_drain_reports_remaining_jobs_precisely() {
            let (mut runtime, mut realm) = runtime_and_realm();
            realm
                .evaluate(
                    EvalSource::new(
                        "Promise.resolve().then(() => {}).then(() => {}).then(() => {})",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            let first = runtime.drain_pending_jobs_up_to(1).unwrap();
            assert_eq!(first.executed, 1);
            assert!(first.jobs_remaining);
            assert!(runtime.drain_pending_jobs().unwrap() > 0);
            assert!(!runtime.has_pending_jobs());
        }

        fn number(realm: &mut Context, source: &str) -> Option<f64> {
            realm
                .evaluate(EvalSource::new(source), EvalOptions::default())
                .expect("evaluation should succeed")
                .as_number()
        }

        #[test]
        fn a_host_function_is_callable_from_javascript() {
            let mut realm = single_realm();
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
            let mut realm = single_realm();
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
            let mut realm = single_realm();
            realm
                .define_global_function("describe", 1, |arguments| {
                    Ok(HostValue::String(match &arguments[0] {
                        HostValue::Undefined => "undefined".to_owned(),
                        HostValue::Null => "null".to_owned(),
                        HostValue::Boolean(value) => format!("boolean:{value}"),
                        HostValue::Number(value) => format!("number:{value}"),
                        HostValue::String(value) => format!("string:{value}"),
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
            let mut realm = single_realm();
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
            let mut realm = single_realm();
            realm
                .define_global_function("take", 1, |_| Ok(HostValue::Undefined))
                .unwrap();
            let error = realm
                .evaluate(EvalSource::new("take({})"), EvalOptions::default())
                .expect_err("an object argument");
            assert!(
                error.message.contains("undefined, null, Boolean"),
                "{error:?}"
            );
        }

        #[test]
        fn a_host_error_becomes_a_catchable_javascript_exception() {
            let mut realm = single_realm();
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
            let mut realm = single_realm();
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
            let mut realm = single_realm();
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
            let mut realm = single_realm();
            let global = realm.global_object().unwrap();
            assert_eq!(global.kind(), ValueKind::Object);
            let value = realm.number(11.0).unwrap();
            realm.set_property(&global, "answer", &value).unwrap();
            assert_eq!(number(&mut realm, "answer"), Some(11.0));
        }

        #[test]
        fn set_property_rejects_a_value_from_another_realm() {
            let mut realm = single_realm();
            let other = single_realm();
            let global = realm.global_object().unwrap();
            let foreign = other.number(1.0).unwrap();
            let error = realm
                .set_property(&global, "x", &foreign)
                .expect_err("cross-realm value");
            assert_eq!(error.kind, ErrorKind::WrongRealm);
        }

        #[test]
        fn a_host_function_can_be_installed_on_an_ordinary_object() {
            let mut realm = single_realm();
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
            let mut realm = single_realm();
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

        fn text_of(realm: &mut Context, expression: &str) -> String {
            let value = realm
                .evaluate(EvalSource::new(expression), EvalOptions::default())
                .expect("evaluate");
            String::from_utf16(&value.to_utf16().expect("text")).expect("well-formed")
        }

        #[test]
        fn a_member_call_passes_every_primitive_through() {
            let mut realm = single_realm();
            realm
                .evaluate(
                    EvalSource::new(
                        "globalThis.host = { seen: null }; \
                         host.take = function take(...args) { host.seen = args; }; \
                         globalThis.host",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();
            let host = realm
                .evaluate(EvalSource::new("globalThis.host"), EvalOptions::default())
                .unwrap();
            let take = realm.member("take").unwrap();
            let outcome = realm
                .call_member(
                    &host,
                    &take,
                    &[
                        HostArgument::Undefined,
                        HostArgument::Null,
                        HostArgument::Boolean(true),
                        HostArgument::Number(-2.5),
                        HostArgument::String("a\u{1f980}b"),
                    ],
                )
                .unwrap();
            assert!(matches!(outcome, CallOutcome::Called(_)));
            assert_eq!(
                text_of(&mut realm, "host.seen.map(v => typeof v).join(',')"),
                "undefined,object,boolean,number,string"
            );
            assert_eq!(number(&mut realm, "host.seen[3]"), Some(-2.5));
            assert_eq!(text_of(&mut realm, "host.seen[4]"), "a\u{1f980}b");
        }

        #[test]
        fn a_member_call_survives_more_arguments_than_the_inline_capacity() {
            let mut realm = single_realm();
            let host = realm
                .evaluate(
                    EvalSource::new(
                        "globalThis.host = { sum(...args) { \
                           return args.reduce((a, b) => a + b, 0); } }; globalThis.host",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();
            let sum = realm.member("sum").unwrap();
            let arguments: Vec<HostArgument<'_>> = (0..32)
                .map(|i| HostArgument::Number(f64::from(i)))
                .collect();
            let CallOutcome::Called(total) = realm.call_member(&host, &sum, &arguments).unwrap()
            else {
                panic!("the member ran");
            };
            assert_eq!(total.as_number(), Some(496.0));
        }

        #[test]
        fn an_unpublished_member_is_absent_rather_than_an_error() {
            let mut realm = single_realm();
            let host = realm
                .evaluate(
                    EvalSource::new("globalThis.host = { notAFunction: 7 }; globalThis.host"),
                    EvalOptions::default(),
                )
                .unwrap();
            for name in ["missing", "notAFunction"] {
                let member = realm.member(name).unwrap();
                let outcome = realm.call_member(&host, &member, &[]).unwrap();
                assert!(matches!(outcome, CallOutcome::MemberAbsent), "{name}");
            }
        }

        #[test]
        fn a_throwing_member_reports_its_exception() {
            let mut realm = single_realm();
            let host = realm
                .evaluate(
                    EvalSource::new(
                        "globalThis.host = { boom() { throw new Error('listener'); } }; \
                         globalThis.host",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();
            let boom = realm.member("boom").unwrap();
            let error = realm
                .call_member(&host, &boom, &[])
                .expect_err("the member threw");
            assert_eq!(error.kind, ErrorKind::Exception);
            assert!(error.message.contains("listener"), "{error:?}");
        }

        #[test]
        fn a_member_name_from_another_realm_is_refused() {
            let mut realm = single_realm();
            let mut other = single_realm();
            let host = realm.global_object().unwrap();
            let foreign = other.member("anything").unwrap();
            let error = realm
                .call_member(&host, &foreign, &[])
                .expect_err("cross-realm member");
            assert_eq!(error.kind, ErrorKind::WrongRealm);
        }

        #[test]
        fn an_interned_member_outlives_its_realm_handle() {
            let member = {
                let mut realm = single_realm();
                realm.member("kept").unwrap()
            };
            drop(member);
        }

        #[test]
        fn an_ill_formed_utf16_argument_is_rejected_at_the_boundary() {
            let mut realm = single_realm();
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
            let mut realm = single_realm();
            let error = realm
                .function("bad\0name", 0, |_| Ok(HostValue::Undefined))
                .expect_err("a NUL in the name");
            assert_eq!(error.kind, ErrorKind::InvalidInput);
        }
    }
}

pub use implementation::{
    CallOutcome, Context, Error, ErrorKind, ErrorPhase, EvalOptions, EvalSource, HostArgument,
    HostFunctionError, HostValue, InterruptHandle, JobDrain, Member, Runtime, RuntimeOptions,
    SourceLocation, SourceType, Value, ValueKind,
};
