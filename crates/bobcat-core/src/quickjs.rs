//! Built-in `QuickJS` adapter for Bobcat's injected VM contract.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use quickjs_rust_bridge as quickjs;

use crate::script::{
    HostCallback, HostValue, ScriptEngine, ScriptEngineFactory, ScriptError, ScriptErrorKind,
    ScriptErrorPhase, ScriptSourceLocation,
};

const DEFAULT_MAX_JOBS_PER_CHECKPOINT: NonZeroUsize =
    NonZeroUsize::new(1_024).expect("the default job limit is non-zero");
const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuickJsConfig {
    realm_options: quickjs::RealmOptions,
    max_jobs_per_checkpoint: NonZeroUsize,
}

#[cfg(test)]
impl QuickJsConfig {
    #[must_use]
    const fn with_execution_timeout(mut self, execution_timeout: Option<Duration>) -> Self {
        self.realm_options.execution_timeout = execution_timeout;
        self
    }
}

impl Default for QuickJsConfig {
    fn default() -> Self {
        Self {
            realm_options: quickjs::RealmOptions {
                memory_limit: None,
                max_stack_size: None,
                execution_timeout: Some(DEFAULT_EXECUTION_TIMEOUT),
            },
            max_jobs_per_checkpoint: DEFAULT_MAX_JOBS_PER_CHECKPOINT,
        }
    }
}

#[derive(Debug)]
struct QuickJsFactory;

impl ScriptEngineFactory for QuickJsFactory {
    fn create(&self) -> Result<Box<dyn ScriptEngine>, ScriptError> {
        QuickJsScriptEngine::new()
            .map(|engine| Box::new(engine) as Box<dyn ScriptEngine>)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Initialize))
    }
}

/// Returns Bobcat's opaque built-in `QuickJS` VM factory.
///
/// The returned capability is `Send + Sync`; each owner-thread-bound realm is
/// allocated only when [`ScriptEngineFactory::create`] is called.
#[must_use]
pub fn engine_factory() -> Arc<dyn ScriptEngineFactory> {
    Arc::new(QuickJsFactory)
}

struct QuickJsScriptEngine {
    realm: quickjs::Realm,
    namespaces: HashMap<String, quickjs::Value>,
    config: QuickJsConfig,
    checkpoint_incomplete: bool,
    deferred_checkpoint_error: Option<ScriptError>,
}

impl QuickJsScriptEngine {
    fn new() -> Result<Self, quickjs::Error> {
        Self::with_config(QuickJsConfig::default())
    }

    fn with_config(config: QuickJsConfig) -> Result<Self, quickjs::Error> {
        Ok(Self {
            realm: quickjs::Realm::with_options(config.realm_options)?,
            namespaces: HashMap::new(),
            config,
            checkpoint_incomplete: false,
            deferred_checkpoint_error: None,
        })
    }

    fn namespace(&mut self, name: &str) -> Result<quickjs::Value, ScriptError> {
        if let Some(namespace) = self.namespaces.get(name) {
            return Ok(namespace.clone());
        }
        let namespace = self
            .realm
            .evaluate(
                quickjs::EvalSource {
                    name: Some("<bobcat host namespace>"),
                    ..quickjs::EvalSource::new("({})")
                },
                quickjs::EvalOptions::default(),
            )
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostFunction))?;
        let global = self
            .realm
            .global_object()
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostFunction))?;
        self.realm
            .set_property(&global, name, &namespace)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostFunction))?;
        self.namespaces.insert(name.to_owned(), namespace.clone());
        Ok(namespace)
    }

    fn execute_raw(
        &mut self,
        source: quickjs::EvalSource<'_>,
        phase: ScriptErrorPhase,
    ) -> Result<(), ScriptError> {
        self.resume_incomplete_checkpoint(phase)?;
        let result = self
            .realm
            .evaluate(source, quickjs::EvalOptions::default())
            .map(|_| ())
            .map_err(|error| map_quickjs_error(error, phase));
        self.finish_operation(result, phase)
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
            .realm
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
}

impl fmt::Debug for QuickJsScriptEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsScriptEngine")
            .field("config", &self.config)
            .field("checkpoint_incomplete", &self.checkpoint_incomplete)
            .finish_non_exhaustive()
    }
}

impl ScriptEngine for QuickJsScriptEngine {
    fn register_host_function(
        &mut self,
        namespace: &str,
        name: &str,
        arity: u8,
        mut callback: HostCallback,
    ) -> Result<(), ScriptError> {
        let namespace = self.namespace(namespace)?;
        let function_name = name.to_owned();
        let member = self
            .realm
            .function(name, u32::from(arity), move |arguments| {
                let arguments = arguments
                    .iter()
                    .map(host_value_from_quickjs)
                    .collect::<Result<Vec<_>, _>>()?;
                callback(&arguments)
                    .map(host_value_to_quickjs)
                    .map_err(quickjs::HostFunctionError::new)
            })
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostFunction))?;
        self.realm
            .set_property(&namespace, &function_name, &member)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostFunction))
    }

    fn execute_script(&mut self, source: &str, source_name: &str) -> Result<(), ScriptError> {
        self.execute_raw(
            quickjs::EvalSource {
                name: Some(source_name),
                ..quickjs::EvalSource::new(source)
            },
            ScriptErrorPhase::Execute,
        )
    }

    fn collect_garbage(&mut self) -> Result<(), ScriptError> {
        self.resume_incomplete_checkpoint(ScriptErrorPhase::CollectGarbage)?;
        self.realm.run_gc();
        self.checkpoint(ScriptErrorPhase::CollectGarbage)
            .map(|_| ())
    }
}

fn host_value_from_quickjs(
    value: &quickjs::HostValue,
) -> Result<HostValue, quickjs::HostFunctionError> {
    match value {
        quickjs::HostValue::Undefined => Ok(HostValue::Undefined),
        quickjs::HostValue::Null => Ok(HostValue::Null),
        quickjs::HostValue::Boolean(value) => Ok(HostValue::Boolean(*value)),
        quickjs::HostValue::Number(value) => Ok(HostValue::Number(*value)),
        quickjs::HostValue::String(value) => Ok(HostValue::String(Arc::from(value.as_str()))),
        _ => Err(quickjs::HostFunctionError::new(
            "this QuickJS value cannot cross Bobcat's host boundary",
        )),
    }
}

fn host_value_to_quickjs(value: HostValue) -> quickjs::HostValue {
    match value {
        HostValue::Undefined => quickjs::HostValue::Undefined,
        HostValue::Null => quickjs::HostValue::Null,
        HostValue::Boolean(value) => quickjs::HostValue::Boolean(value),
        HostValue::Number(value) => quickjs::HostValue::Number(value),
        HostValue::String(value) => quickjs::HostValue::String(value.to_string()),
    }
}

fn map_quickjs_error(error: quickjs::Error, phase: ScriptErrorPhase) -> ScriptError {
    let kind = match error.kind {
        quickjs::ErrorKind::Syntax => ScriptErrorKind::Syntax,
        quickjs::ErrorKind::Exception => ScriptErrorKind::Exception,
        quickjs::ErrorKind::InvalidInput if error.phase == quickjs::ErrorPhase::ConstructValue => {
            ScriptErrorKind::InvalidBoundaryValue
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

    fn assert_transferable<T: Send + Sync>() {}

    fn engine_with(config: QuickJsConfig) -> QuickJsScriptEngine {
        QuickJsScriptEngine::with_config(config).expect("QuickJS realm")
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
    fn execution_timeout_policy_is_configurable() {
        let default = QuickJsConfig::default();
        assert_eq!(
            default.realm_options.execution_timeout,
            Some(DEFAULT_EXECUTION_TIMEOUT)
        );
        assert_eq!(
            default
                .with_execution_timeout(None)
                .realm_options
                .execution_timeout,
            None
        );
    }

    #[test]
    fn factory_capability_is_transferable_while_realms_remain_owner_thread_bound() {
        assert_transferable::<Arc<dyn ScriptEngineFactory>>();
        let _factory = engine_factory();
    }

    #[test]
    fn factory_delays_realm_creation_and_executes_named_scripts() {
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
        engine
            .execute_script("globalThis.answer = 42", "app:///main.js")
            .expect("execute");
    }

    #[test]
    fn host_functions_are_installed_under_a_namespace() {
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
        engine
            .register_host_function(
                "bobcat",
                "answer",
                0,
                Box::new(|_| Ok(HostValue::Number(42.0))),
            )
            .expect("register");
        engine
            .execute_script(
                "if (bobcat.answer() !== 42) throw new Error('wrong answer')",
                "host-test.js",
            )
            .expect("call host function");
    }

    #[test]
    fn execute_runs_a_microtask_checkpoint() {
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
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
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
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
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");

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
    fn realms_from_one_factory_do_not_share_globals() {
        let factory = engine_factory();
        let mut first = factory.create().expect("QuickJS realm");
        let mut second = factory.create().expect("QuickJS realm");

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
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
        let seen: Arc<Mutex<Vec<HostValue>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        engine
            .register_host_function(
                "bobcat",
                "echo",
                1,
                Box::new(move |arguments| {
                    let value = arguments.first().cloned().unwrap_or(HostValue::Undefined);
                    recorder
                        .lock()
                        .expect("the recorder is not poisoned")
                        .push(value.clone());
                    Ok(value)
                }),
            )
            .expect("register");

        engine
            .execute_script(
                r"
                const cases = [undefined, null, true, -0, 'a\u{1F980}b'];
                for (const value of cases) {
                    const echoed = bobcat.echo(value);
                    if (!Object.is(echoed, value)) {
                        throw new Error('echo changed ' + String(value));
                    }
                }
                if (!Number.isNaN(bobcat.echo(NaN))) throw new Error('NaN did not survive');
                ",
                "round-trip.js",
            )
            .expect("every primitive crosses in both directions unchanged");

        let seen = seen.lock().expect("the recorder is not poisoned");
        assert!(matches!(seen[0], HostValue::Undefined));
        assert!(matches!(seen[1], HostValue::Null));
        assert!(matches!(seen[2], HostValue::Boolean(true)));
        assert!(matches!(seen[3], HostValue::Number(value) if value.is_sign_negative()));
        assert!(matches!(&seen[4], HostValue::String(value) if value.as_ref() == "a🦀b"));
    }

    #[test]
    fn a_non_primitive_argument_is_refused_at_the_host_boundary() {
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
        engine
            .register_host_function("bobcat", "echo", 1, Box::new(|_| Ok(HostValue::Undefined)))
            .expect("register");

        // The refusal is a JavaScript exception the script can observe, not a
        // lossy conversion: an object never reaches the callback at all.
        engine
            .execute_script(
                r"
                let refused = '';
                try { bobcat.echo({ answer: 42 }); } catch (error) { refused = String(error); }
                if (!refused.includes('String arguments only')) {
                    throw new Error('an object was not refused at the boundary: ' + refused);
                }
                ",
                "non-primitive.js",
            )
            .expect("the script observes the refusal and continues");
    }

    #[test]
    fn an_ill_formed_string_is_refused_at_the_host_boundary() {
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
        engine
            .register_host_function("bobcat", "echo", 1, Box::new(|_| Ok(HostValue::Undefined)))
            .expect("register");

        // `HostValue::String` is an `Arc<str>`, so a lone surrogate has no
        // representation on the Rust side; it is refused rather than replaced.
        engine
            .execute_script(
                r"
                let refused = '';
                try { bobcat.echo('\uD800'); } catch (error) { refused = String(error); }
                if (!refused.includes('ill-formed UTF-16')) {
                    throw new Error('a lone surrogate was not refused: ' + refused);
                }
                ",
                "surrogate.js",
            )
            .expect("the script observes the refusal");
    }

    #[test]
    fn an_unhandled_rejection_fails_the_checkpoint_that_follows_the_script() {
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
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
        let factory = engine_factory();
        let mut engine = factory.create().expect("QuickJS realm");
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
