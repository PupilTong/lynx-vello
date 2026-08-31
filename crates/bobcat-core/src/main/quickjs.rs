//! The main thread's script engine: one `QuickJS` runtime and one realm,
//! the policy around them.
//!
//! Runtime and realm live in `quickjs-rust-bridge`. What this module adds is
//! everything the runtime needs and the bridge deliberately leaves open: when
//! the promise-job queue is drained, how many jobs one checkpoint may run,
//! how a bridge failure becomes a sanitized [`ScriptError`], and how a module
//! namespace caches the atoms an export is looked up by.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use quickjs_rust_bridge as quickjs;

use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase, ScriptSourceLocation};

/// A leaf host callback Bobcat installs in the realm.
///
/// The realm turns a returned message into a JavaScript exception. Values
/// crossing either way are [`quickjs::HostValue`] primitives, so no realm
/// handle and no DOM identity ever crosses as itself.
pub(crate) type HostCallback =
    Box<dyn FnMut(&[quickjs::HostValue]) -> Result<quickjs::HostValue, String> + 'static>;

const DEFAULT_MAX_JOBS_PER_CHECKPOINT: NonZeroUsize =
    NonZeroUsize::new(1_024).expect("the default job limit is non-zero");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuickJsConfig {
    runtime_options: quickjs::RuntimeOptions,
    max_jobs_per_checkpoint: NonZeroUsize,
}

#[cfg(test)]
impl QuickJsConfig {
    #[must_use]
    const fn with_execution_timeout(mut self, execution_timeout: Option<Duration>) -> Self {
        self.runtime_options.execution_timeout = execution_timeout;
        self
    }
}

impl Default for QuickJsConfig {
    fn default() -> Self {
        Self {
            runtime_options: quickjs::RuntimeOptions {
                memory_limit: None,
                max_stack_size: None,
                execution_timeout: None,
            },
            max_jobs_per_checkpoint: DEFAULT_MAX_JOBS_PER_CHECKPOINT,
        }
    }
}

/// One loaded module's namespace object, plus the export names already
/// interned in it.
///
/// `QuickJS` resolves a property by atom. Interning the name on every call
/// would hash and allocate once per crossing, so a namespace remembers the
/// atom for each export the host has ever called through it — the event path
/// calls exactly one, on every step of every walk.
struct ModuleNamespace {
    object: quickjs::Value,
    exports: HashMap<Box<str>, Rc<quickjs::Member>>,
}

/// One owner-thread-bound `QuickJS` runtime and its single realm, with
/// Bobcat's job policy on them.
///
/// The runtime could carry more realms — a bundle's main-thread and
/// background scripts are the obvious pair — but Bobcat runs one, and the
/// job queue and execution limits it owns are shared by whatever it carries.
///
/// Created on the thread that will own it — the engine-owned Lynx main
/// thread — and never moved off it, which is why nothing here is `Send`.
pub(crate) struct ScriptEngine {
    runtime: quickjs::Runtime,
    realm: quickjs::Context,
    module_namespaces: HashMap<String, ModuleNamespace>,
    config: QuickJsConfig,
    checkpoint_incomplete: bool,
    deferred_checkpoint_error: Option<ScriptError>,
}

impl ScriptEngine {
    pub(crate) fn new() -> Result<Self, ScriptError> {
        Self::with_config(QuickJsConfig::default())
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Initialize))
    }

    /// A thread-safe handle used only to cancel startup while author code is
    /// synchronously executing. Realm values and the runtime itself remain on
    /// the Lynx main thread.
    pub(crate) fn interrupt_handle(&self) -> quickjs::InterruptHandle {
        self.runtime.interrupt_handle()
    }

    fn with_config(config: QuickJsConfig) -> Result<Self, quickjs::Error> {
        let runtime = quickjs::Runtime::with_options(config.runtime_options)?;
        let realm = runtime.create_context()?;
        Ok(Self {
            runtime,
            realm,
            module_namespaces: HashMap::new(),
            config,
            checkpoint_incomplete: false,
            deferred_checkpoint_error: None,
        })
    }

    fn module_namespace(&mut self, specifier: &str) -> Result<quickjs::Value, ScriptError> {
        if let Some(entry) = self.module_namespaces.get(specifier) {
            return Ok(entry.object.clone());
        }
        let object = self
            .realm
            .module_namespace(specifier)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::CallModuleExport))?;
        self.module_namespaces.insert(
            specifier.to_owned(),
            ModuleNamespace {
                object: object.clone(),
                exports: HashMap::new(),
            },
        );
        Ok(object)
    }

    /// Resolves a module export to the namespace object it lives on and the
    /// interned name to look it up by, interning that name at most once per
    /// export.
    ///
    /// A module namespace is an ordinary object to `QuickJS`, so the atom a
    /// property lookup needs is the same one whatever the object is — which
    /// is why the cache survived the move off `globalThis`.
    fn module_export(
        &mut self,
        specifier: &str,
        export_name: &str,
    ) -> Result<(quickjs::Value, Rc<quickjs::Member>), ScriptError> {
        let object = self.module_namespace(specifier)?;
        if let Some(member) = self
            .module_namespaces
            .get(specifier)
            .and_then(|entry| entry.exports.get(export_name))
        {
            return Ok((object, Rc::clone(member)));
        }
        let member = Rc::new(
            self.realm
                .member(export_name)
                .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::CallModuleExport))?,
        );
        self.module_namespaces
            .get_mut(specifier)
            .expect("the module namespace was just ensured")
            .exports
            .insert(Box::from(export_name), Rc::clone(&member));
        Ok((object, member))
    }

    fn finish_operation<T>(
        &mut self,
        result: Result<T, ScriptError>,
        phase: ScriptErrorPhase,
    ) -> Result<T, ScriptError> {
        match result {
            Ok(value) => {
                self.checkpoint(phase)?;
                Ok(value)
            }
            Err(primary_error) => {
                if let Err(checkpoint_error) = self.checkpoint(phase) {
                    self.deferred_checkpoint_error
                        .get_or_insert(checkpoint_error);
                }
                Err(primary_error)
            }
        }
    }

    fn checkpoint(&mut self, phase: ScriptErrorPhase) -> Result<usize, ScriptError> {
        let drain = match self
            .runtime
            .drain_pending_jobs_up_to(self.config.max_jobs_per_checkpoint.get())
        {
            Ok(drain) => drain,
            Err(error) => {
                self.checkpoint_incomplete = true;
                return Err(map_quickjs_error(error, phase));
            }
        };
        self.checkpoint_incomplete = drain.jobs_remaining;
        if drain.jobs_remaining {
            return Err(script_error(
                ScriptErrorKind::Other,
                phase,
                "QuickJS promise jobs exceeded the per-checkpoint limit",
            ));
        }
        Ok(drain.executed)
    }

    fn resume_incomplete_checkpoint(&mut self, phase: ScriptErrorPhase) -> Result<(), ScriptError> {
        if let Some(error) = self.deferred_checkpoint_error.take() {
            return Err(error);
        }
        if self.checkpoint_incomplete {
            self.checkpoint(phase)?;
        }
        Ok(())
    }

    /// Installs one Rust-backed function as a named export of a native ESM
    /// module.
    ///
    /// Every export must be registered before that module first loads, and a
    /// name cannot be replaced. Registration retains the callback without
    /// invoking it.
    pub(crate) fn register_host_module_function(
        &mut self,
        module_specifier: &str,
        export_name: &str,
        arity: u8,
        mut callback: HostCallback,
    ) -> Result<(), ScriptError> {
        self.resume_incomplete_checkpoint(ScriptErrorPhase::RegisterHostModuleFunction)?;
        self.realm
            .register_host_module_function(
                module_specifier,
                export_name,
                u32::from(arity),
                move |arguments| callback(arguments).map_err(quickjs::HostFunctionError::new),
            )
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostModuleFunction))
    }

    /// Evaluates classic source, which the runtime never does: every entry and
    /// every built-in is a module. It survives so this module's own tests can
    /// state a realm invariant in a snippet and let a throw fail the test.
    #[cfg(test)]
    fn execute_script(&mut self, source: &str, source_name: &str) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::Execute;
        self.resume_incomplete_checkpoint(PHASE)?;
        let result = self
            .realm
            .evaluate(
                quickjs::EvalSource {
                    name: Some(source_name),
                    ..quickjs::EvalSource::new(source)
                },
                quickjs::EvalOptions::default(),
            )
            .map(|_| ())
            .map_err(|error| map_quickjs_error(error, PHASE));
        self.finish_operation(result, PHASE)
    }

    /// Registers one exact, normalized module name and its UTF-8 source in
    /// the synchronous preloaded module graph.
    ///
    /// Source modules and native host modules share one specifier namespace.
    pub(crate) fn register_module_source(
        &mut self,
        specifier: &str,
        source: &str,
    ) -> Result<(), ScriptError> {
        self.resume_incomplete_checkpoint(ScriptErrorPhase::RegisterModule)?;
        self.runtime
            .register_module_source(specifier, source)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterModule))
    }

    /// Compiles, links, and evaluates one ESM entry, waiting for its
    /// evaluation promise — top-level await included — to settle.
    pub(crate) fn execute_module(
        &mut self,
        source: &str,
        source_name: &str,
    ) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::ExecuteModule;
        self.resume_incomplete_checkpoint(PHASE)?;
        let evaluation = self
            .realm
            .evaluate(
                quickjs::EvalSource {
                    name: Some(source_name),
                    ..quickjs::EvalSource::new(source)
                },
                quickjs::EvalOptions {
                    source_type: quickjs::SourceType::Module,
                    ..quickjs::EvalOptions::default()
                },
            )
            .map_err(|error| map_quickjs_error(error, PHASE));
        let evaluation = self.finish_operation(evaluation, PHASE)?;
        match self
            .realm
            .settled_promise_result(&evaluation)
            .map_err(|error| map_quickjs_error(error, PHASE))?
        {
            Some(_) => Ok(()),
            None => Err(script_error(
                ScriptErrorKind::ModuleEvaluate,
                PHASE,
                "QuickJS module evaluation remained pending after its job checkpoint",
            )),
        }
    }

    /// Calls a function a loaded source module exported, if it exported one
    /// under that name.
    ///
    /// `Ok(false)` means the module exported no such function — a bundle with
    /// no listener runtime, not a failure. `Ok(true)` means it ran and
    /// returned.
    ///
    /// There is no return value, because the one thing a callee needs to tell
    /// the host — that a walk should end — is a host call of its own, and
    /// making it a return value would mean the host could only hear it once
    /// the callee finished.
    pub(crate) fn call_module_export(
        &mut self,
        module_specifier: &str,
        export_name: &str,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<bool, ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::CallModuleExport;
        self.resume_incomplete_checkpoint(PHASE)?;
        let (object, member) = self.module_export(module_specifier, export_name)?;
        // One crossing carries the lookup and every argument, and nothing
        // here allocates: the primitives are described in place and a string
        // is lent, not copied — the caller holds it for the whole walk, and
        // the realm needs it only for the length of this call.
        //
        // The checkpoint runs through `finish_operation`, never inline:
        // draining the job queue while this call is still on the stack would
        // run promise jobs re-entrantly, inside the very walk they belong to.
        let result = self
            .realm
            .call_member(&object, &member, arguments)
            .map(|outcome| match outcome {
                // The realm published nothing here. Not an error: a bundle
                // with no listener runtime simply has no member to call.
                quickjs::CallOutcome::MemberAbsent => false,
                quickjs::CallOutcome::Called(_) => true,
            })
            .map_err(|error| map_quickjs_error(error, PHASE));
        self.finish_operation(result, PHASE)
    }

    /// Runs a collection now, so a dead handle's finalizer reaches the
    /// document before the batch it belongs to ends.
    pub(crate) fn collect_garbage(&mut self) -> Result<(), ScriptError> {
        self.resume_incomplete_checkpoint(ScriptErrorPhase::CollectGarbage)?;
        self.runtime.run_gc();
        self.checkpoint(ScriptErrorPhase::CollectGarbage)
            .map(|_| ())
    }
}

impl fmt::Debug for ScriptEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptEngine")
            .field("config", &self.config)
            .field("checkpoint_incomplete", &self.checkpoint_incomplete)
            .finish_non_exhaustive()
    }
}

fn map_quickjs_error(error: quickjs::Error, phase: ScriptErrorPhase) -> ScriptError {
    let kind = match error.kind {
        quickjs::ErrorKind::Syntax => ScriptErrorKind::Syntax,
        quickjs::ErrorKind::Exception => ScriptErrorKind::Exception,
        quickjs::ErrorKind::InvalidInput if error.phase == quickjs::ErrorPhase::ConstructValue => {
            ScriptErrorKind::InvalidBoundaryValue
        }
        quickjs::ErrorKind::InvalidInput if error.phase == quickjs::ErrorPhase::RegisterModule => {
            ScriptErrorKind::ModuleLoad
        }
        quickjs::ErrorKind::Interrupted | quickjs::ErrorKind::ExecutionTimeout => {
            ScriptErrorKind::Other
        }
        _ => ScriptErrorKind::Other,
    };
    let location = error.location.map(|location| ScriptSourceLocation {
        source: location.source.map(Arc::from),
        line: location.line,
        column: location.column,
    });
    let message = match (error.name, error.message) {
        (Some(name), message) if name.is_empty() => message,
        (Some(name), message) if message.is_empty() => name,
        (Some(name), message) => format!("{name}: {message}"),
        (None, message) => message,
    };
    ScriptError {
        kind,
        phase,
        message: Arc::from(message),
        location,
    }
}

fn script_error(
    kind: ScriptErrorKind,
    phase: ScriptErrorPhase,
    message: &'static str,
) -> ScriptError {
    ScriptError {
        kind,
        phase,
        message: Arc::from(message),
        location: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, mpsc};
    use std::{panic, thread};

    use super::*;

    fn engine_with(config: QuickJsConfig) -> ScriptEngine {
        ScriptEngine::with_config(config).expect("QuickJS realm")
    }

    /// Runs `operation` on its own thread so a wedged interrupt fails the test
    /// instead of hanging the suite.
    fn with_watchdog<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let outcome = panic::catch_unwind(panic::AssertUnwindSafe(operation));
            let _ = sender.send(outcome);
        });
        let outcome = receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| panic!("the QuickJS interrupt watchdog expired: {error}"));
        worker.join().expect("the watchdog worker captured a panic");
        match outcome {
            Ok(value) => value,
            Err(payload) => panic::resume_unwind(payload),
        }
    }

    #[test]
    fn execution_timeout_policy_is_opt_in() {
        let default = QuickJsConfig::default();
        assert_eq!(default.runtime_options.execution_timeout, None);
        assert_eq!(
            default
                .with_execution_timeout(Some(Duration::from_millis(20)))
                .runtime_options
                .execution_timeout,
            Some(Duration::from_millis(20))
        );
    }

    #[test]
    fn a_realm_executes_named_scripts() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        engine
            .execute_script("globalThis.answer = 42", "app:///main.js")
            .expect("execute");
    }

    #[test]
    fn preloaded_entry_modules_finish_before_execute_module_returns() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        engine
            .register_module_source(
                "app:///entry.js",
                "globalThis.answer = await Promise.resolve(42);",
            )
            .expect("register entry");
        engine
            .execute_module("await import('app:///entry.js');", "bobcat:boot")
            .expect("execute boot module");
        engine
            .execute_script(
                "if (globalThis.answer !== 42) throw new Error('entry was not awaited')",
                "verify.js",
            )
            .expect("entry completion must be visible");
    }

    #[test]
    fn a_missing_preloaded_module_is_a_named_module_error() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        let error = engine
            .execute_module("await import('app:///missing.mjs');", "bobcat:boot")
            .expect_err("the loader must reject an unknown module");

        assert_eq!(error.phase, ScriptErrorPhase::ExecuteModule);
        assert!(error.message.contains("app:///missing.mjs"));
        assert!(error.message.contains("not preloaded"));
    }

    #[test]
    fn host_functions_are_native_module_exports_not_globals() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        engine
            .register_host_module_function(
                "bobcat-internal:test",
                "answer",
                0,
                Box::new(|_| Ok(quickjs::HostValue::Number(42.0))),
            )
            .expect("register");
        engine
            .execute_module(
                "import { answer } from 'bobcat-internal:test';\n\
                 if (answer() !== 42) throw new Error('wrong answer');\n\
                 if (typeof globalThis.answer !== 'undefined') throw new Error('host export leaked');\n\
                 if (typeof globalThis.bobcat !== 'undefined') throw new Error('host object leaked');",
                "host-test.mjs",
            )
            .expect("call host function");
    }

    #[test]
    fn rust_calls_back_through_a_loaded_source_module_export() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        engine
            .register_module_source(
                "bobcat:callback",
                "export function receive(value) { globalThis.received = value; }",
            )
            .expect("register callback module");
        engine
            .execute_module("import 'bobcat:callback';", "bobcat:boot")
            .expect("load callback module");

        assert!(
            engine
                .call_module_export(
                    "bobcat:callback",
                    "receive",
                    &[quickjs::HostArgument::String("from Rust")],
                )
                .expect("call callback export")
        );
        assert!(
            !engine
                .call_module_export("bobcat:callback", "missing", &[])
                .expect("an absent export is not an engine failure")
        );
        engine
            .execute_script(
                "if (globalThis.received !== 'from Rust') throw new Error('callback did not run')",
                "verify.js",
            )
            .expect("callback effect must be visible");
    }

    #[test]
    fn execute_runs_a_microtask_checkpoint() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        engine
            .execute_script(
                "globalThis.answer = 0; Promise.resolve().then(() => answer = 42)",
                "app:///schedule.js",
            )
            .expect("execute and checkpoint");
        engine
            .execute_script(
                "if (answer !== 42) throw new Error('checkpoint did not run')",
                "app:///verify.js",
            )
            .expect("microtask ran");
    }

    #[test]
    fn source_name_is_preserved_in_sanitized_errors() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        let error = engine
            .execute_script("const = 1", "app:///broken.js")
            .expect_err("syntax error");
        assert_eq!(error.kind, ScriptErrorKind::Syntax);
        assert_eq!(
            error.location.and_then(|location| location.source),
            Some(Arc::from("app:///broken.js"))
        );
    }

    #[test]
    fn a_thrown_error_keeps_its_constructor_name_and_stays_an_exception() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");

        let error = engine
            .execute_script("throw new TypeError('invalid receiver')", "throw.js")
            .expect_err("a throw fails");
        assert_eq!(error.kind, ScriptErrorKind::Exception);
        assert_eq!(error.message.as_ref(), "TypeError: invalid receiver");

        // A `SyntaxError` *object* thrown at run time is still an exception:
        // `Syntax` names a source that would not parse, which this one did.
        let thrown = engine
            .execute_script(
                "throw new SyntaxError('a runtime object')",
                "throw-syntax.js",
            )
            .expect_err("a throw fails");
        assert_eq!(thrown.kind, ScriptErrorKind::Exception);
    }

    #[test]
    fn two_realms_do_not_share_globals() {
        let mut first = ScriptEngine::new().expect("QuickJS realm");
        let mut second = ScriptEngine::new().expect("QuickJS realm");

        first
            .execute_script("globalThis.answer = 42", "first.js")
            .expect("execute");
        second
            .execute_script(
                "if (typeof answer !== 'undefined') throw new Error('realms share a global')",
                "second.js",
            )
            .expect("the second realm has its own global object");
    }

    #[test]
    fn every_boundary_primitive_round_trips_through_a_host_function() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        let seen: Arc<Mutex<Vec<quickjs::HostValue>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        engine
            .register_host_module_function(
                "bobcat-internal:test",
                "echo",
                1,
                Box::new(move |arguments| {
                    let value = arguments
                        .first()
                        .cloned()
                        .unwrap_or(quickjs::HostValue::Undefined);
                    recorder
                        .lock()
                        .expect("the recorder is not poisoned")
                        .push(value.clone());
                    Ok(value)
                }),
            )
            .expect("register");

        engine
            .execute_module(
                r"
                import { echo } from 'bobcat-internal:test';
                const cases = [undefined, null, true, -0, 'a\u{1F980}b'];
                for (const value of cases) {
                    const echoed = echo(value);
                    if (!Object.is(echoed, value)) {
                        throw new Error('echo changed ' + String(value));
                    }
                }
                if (!Number.isNaN(echo(NaN))) throw new Error('NaN did not survive');
                ",
                "round-trip.mjs",
            )
            .expect("every primitive crosses in both directions unchanged");

        let seen = seen.lock().expect("the recorder is not poisoned");
        assert!(matches!(seen[0], quickjs::HostValue::Undefined));
        assert!(matches!(seen[1], quickjs::HostValue::Null));
        assert!(matches!(seen[2], quickjs::HostValue::Boolean(true)));
        assert!(matches!(seen[3], quickjs::HostValue::Number(value) if value.is_sign_negative()));
        assert!(matches!(&seen[4], quickjs::HostValue::String(value) if value == "a🦀b"));
    }

    #[test]
    fn an_unhandled_rejection_fails_the_checkpoint_that_follows_the_script() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        let error = engine
            .execute_script(
                "Promise.reject(new Error('microtask rejection'))",
                "app:///reject.js",
            )
            .expect_err("the checkpoint reports the rejection the script left behind");

        assert_eq!(error.kind, ScriptErrorKind::Exception);
        assert_eq!(error.phase, ScriptErrorPhase::Execute);
        assert_eq!(error.message.as_ref(), "Error: microtask rejection");
    }

    #[test]
    fn a_checkpoint_error_beside_a_primary_one_is_reported_before_the_next_script() {
        let mut engine = ScriptEngine::new().expect("QuickJS realm");
        let primary = engine
            .execute_script(
                "Promise.reject(new Error('deferred')); throw new Error('primary')",
                "app:///both.js",
            )
            .expect_err("the script's own exception wins");
        assert_eq!(primary.message.as_ref(), "Error: primary");

        let deferred = engine
            .execute_script("globalThis.reentered = true", "app:///after.js")
            .expect_err("the deferred checkpoint error is reported first");
        assert_eq!(deferred.kind, ScriptErrorKind::Exception);
        assert_eq!(deferred.message.as_ref(), "Error: deferred");

        engine
            .execute_script(
                "if (typeof reentered !== 'undefined') throw new Error('the script ran anyway')",
                "app:///verify.js",
            )
            .expect("the refused script never entered JavaScript");
    }

    #[test]
    fn a_checkpoint_at_exactly_the_job_limit_is_not_an_error() {
        let mut engine = engine_with(QuickJsConfig {
            max_jobs_per_checkpoint: NonZeroUsize::MIN,
            ..QuickJsConfig::default()
        });
        engine
            .execute_script(
                "globalThis.jobs = 0; Promise.resolve().then(() => jobs = 1)",
                "one-job.js",
            )
            .expect("one job fits a one-job checkpoint");
        engine
            .execute_script(
                "if (jobs !== 1) throw new Error('the job did not run')",
                "verify.js",
            )
            .expect("the single job ran");
    }

    #[test]
    fn exceeding_the_job_limit_is_an_error_and_the_rest_runs_before_reentry() {
        let mut engine = engine_with(QuickJsConfig {
            max_jobs_per_checkpoint: NonZeroUsize::MIN,
            ..QuickJsConfig::default()
        });
        let error = engine
            .execute_script(
                "globalThis.order = [];
                 Promise.resolve().then(() => order.push('old-1'));
                 Promise.resolve().then(() => order.push('old-2'))",
                "two-jobs.js",
            )
            .expect_err("two jobs exceed a one-job checkpoint");
        assert_eq!(error.kind, ScriptErrorKind::Other);
        assert_eq!(error.phase, ScriptErrorPhase::Execute);

        engine
            .execute_script(
                "order.push('new');
                 if (order.join(',') !== 'old-1,old-2,new') {
                     throw new Error('left-over jobs ran late: ' + order.join(','));
                 }",
                "reentry.js",
            )
            .expect("the queued job finishes before the next script's own code");
    }

    #[test]
    fn an_execution_timeout_is_reported_and_leaves_the_engine_usable() {
        let (error, reusable) = with_watchdog(|| {
            let mut engine = engine_with(
                QuickJsConfig::default().with_execution_timeout(Some(Duration::from_millis(20))),
            );
            let error = engine
                .execute_script("for (;;) {}", "app:///spin.js")
                .expect_err("an endless script must be interrupted");
            let reusable = engine
                .execute_script("globalThis.answer = 6 * 7", "app:///after.js")
                .is_ok();
            (error, reusable)
        });

        assert_eq!(error.kind, ScriptErrorKind::Other);
        assert_eq!(error.phase, ScriptErrorPhase::Execute);
        assert!(
            error.message.contains("timeout"),
            "the interrupt must be the configured timeout, not some other Other: {}",
            error.message
        );
        assert!(
            reusable,
            "an interrupted realm stays usable: the timeout is a script failure, not a teardown"
        );
    }
}
