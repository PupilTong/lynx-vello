//! A small, safe Rust boundary around the repository's pinned `QuickJS` source.
//!
//! A [`Realm`] owns one `QuickJS` runtime and context. Realms and their
//! [`Value`] handles are deliberately owner-thread-bound (`!Send` and
//! `!Sync`). Every value keeps its runtime alive, so dropping a realm before
//! its handles is safe. No raw `QuickJS` pointer is part of the public API.
//!
//! The vendored engine builds on Unix targets and Windows GNU/MinGW. Windows
//! MSVC is rejected explicitly because the upstream C sources do not support
//! that ABI/toolchain.
//!
//! The crate also provides [`QuickJsScriptEngine`], a direct implementation of
//! [`bobcat_engine::script::ScriptEngine`], plus convenience constructors for
//! QuickJS-backed [`bobcat_engine::view::LynxView`] instances.

mod bobcat;
mod ffi;

#[allow(
    unsafe_code,
    reason = "this private implementation module contains the audited QuickJS FFI call sites"
)]
mod implementation {
    use std::ffi::CString;
    use std::fmt;
    use std::num::TryFromIntError;
    use std::ptr::{self, NonNull};
    use std::rc::Rc;

    use super::ffi;

    const JS_EVAL_TYPE_GLOBAL: i32 = 0;
    const JS_EVAL_TYPE_MODULE: i32 = 1;
    const JS_EVAL_FLAG_STRICT: i32 = 1 << 3;
    const JS_EVAL_FLAG_BACKTRACE_BARRIER: i32 = 1 << 6;
    const JS_EVAL_FLAG_ASYNC: i32 = 1 << 7;
    const QJS_EVAL_FAILURE_COMPILE: i32 = 1;

    /// Configuration applied before `QuickJS` creates a realm context.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct RealmOptions {
        /// Heap limit in bytes. `None` uses `QuickJS`'s default (unlimited).
        pub memory_limit: Option<usize>,
        /// Native stack limit in bytes. `None` uses `QuickJS`'s default.
        pub max_stack_size: Option<usize>,
    }

    /// Borrowed JavaScript source plus diagnostic metadata.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct EvalSource<'a> {
        /// UTF-8 JavaScript source text.
        pub text: &'a str,
        /// Diagnostic source name. `QuickJS` uses `"<eval>"` when absent.
        pub name: Option<&'a str>,
        /// Number of lines to add before the first source line in diagnostics.
        pub line_offset: u32,
    }

    impl<'a> EvalSource<'a> {
        /// Creates source with no explicit name or line offset.
        #[must_use]
        pub const fn new(text: &'a str) -> Self {
            Self {
                text,
                name: None,
                line_offset: 0,
            }
        }
    }

    /// How `QuickJS` should parse and execute an [`EvalSource`].
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    #[allow(
        clippy::struct_excessive_bools,
        reason = "these independent flags map directly to QuickJS evaluation flags"
    )]
    pub struct EvalOptions {
        /// Parse as an ECMAScript module rather than a classic script.
        pub module: bool,
        /// Force strict mode for a classic script.
        pub strict: bool,
        /// Hide stack frames that precede this evaluation.
        pub backtrace_barrier: bool,
        /// Permit top-level await in a classic script.
        pub top_level_await: bool,
    }

    /// Coarse, stable classification of a JavaScript value.
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

    /// The bridge operation that failed.
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

    /// Stable failure category, independent of `QuickJS` object identity.
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
        Engine,
    }

    /// Sanitized source coordinates copied out of a `QuickJS` error.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SourceLocation {
        pub source: Option<String>,
        pub line: Option<u32>,
        pub column: Option<u32>,
    }

    /// A JavaScript or bridge failure with no engine-owned object attached.
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

    /// Result of running pending jobs with a finite budget.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct JobDrain {
        /// Jobs executed during this drain attempt.
        pub executed: usize,
        /// Whether at least one job remains queued.
        pub jobs_remaining: bool,
    }

    struct RealmInner {
        runtime: NonNull<ffi::QjsRuntime>,
        context: NonNull<ffi::JSContext>,
    }

    impl Drop for RealmInner {
        fn drop(&mut self) {
            // SAFETY: this is the last `Rc<RealmInner>`, hence all ValueInner
            // instances have already freed their values. The context belongs to
            // this runtime and both pointers came from their matching constructors.
            unsafe {
                ffi::qjs_context_free(self.context.as_ptr());
                ffi::qjs_runtime_free(self.runtime.as_ptr());
            }
        }
    }

    /// One owner-thread-bound `QuickJS` runtime and global realm.
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
        /// Creates a realm with `QuickJS` defaults and blocking operations disabled.
        pub fn new() -> Result<Self, Error> {
            Self::with_options(RealmOptions::default())
        }

        /// Creates a realm with optional heap and native-stack limits.
        pub fn with_options(options: RealmOptions) -> Result<Self, Error> {
            // SAFETY: the returned pointers are checked before use and immediately
            // placed under `RealmInner`'s matching destruction path.
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
                Ok(Self {
                    inner: Rc::new(RealmInner { runtime, context }),
                })
            }
        }

        /// Creates JavaScript `undefined`.
        pub fn undefined(&self) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: `context` is live for the duration of this call.
                unsafe { ffi::qjs_new_undefined(context) }
            })
        }

        /// Creates JavaScript `null`.
        pub fn null(&self) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: `context` is live for the duration of this call.
                unsafe { ffi::qjs_new_null(context) }
            })
        }

        /// Creates a JavaScript Boolean.
        pub fn boolean(&self, value: bool) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: `context` is live for the duration of this call.
                unsafe { ffi::qjs_new_boolean(context, i32::from(value)) }
            })
        }

        /// Creates a JavaScript Number, preserving negative zero and non-finite values.
        pub fn number(&self, value: f64) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: `context` is live for the duration of this call.
                unsafe { ffi::qjs_new_number(context, value) }
            })
        }

        /// Creates a JavaScript `BigInt` from a signed 64-bit integer.
        pub fn big_int64(&self, value: i64) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: `context` is live for the duration of this call.
                unsafe { ffi::qjs_new_big_int64(context, value) }
            })
        }

        /// Creates a JavaScript `BigInt` from an unsigned 64-bit integer.
        pub fn big_uint64(&self, value: u64) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: `context` is live for the duration of this call.
                unsafe { ffi::qjs_new_big_uint64(context, value) }
            })
        }

        /// Creates an arbitrary-size `BigInt` from canonical signed decimal text.
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
            self.eval(
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

        /// Creates a JavaScript String from Unicode scalar-value text.
        pub fn string(&self, value: &str) -> Result<Value, Error> {
            let utf16: Vec<u16> = value.encode_utf16().collect();
            self.string_utf16(&utf16)
        }

        /// Creates a JavaScript String from its exact UTF-16 code-unit sequence.
        /// Unpaired surrogates are preserved.
        pub fn string_utf16(&self, units: &[u16]) -> Result<Value, Error> {
            self.construct(ErrorPhase::ConstructValue, |context| {
                // SAFETY: the slice remains live during the C call; the shim copies
                // every code unit into a freshly allocated QuickJS string.
                unsafe { ffi::qjs_new_string_utf16(context, units.as_ptr(), units.len()) }
            })
        }

        /// Evaluates source in this realm.
        pub fn eval(
            &mut self,
            source: EvalSource<'_>,
            options: EvalOptions,
        ) -> Result<Value, Error> {
            if options.module && options.top_level_await {
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

            let mut flags = if options.module {
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
            // SAFETY: source is explicitly NUL-terminated, its length excludes the
            // terminator, source_name and failure_stage are live, and context is
            // live. The shim initializes failure_stage before returning.
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
            NonNull::new(raw)
                .map(|raw| Value::from_raw(Rc::clone(&self.inner), raw))
                .ok_or_else(|| {
                    self.capture_exception_with_syntax(
                        self.inner.context.as_ptr(),
                        ErrorPhase::Evaluate,
                        failure_stage == QJS_EVAL_FAILURE_COMPILE,
                    )
                })
        }

        /// Calls a function value after validating every handle's realm affinity.
        pub fn call(
            &mut self,
            callable: &Value,
            this_value: Option<&Value>,
            arguments: &[Value],
        ) -> Result<Value, Error> {
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
            // SAFETY: affinity validation proves all handles use this live context;
            // the pointer array remains live for the call and count fits `int`.
            let raw = unsafe {
                ffi::qjs_call(
                    self.inner.context.as_ptr(),
                    callable.inner.raw.as_ptr(),
                    this_value.map_or(ptr::null(), |value| value.inner.raw.as_ptr()),
                    arguments.len(),
                    raw_arguments.as_ptr(),
                )
            };
            self.value_or_exception(raw, self.inner.context.as_ptr(), ErrorPhase::Call)
        }

        /// Executes at most one pending Promise/microtask job.
        ///
        /// Returns `true` when a job ran and `false` when the queue was empty.
        pub fn execute_pending_job(&mut self) -> Result<bool, Error> {
            let mut job_context = ptr::null_mut();
            // SAFETY: runtime is live; QuickJS initializes job_context whenever it
            // reports a failing job.
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

        /// Reports whether at least one Promise/microtask job is queued.
        #[must_use]
        pub fn has_pending_job(&self) -> bool {
            // SAFETY: this realm owns the live runtime pointer.
            unsafe { ffi::qjs_has_pending_job(self.inner.runtime.as_ptr()) != 0 }
        }

        /// Runs pending jobs until `QuickJS` reports an empty queue.
        pub fn drain_pending_jobs(&mut self) -> Result<usize, Error> {
            let mut executed = 0usize;
            while self.execute_pending_job()? {
                executed = executed.saturating_add(1);
            }
            Ok(executed)
        }

        /// Runs pending jobs until the queue is empty or `budget` jobs ran.
        pub fn drain_pending_jobs_bounded(&mut self, budget: usize) -> Result<JobDrain, Error> {
            let mut executed = 0usize;
            while executed < budget && self.execute_pending_job()? {
                executed += 1;
            }
            let jobs_remaining = self.has_pending_job();
            if !jobs_remaining && let Some(error) = self.take_unhandled_rejection() {
                return Err(error);
            }
            Ok(JobDrain {
                executed,
                jobs_remaining,
            })
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
            // SAFETY: context is live and has just reported an exception. The shim
            // transfers the exception into an ordinary rooted value box.
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
            // SAFETY: this realm owns the live runtime pointer.
            if unsafe { ffi::qjs_has_unhandled_rejection(self.inner.runtime.as_ptr()) } == 0 {
                return None;
            }
            // SAFETY: the preceding query reported a tracked rejection. The shim
            // transfers one rooted reason into a value box.
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
            // SAFETY: `owner` keeps the matching context live, and this is the sole
            // `ValueInner` owning this C box.
            unsafe { ffi::qjs_value_free(self.owner.context.as_ptr(), self.raw.as_ptr()) }
        }
    }

    /// A rooted `QuickJS` value that keeps its owning realm alive.
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
        fn from_raw(owner: Rc<RealmInner>, raw: NonNull<ffi::QjsValue>) -> Self {
            Self {
                inner: Rc::new(ValueInner { owner, raw }),
            }
        }

        /// Returns the stable coarse type of this value.
        #[must_use]
        pub fn kind(&self) -> ValueKind {
            // SAFETY: the owner keeps both context and value live.
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

        /// Extracts a Boolean without coercion.
        #[must_use]
        pub fn as_boolean(&self) -> Option<bool> {
            if self.kind() != ValueKind::Boolean {
                return None;
            }
            let mut result = 0;
            // SAFETY: type was checked and both pointers remain live.
            let status = unsafe {
                ffi::qjs_value_get_boolean(
                    self.inner.owner.context.as_ptr(),
                    self.inner.raw.as_ptr(),
                    &raw mut result,
                )
            };
            (status == 0).then_some(result != 0)
        }

        /// Extracts a Number without coercion.
        #[must_use]
        pub fn as_number(&self) -> Option<f64> {
            if self.kind() != ValueKind::Number {
                return None;
            }
            let mut result = 0.0;
            // SAFETY: type was checked and both pointers remain live.
            let status = unsafe {
                ffi::qjs_value_get_number(
                    self.inner.owner.context.as_ptr(),
                    self.inner.raw.as_ptr(),
                    &raw mut result,
                )
            };
            (status == 0).then_some(result)
        }

        /// Extracts an exact UTF-16 code-unit sequence from a String.
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

        /// Extracts a `BigInt` as canonical signed decimal text.
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
            let context = self.inner.owner.context.as_ptr();
            let mut bytes = ptr::null();
            let mut length = 0usize;
            // SAFETY: the owner keeps both values live; on success the returned
            // allocation is owned by QuickJS until qjs_cesu8_free below.
            let status = unsafe {
                ffi::qjs_value_to_cesu8(
                    context,
                    self.inner.raw.as_ptr(),
                    &raw mut bytes,
                    &raw mut length,
                )
            };
            if status != 0 || bytes.is_null() {
                // SAFETY: conversion failure leaves a pending exception, which is
                // intentionally discarded because this API returns a stable error.
                unsafe { ffi::qjs_discard_exception(context) };
                return Err(Error::bridge(
                    ErrorKind::Engine,
                    ErrorPhase::ConvertValue,
                    "QuickJS could not convert the value to CESU-8",
                ));
            }
            // SAFETY: QuickJS returned `length` readable bytes.
            let encoded = unsafe { std::slice::from_raw_parts(bytes, length) };
            let decoded = decode_cesu8(encoded);
            // SAFETY: bytes came from qjs_value_to_cesu8 in this context.
            unsafe { ffi::qjs_cesu8_free(context, bytes) };
            decoded.map_err(|message| {
                Error::bridge(ErrorKind::Engine, ErrorPhase::ConvertValue, message)
            })
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
            // SAFETY: context/value/name are live. A null result means property
            // lookup threw; that secondary exception is discarded below.
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
                // SAFETY: a null qjs_get_property result means an exception marker.
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
            ErrorKind::OutOfMemory,
            ErrorPhase::Evaluate,
            "line offset does not fit this platform",
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn evaluates_and_calls_functions() {
            let mut realm = Realm::new().unwrap();
            let function = realm
                .eval(
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
                .eval(EvalSource::new("value => value"), EvalOptions::default())
                .unwrap();
            let foreign = second.number(1.0).unwrap();

            let error = first.call(&function, None, &[foreign]).unwrap_err();
            assert_eq!(error.kind, ErrorKind::WrongRealm);
        }

        #[test]
        fn drains_pending_jobs() {
            let mut realm = Realm::new().unwrap();
            realm
                .eval(
                    EvalSource::new(
                        "globalThis.answer = 0; Promise.resolve().then(() => answer = 42)",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            assert_eq!(realm.drain_pending_jobs().unwrap(), 1);
            let result = realm
                .eval(EvalSource::new("answer"), EvalOptions::default())
                .unwrap();
            assert_eq!(result.as_number(), Some(42.0));
        }

        #[test]
        fn reports_sanitized_source_location_with_offset() {
            let mut realm = Realm::new().unwrap();
            let error = realm
                .eval(
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
                .eval(EvalSource::new("const = 1"), EvalOptions::default())
                .unwrap_err();
            let thrown_error = realm
                .eval(
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
                    .eval(
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
                .eval(
                    EvalSource::new("void Promise.reject(new Error('unhandled'))"),
                    EvalOptions::default(),
                )
                .unwrap();

            let error = realm.drain_pending_jobs_bounded(8).unwrap_err();
            assert_eq!(error.phase, ErrorPhase::PendingJob);
            assert_eq!(error.name.as_deref(), Some("Error"));
            assert_eq!(error.message, "unhandled");
        }

        #[test]
        fn clears_rejections_handled_before_checkpoint_finishes() {
            let mut realm = Realm::new().unwrap();
            realm
                .eval(
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
                .eval(
                    EvalSource::new(
                        "void Promise.reject(new Error('first')); \
                     void Promise.reject(new Error('second'))",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            let first = realm.drain_pending_jobs_bounded(0).unwrap_err();
            let second = realm.drain_pending_jobs_bounded(0).unwrap_err();
            assert_eq!(first.message, "first");
            assert_eq!(second.message, "second");
        }

        #[test]
        fn bounded_drain_reports_remaining_jobs_precisely() {
            let mut realm = Realm::new().unwrap();
            realm
                .eval(
                    EvalSource::new(
                        "Promise.resolve().then(() => {}).then(() => {}).then(() => {})",
                    ),
                    EvalOptions::default(),
                )
                .unwrap();

            let first = realm.drain_pending_jobs_bounded(1).unwrap();
            assert_eq!(first.executed, 1);
            assert!(first.jobs_remaining);
            assert!(realm.drain_pending_jobs().unwrap() > 0);
            assert!(!realm.has_pending_job());
        }
    }
}

pub use bobcat::{
    DEFAULT_MAX_JOBS_PER_CHECKPOINT, QuickJsCallable, QuickJsConfig, QuickJsInitializationError,
    QuickJsLynxView, QuickJsScriptEngine, QuickJsScriptEngineConfig, QuickJsSymbol,
    new_quickjs_view, new_quickjs_view_with_config,
};
pub use implementation::{
    Error, ErrorKind, ErrorPhase, EvalOptions, EvalSource, JobDrain, Realm, RealmOptions,
    SourceLocation, Value, ValueKind,
};
