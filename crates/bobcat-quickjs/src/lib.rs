//! QuickJS-backed runtime composition for [`bobcat_engine`].
//!
//! The public API exposes an opaque [`QuickJsLynxView`], its [`QuickJsConfig`]
//! and construction helpers. The concrete script adapter, realm values and
//! direct source-evaluation controls remain implementation details. The
//! lower-level [`quickjs_rust_bridge`] crate remains independently usable.

use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use bobcat_engine::resource::ResourceFetcher;
use bobcat_engine::script::{
    ScriptEngine, ScriptError, ScriptErrorKind, ScriptErrorPhase, ScriptFuture,
    ScriptSourceLocation, ScriptValue,
};
use bobcat_engine::view::{EngineMetrics, LynxView, LynxWidgetApi};
use quickjs_rust_bridge as quickjs;

/// Default maximum number of promise jobs run by one checkpoint.
pub const DEFAULT_MAX_JOBS_PER_CHECKPOINT: NonZeroUsize =
    NonZeroUsize::new(1_024).expect("the default job limit is non-zero");

/// Default wall-time limit for one JavaScript entry or promise-job checkpoint.
pub const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(5);

/// Runtime policy for a [`QuickJsLynxView`].
///
/// The job limit is non-zero by construction, and the default execution
/// timeout is finite. Together they prevent one script entry or checkpoint
/// from monopolizing the owner thread indefinitely. The timeout can be
/// disabled explicitly for embedders that provide another interruption
/// policy.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct QuickJsConfig {
    realm_options: quickjs::RealmOptions,
    max_jobs_per_checkpoint: NonZeroUsize,
}

impl QuickJsConfig {
    /// Set the `QuickJS` heap limit in bytes, or `None` for no explicit limit.
    #[must_use]
    pub const fn with_memory_limit(mut self, memory_limit: Option<usize>) -> Self {
        self.realm_options.memory_limit = memory_limit;
        self
    }

    /// Set the `QuickJS` native-stack limit, or `None` for its default.
    #[must_use]
    pub const fn with_max_stack_size(mut self, max_stack_size: Option<usize>) -> Self {
        self.realm_options.max_stack_size = max_stack_size;
        self
    }

    /// Set the wall-time limit for one JavaScript entry or job checkpoint.
    ///
    /// `None` disables deadline-based interruption. The limit remains
    /// cooperative and cannot preempt a blocking native host callback.
    #[must_use]
    pub const fn with_execution_timeout(mut self, execution_timeout: Option<Duration>) -> Self {
        self.realm_options.execution_timeout = execution_timeout;
        self
    }

    /// Replace the maximum number of jobs run by one checkpoint.
    #[must_use]
    pub const fn with_max_jobs_per_checkpoint(
        mut self,
        max_jobs_per_checkpoint: NonZeroUsize,
    ) -> Self {
        self.max_jobs_per_checkpoint = max_jobs_per_checkpoint;
        self
    }

    /// Return the maximum number of jobs run by one checkpoint.
    #[must_use]
    pub const fn max_jobs_per_checkpoint(self) -> NonZeroUsize {
        self.max_jobs_per_checkpoint
    }

    /// Return the configured `QuickJS` heap limit in bytes.
    #[must_use]
    pub const fn memory_limit(self) -> Option<usize> {
        self.realm_options.memory_limit
    }

    /// Return the configured `QuickJS` native-stack limit in bytes.
    #[must_use]
    pub const fn max_stack_size(self) -> Option<usize> {
        self.realm_options.max_stack_size
    }

    /// Return the wall-time limit for one JavaScript entry or job checkpoint.
    #[must_use]
    pub const fn execution_timeout(self) -> Option<Duration> {
        self.realm_options.execution_timeout
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

impl fmt::Debug for QuickJsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsConfig")
            .field("memory_limit", &self.memory_limit())
            .field("max_stack_size", &self.max_stack_size())
            .field("execution_timeout", &self.execution_timeout())
            .field("max_jobs_per_checkpoint", &self.max_jobs_per_checkpoint())
            .finish()
    }
}

/// Failure to allocate or initialize a `QuickJS` realm.
#[derive(Clone, PartialEq, Eq)]
pub struct QuickJsInitializationError {
    message: Arc<str>,
}

impl QuickJsInitializationError {
    fn from_quickjs(error: quickjs::Error) -> Self {
        Self {
            message: Arc::from(error.message),
        }
    }
}

impl fmt::Debug for QuickJsInitializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsInitializationError")
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for QuickJsInitializationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not initialize the QuickJS runtime: {}",
            self.message
        )
    }
}

impl std::error::Error for QuickJsInitializationError {}

#[derive(Clone)]
struct QuickJsCallable(quickjs::Value);

impl fmt::Debug for QuickJsCallable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsCallable")
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct QuickJsSymbol(quickjs::Value);

impl fmt::Debug for QuickJsSymbol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsSymbol")
            .finish_non_exhaustive()
    }
}

struct QuickJsScriptEngine {
    realm: quickjs::Realm,
    config: QuickJsConfig,
    checkpoint_incomplete: bool,
    deferred_checkpoint_error: Option<ScriptError>,
}

impl QuickJsScriptEngine {
    fn new() -> Result<Self, QuickJsInitializationError> {
        Self::with_config(QuickJsConfig::default())
    }

    fn with_config(config: QuickJsConfig) -> Result<Self, QuickJsInitializationError> {
        Ok(Self {
            realm: quickjs::Realm::with_options(config.realm_options)
                .map_err(QuickJsInitializationError::from_quickjs)?,
            config,
            checkpoint_incomplete: false,
            deferred_checkpoint_error: None,
        })
    }

    #[must_use]
    const fn config(&self) -> QuickJsConfig {
        self.config
    }

    fn evaluate_source(
        &mut self,
        source: quickjs::EvalSource<'_>,
    ) -> Result<ScriptValue<QuickJsCallable, QuickJsSymbol>, ScriptError> {
        self.resume_incomplete_checkpoint(ScriptErrorPhase::Evaluate)?;
        let result = self
            .realm
            .eval(source, quickjs::EvalOptions::default())
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Evaluate));
        let value = self.finish_operation(result, ScriptErrorPhase::Evaluate)?;
        quickjs_to_script_value(value, ScriptErrorPhase::Evaluate)
    }
}

impl fmt::Debug for QuickJsScriptEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsScriptEngine")
            .field("config", &self.config)
            .field("checkpoint_incomplete", &self.checkpoint_incomplete)
            .field(
                "has_deferred_checkpoint_error",
                &self.deferred_checkpoint_error.is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl ScriptEngine for QuickJsScriptEngine {
    type Callable = QuickJsCallable;
    type Symbol = QuickJsSymbol;

    fn evaluate(
        &mut self,
        source_text: &str,
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError> {
        self.evaluate_source(quickjs::EvalSource::new(source_text))
    }

    fn import_value<'a>(
        &'a mut self,
        _specifier: &'a str,
        _export_name: &'a str,
    ) -> ScriptFuture<'a, Self::Callable, Self::Symbol> {
        Box::pin(async {
            Err(script_error(
                ScriptErrorKind::ModuleLoad,
                ScriptErrorPhase::ImportValue,
                "QuickJS module loading is not configured",
            ))
        })
    }

    fn call(
        &mut self,
        callable: &Self::Callable,
        this_value: &ScriptValue<Self::Callable, Self::Symbol>,
        arguments: &[ScriptValue<Self::Callable, Self::Symbol>],
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError> {
        self.resume_incomplete_checkpoint(ScriptErrorPhase::Call)?;
        let this_value = self.script_to_quickjs_value(this_value, ScriptErrorPhase::Call)?;
        let arguments = arguments
            .iter()
            .map(|value| self.script_to_quickjs_value(value, ScriptErrorPhase::Call))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .realm
            .call(&callable.0, Some(&this_value), &arguments)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Call));
        let value = self.finish_operation(result, ScriptErrorPhase::Call)?;
        quickjs_to_script_value(value, ScriptErrorPhase::Call)
    }
}

impl QuickJsScriptEngine {
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
                // JavaScript may enqueue Promise jobs before throwing. Drain
                // them even when the main operation fails so older work never
                // runs after a later host-to-JavaScript entry. Preserve the
                // primary exception and defer any checkpoint failure.
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
            .drain_pending_jobs_bounded(self.config.max_jobs_per_checkpoint.get())
        {
            Ok(drain) => drain,
            Err(error) => {
                // The bridge can surface one unhandled rejection while more
                // remain in its rejection sidecar even when the QuickJS job
                // queue itself is empty. Conservatively require another
                // checkpoint before JavaScript re-entry after every failure.
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

    fn script_to_quickjs_value(
        &mut self,
        value: &ScriptValue<QuickJsCallable, QuickJsSymbol>,
        phase: ScriptErrorPhase,
    ) -> Result<quickjs::Value, ScriptError> {
        let result = match value {
            ScriptValue::Undefined => self.realm.undefined(),
            ScriptValue::Null => self.realm.null(),
            ScriptValue::Boolean(value) => self.realm.boolean(*value),
            ScriptValue::Number(value) => self.realm.number(*value),
            ScriptValue::BigInt(value) => self.realm.big_int_decimal(value),
            ScriptValue::String(value) => self.realm.string(value),
            ScriptValue::Symbol(value) => return Ok(value.0.clone()),
            ScriptValue::Callable(value) => return Ok(value.0.clone()),
            _ => {
                return Err(script_error(
                    ScriptErrorKind::InvalidBoundaryValue,
                    phase,
                    "this script boundary value is not supported by QuickJS",
                ));
            }
        };
        result.map_err(|error| map_quickjs_error(error, phase))
    }
}

fn quickjs_to_script_value(
    value: quickjs::Value,
    phase: ScriptErrorPhase,
) -> Result<ScriptValue<QuickJsCallable, QuickJsSymbol>, ScriptError> {
    match value.kind() {
        quickjs::ValueKind::Undefined => Ok(ScriptValue::Undefined),
        quickjs::ValueKind::Null => Ok(ScriptValue::Null),
        quickjs::ValueKind::Boolean => value
            .as_boolean()
            .map(ScriptValue::Boolean)
            .ok_or_else(|| conversion_error(phase, "QuickJS Boolean conversion failed")),
        quickjs::ValueKind::Number => value
            .as_number()
            .map(ScriptValue::Number)
            .ok_or_else(|| conversion_error(phase, "QuickJS Number conversion failed")),
        quickjs::ValueKind::BigInt => value
            .to_big_int_decimal()
            .map(|value| ScriptValue::BigInt(Arc::from(value)))
            .map_err(|error| map_quickjs_error(error, phase)),
        quickjs::ValueKind::String => value
            .to_utf16()
            .map_err(|error| map_quickjs_error(error, phase))
            .and_then(|units| {
                String::from_utf16(&units)
                    .map(|value| ScriptValue::String(Arc::from(value)))
                    .map_err(|_| {
                        script_error(
                            ScriptErrorKind::NonTransferableValue,
                            phase,
                            "an ill-formed UTF-16 string cannot cross Bobcat's UTF-8 boundary",
                        )
                    })
            }),
        quickjs::ValueKind::Symbol => Ok(ScriptValue::Symbol(QuickJsSymbol(value))),
        quickjs::ValueKind::Function => Ok(ScriptValue::Callable(QuickJsCallable(value))),
        quickjs::ValueKind::Object | quickjs::ValueKind::Other => Err(script_error(
            ScriptErrorKind::NonTransferableValue,
            phase,
            "ordinary QuickJS objects cannot cross the script boundary",
        )),
        _ => Err(script_error(
            ScriptErrorKind::NonTransferableValue,
            phase,
            "this QuickJS value kind cannot cross the script boundary",
        )),
    }
}

fn conversion_error(phase: ScriptErrorPhase, message: &'static str) -> ScriptError {
    script_error(ScriptErrorKind::Other, phase, message)
}

fn map_quickjs_error(error: quickjs::Error, phase: ScriptErrorPhase) -> ScriptError {
    let kind = match error.kind {
        quickjs::ErrorKind::Syntax => ScriptErrorKind::Syntax,
        quickjs::ErrorKind::Exception => ScriptErrorKind::Exception,
        quickjs::ErrorKind::InvalidInput if error.phase == quickjs::ErrorPhase::ConstructValue => {
            ScriptErrorKind::InvalidBoundaryValue
        }
        quickjs::ErrorKind::WrongRealm => ScriptErrorKind::WrongEngine,
        quickjs::ErrorKind::Interrupted | quickjs::ErrorKind::ExecutionTimeout => {
            // Interruption ends only the current entry; the realm remains reusable.
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

/// A QuickJS-backed Bobcat view whose script runtime is kept private.
///
/// Host code can access resources and widgets through this facade. Script
/// execution is driven by the runtime integration rather than by exposing the
/// concrete [`bobcat_engine::script::ScriptEngine`] implementation.
pub struct QuickJsLynxView<R: ResourceFetcher + ?Sized> {
    inner: LynxView<R, QuickJsScriptEngine>,
}

impl<R: ResourceFetcher + ?Sized> QuickJsLynxView<R> {
    /// Return this view's `QuickJS` runtime policy.
    #[must_use]
    pub const fn config(&self) -> QuickJsConfig {
        self.inner.script_engine().config()
    }

    /// Borrow the host resource fetcher.
    #[must_use]
    pub fn resource_fetcher(&self) -> &R {
        self.inner.resource_fetcher()
    }

    /// Borrow the shared ownership handle for the host resource fetcher.
    #[must_use]
    pub const fn shared_resource_fetcher(&self) -> &Arc<R> {
        self.inner.shared_resource_fetcher()
    }

    /// Borrow this view's widget API.
    #[must_use]
    pub const fn widget_api(&self) -> &LynxWidgetApi {
        self.inner.widget_api()
    }

    /// Mutably borrow this view's widget API.
    pub const fn widget_api_mut(&mut self) -> &mut LynxWidgetApi {
        self.inner.widget_api_mut()
    }
}

impl<R: ResourceFetcher + ?Sized> fmt::Debug for QuickJsLynxView<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuickJsLynxView")
            .field("config", &self.config())
            .field("widget_api", &self.widget_api())
            .finish_non_exhaustive()
    }
}

/// Create a QuickJS-backed view with default runtime policy.
pub fn new_quickjs_view<R: ResourceFetcher>(
    resource_fetcher: R,
    metrics: EngineMetrics,
) -> Result<QuickJsLynxView<R>, QuickJsInitializationError> {
    let script_engine = QuickJsScriptEngine::new()?;
    Ok(QuickJsLynxView {
        inner: LynxView::new(resource_fetcher, script_engine, metrics),
    })
}

/// Create a QuickJS-backed view with explicit runtime policy.
pub fn new_quickjs_view_with_config<R: ResourceFetcher>(
    resource_fetcher: R,
    metrics: EngineMetrics,
    config: QuickJsConfig,
) -> Result<QuickJsLynxView<R>, QuickJsInitializationError> {
    let script_engine = QuickJsScriptEngine::with_config(config)?;
    Ok(QuickJsLynxView {
        inner: LynxView::new(resource_fetcher, script_engine, metrics),
    })
}

#[cfg(test)]
mod tests;
