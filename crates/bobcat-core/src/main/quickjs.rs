//! The main thread's script engine: one `QuickJS` runtime, the realms a
//! group opens on it, and the policy around them.
//!
//! Runtime and realms live in `quickjs-rust-bridge`. What this module adds is
//! everything the runtime needs and the bridge deliberately leaves open: when
//! the promise-job queue is drained, which realm a drained failure is
//! reported to, how a bridge failure becomes
//! a sanitized [`ScriptError`], that a host function's panic is resumed as a
//! panic once the entry that called it ends, and how a realm keeps the
//! namespace and the atom each [`Export`] is called through.

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::rc::{Rc, Weak};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use quickjs_rust_bridge as quickjs;

use crate::code_cache::RealmCodeCache;
use crate::esm::Export;
use crate::jobs::JsThreadHandle;
use crate::realm::child::ChildRealms;
use crate::script::{ScriptError, ScriptErrorKind, ScriptErrorPhase, ScriptSourceLocation};

type HostPanic = Rc<Cell<Option<Box<dyn Any + Send>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuickJsConfig {
    runtime_options: quickjs::RuntimeOptions,
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
                strip_source: false,
            },
        }
    }
}

pub(crate) struct ScriptEngine {
    realm: quickjs::Context,
    exports: [OnceCell<(quickjs::Value, quickjs::Member)>; Export::COUNT],
    evaluation: RefCell<Option<quickjs::Value>>,
    host_panic: HostPanic,
    children: Weak<ChildRealms>,
    code: Rc<RealmCodeCache>,
}

pub(crate) struct ScriptRuntime {
    runtime: quickjs::Runtime,
    config: QuickJsConfig,
    checkpoint: tokio::sync::watch::Sender<u64>,
    artifacts: Arc<vant::artifact::Artifacts>,
}

impl ScriptRuntime {
    pub(crate) fn new() -> Result<Self, ScriptError> {
        Self::with_config(QuickJsConfig::default())
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Initialize))
    }

    fn with_config(config: QuickJsConfig) -> Result<Self, quickjs::Error> {
        Ok(Self {
            runtime: quickjs::Runtime::with_options(config.runtime_options)?,
            config,
            checkpoint: tokio::sync::watch::channel(0).0,
            artifacts: Arc::clone(vant::artifact::Artifacts::global()),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_artifacts(
        artifacts: Arc<vant::artifact::Artifacts>,
    ) -> Result<Self, ScriptError> {
        let mut runtime = Self::with_config(QuickJsConfig::default())
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Initialize))?;
        runtime.artifacts = artifacts;
        Ok(runtime)
    }

    pub(crate) fn checkpoints(&self) -> tokio::sync::watch::Receiver<u64> {
        self.checkpoint.subscribe()
    }

    pub(crate) fn checkpoint_generation(&self) -> u64 {
        *self.checkpoint.borrow()
    }

    pub(crate) fn mark_checkpoint(&self) {
        self.checkpoint
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    pub(crate) fn memory_usage(&self) -> quickjs::MemoryUsage {
        self.runtime.memory_usage()
    }

    pub(crate) fn create_realm(&self) -> Result<ScriptEngine, ScriptError> {
        let realm = self
            .runtime
            .create_context()
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Initialize))?;
        let code = RealmCodeCache::new(Arc::clone(&self.artifacts), realm.strips_source());
        Ok(ScriptEngine::adopt(
            realm,
            HostPanic::default(),
            Rc::new(code),
        ))
    }

    #[cfg(test)]
    pub(crate) fn register_module_source(
        &mut self,
        specifier: &str,
        source: &str,
    ) -> Result<(), ScriptError> {
        self.runtime
            .register_module_source(specifier, source)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterModule))
    }

    pub(crate) fn register_module_bytecode(
        &mut self,
        specifier: &str,
        bytecode: &'static [u8],
    ) -> Result<(), ScriptError> {
        self.runtime
            .register_module_bytecode(specifier, bytecode)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterModule))
    }

    pub(crate) fn reserve_module_prefix(&mut self, prefix: &str) -> Result<(), ScriptError> {
        self.runtime
            .reserve_module_prefix(prefix)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterModule))
    }
}

pub(crate) type SharedRuntime = Rc<RefCell<Result<ScriptRuntime, ScriptError>>>;

pub(crate) fn mark_checkpoint_later(js: &SharedRuntime, thread: &JsThreadHandle) {
    let js = Rc::clone(js);
    drop(thread.run(move || {
        if let Ok(js) = &*js.borrow() {
            js.mark_checkpoint();
        }
    }));
}

impl ScriptEngine {
    fn adopt(realm: quickjs::Context, host_panic: HostPanic, code: Rc<RealmCodeCache>) -> Self {
        Self {
            realm,
            exports: [const { OnceCell::new() }; Export::COUNT],
            evaluation: RefCell::new(None),
            host_panic,
            children: Weak::new(),
            code,
        }
    }

    fn file_compiled(&self) {
        while let Some(compiled) = self.realm.take_compiled() {
            self.code.insert(compiled);
        }
    }

    pub(crate) fn set_children(&mut self, children: Weak<ChildRealms>) {
        self.children = children;
    }

    pub(crate) fn install_shadow_realm(
        &mut self,
        mut hook: impl FnMut(ScriptEngine) -> Result<(), String> + 'static,
    ) -> Result<(), ScriptError> {
        let host_panic = Rc::clone(&self.host_panic);
        let code = Rc::clone(&self.code);
        self.realm
            .install_shadow_realm(move |child| {
                let child =
                    ScriptEngine::adopt(child, Rc::clone(&host_panic), Rc::new(code.sibling()));
                keep_host_panic(&host_panic, "the ShadowRealm host hook panicked", || {
                    hook(child)
                })
            })
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::InstallShadowRealm))
    }

    pub(crate) fn is_referenced_across_realms(&self) -> bool {
        self.realm.is_referenced_across_realms()
    }

    pub(crate) fn take_unhandled_rejection(
        &self,
        runtime: &mut ScriptRuntime,
    ) -> Result<usize, ScriptError> {
        let drained = runtime.runtime.drain_pending_jobs(&self.realm);
        self.file_compiled();
        if let Some(payload) = self.host_panic.take() {
            panic::resume_unwind(payload);
        }
        drained.map_err(|error| {
            runtime.runtime.discard_unhandled_rejections(&self.realm);
            map_quickjs_error(error, ScriptErrorPhase::ShadowRealmCheckpoint)
        })
    }

    pub(crate) fn discard_own_rejections(&self, runtime: &mut ScriptRuntime) {
        runtime.runtime.discard_unhandled_rejections(&self.realm);
    }

    fn finish_operation<T>(
        &self,
        runtime: &mut ScriptRuntime,
        result: Result<T, ScriptError>,
        phase: ScriptErrorPhase,
    ) -> Result<T, ScriptError> {
        match result {
            Ok(value) => match self.checkpoint(runtime, phase) {
                Ok(_) => Ok(value),
                Err(checkpoint_error) => {
                    self.discard_leftover_rejections(runtime);
                    Err(checkpoint_error)
                }
            },
            Err(primary_error) => {
                let _ = self.checkpoint(runtime, phase);
                self.discard_leftover_rejections(runtime);
                Err(primary_error)
            }
        }
    }

    fn discard_leftover_rejections(&self, runtime: &mut ScriptRuntime) {
        self.discard_own_rejections(runtime);
        if let Some(children) = self.children.upgrade() {
            children.discard_rejections(runtime);
        }
    }

    fn checkpoint(
        &self,
        runtime: &mut ScriptRuntime,
        phase: ScriptErrorPhase,
    ) -> Result<usize, ScriptError> {
        let drained = runtime.runtime.drain_pending_jobs(&self.realm);
        runtime.mark_checkpoint();
        self.file_compiled();
        if let Some(payload) = self.host_panic.take() {
            panic::resume_unwind(payload);
        }
        let executed = drained.map_err(|error| map_quickjs_error(error, phase))?;
        Ok(executed)
    }

    pub(crate) fn collect_garbage(&self, runtime: &mut ScriptRuntime) -> Result<(), ScriptError> {
        runtime.runtime.run_gc();
        self.checkpoint(runtime, ScriptErrorPhase::CollectGarbage)
            .map(|_| ())
    }

    pub(crate) fn enable_module_loading(&mut self) {
        self.realm.enable_module_loading(normalize_module_url);
    }

    pub(crate) fn take_module_request(&self) -> Option<String> {
        self.realm.take_module_request()
    }

    pub(crate) fn complete_module(
        &self,
        runtime: &mut ScriptRuntime,
        name: &str,
        result: Result<(&str, &str), &str>,
    ) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::ExecuteModule;
        let result = match result {
            Ok((url, _)) if url.contains('\0') => Err(format!(
                "module '{name}': the response URL contains a NUL byte"
            )),
            Ok(loaded) => Ok(loaded),
            Err(error) => Err(error.replace('\0', "\u{fffd}")),
        };
        let completed = match result {
            Ok((url, text)) => match self.code.module_code(name, text) {
                crate::code_cache::ModuleCode::Compile => {
                    self.realm.complete_module(name, Ok((url, text)))
                }
                code => self
                    .realm
                    .complete_module_with(name, url, text, code.as_bridge()),
            },
            Err(error) => self.realm.complete_module(name, Err(error.as_str())),
        };
        completed.map_err(|error| map_quickjs_error(error, PHASE))?;
        let result = self
            .realm
            .resume_module_loads()
            .map_err(|error| map_quickjs_error(error, PHASE));
        self.finish_operation(runtime, result, PHASE)
    }

    pub(crate) fn complete_module_bytecode(
        &self,
        runtime: &mut ScriptRuntime,
        name: &str,
        bytecode: &[u8],
    ) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::ExecuteModule;
        self.realm
            .complete_module_with(name, name, "", quickjs::ModuleCode::Bytecode(bytecode))
            .map_err(|error| map_quickjs_error(error, PHASE))?;
        let result = self
            .realm
            .resume_module_loads()
            .map_err(|error| map_quickjs_error(error, PHASE));
        self.finish_operation(runtime, result, PHASE)
    }

    pub(crate) fn module_finished(&self) -> Result<bool, ScriptError> {
        let result = {
            let evaluation = self.evaluation.borrow();
            let Some(evaluation) = evaluation.as_ref() else {
                return Ok(true);
            };
            self.realm
                .settled_promise_result(evaluation)
                .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::ExecuteModule))
        };
        if !matches!(result, Ok(None)) {
            *self.evaluation.borrow_mut() = None;
        }
        result.map(|value| value.is_some())
    }

    pub(crate) fn register_host_module_function<F>(
        &self,
        module_specifier: &str,
        export_name: &str,
        arity: u8,
        mut callback: F,
    ) -> Result<(), ScriptError>
    where
        F: FnMut(&[quickjs::HostArgument<'_>]) -> Result<quickjs::HostValue, String> + 'static,
    {
        let host_panic = Rc::clone(&self.host_panic);
        self.realm
            .register_host_module_function(
                module_specifier,
                export_name,
                u32::from(arity),
                move |arguments| {
                    keep_host_panic(&host_panic, "the host function panicked", || {
                        callback(arguments)
                    })
                    .map_err(quickjs::HostFunctionError::new)
                },
            )
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostModuleFunction))
    }

    pub(crate) fn register_reentrant_host_module_function<F>(
        &self,
        module_specifier: &str,
        export_name: &str,
        arity: u8,
        callback: F,
    ) -> Result<(), ScriptError>
    where
        F: Fn(&[quickjs::HostArgument<'_>]) -> Result<quickjs::HostValue, String> + 'static,
    {
        let host_panic = Rc::clone(&self.host_panic);
        self.realm
            .register_host_module_function_reentrant(
                module_specifier,
                export_name,
                u32::from(arity),
                move |arguments| {
                    keep_host_panic(&host_panic, "the host function panicked", || {
                        callback(arguments)
                    })
                    .map_err(quickjs::HostFunctionError::new)
                },
            )
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostModuleFunction))
    }

    pub(crate) fn register_host_module_shared_object(
        &mut self,
        module_specifier: &str,
        export_name: &str,
    ) -> Result<(), ScriptError> {
        self.realm
            .register_host_module_shared_object(module_specifier, export_name)
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostModuleFunction))
    }

    pub(crate) fn register_synchronous_loader<F>(
        &mut self,
        module_specifier: &str,
        export_name: &str,
        mut load: F,
    ) -> Result<(), ScriptError>
    where
        F: FnMut(&str) -> Result<quickjs::RequiredSource, String> + 'static,
    {
        let host_panic = Rc::clone(&self.host_panic);
        let code = Rc::clone(&self.code);
        self.realm
            .register_synchronous_loader_with_code(
                module_specifier,
                export_name,
                move |url, parameters| {
                    keep_host_panic(&host_panic, "the require loader panicked", || {
                        load(url).map(|source| code.required(url, parameters, source))
                    })
                },
            )
            .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterHostModuleFunction))
    }

    #[cfg(test)]
    fn execute_script(
        &self,
        runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::Execute;
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
        self.finish_operation(runtime, result, PHASE)
    }

    #[cfg(test)]
    fn execute_module(
        &self,
        runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), ScriptError> {
        self.start_module(runtime, source, source_name)?;
        if self.module_finished()? {
            Ok(())
        } else {
            Err(script_error(
                ScriptErrorKind::ModuleEvaluate,
                ScriptErrorPhase::ExecuteModule,
                "QuickJS module evaluation remained pending after its job checkpoint",
            ))
        }
    }

    pub(crate) fn start_module(
        &self,
        runtime: &mut ScriptRuntime,
        source: &str,
        source_name: &str,
    ) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::ExecuteModule;
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
        let evaluation = self.finish_operation(runtime, evaluation, PHASE)?;
        *self.evaluation.borrow_mut() = Some(evaluation);
        self.module_finished().map(|_| ())
    }

    pub(crate) fn load_root_module(
        &self,
        runtime: &mut ScriptRuntime,
        name: &str,
    ) -> Result<(), ScriptError> {
        const PHASE: ScriptErrorPhase = ScriptErrorPhase::ExecuteModule;
        let load = self
            .realm
            .load_module(name)
            .map_err(|error| map_quickjs_error(error, PHASE));
        let load = self.finish_operation(runtime, load, PHASE)?;
        *self.evaluation.borrow_mut() = Some(load);
        self.module_finished().map(|_| ())
    }

    pub(crate) fn call_module_export(
        &self,
        runtime: &mut ScriptRuntime,
        export: Export,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<bool, ScriptError> {
        let result = self.invoke_module_export(export, arguments)?;
        self.finish_operation(runtime, result, ScriptErrorPhase::CallModuleExport)
    }

    #[cfg(test)]
    fn call_named_export(
        &self,
        runtime: &mut ScriptRuntime,
        specifier: &str,
        export_name: &str,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<bool, ScriptError> {
        let (object, member) = resolve_export(&self.realm, specifier, export_name)?;
        let result = call_resolved(&self.realm, &object, &member, arguments);
        self.finish_operation(runtime, result, ScriptErrorPhase::CallModuleExport)
    }

    pub(crate) fn call_module_export_before_operation(
        &self,
        runtime: &mut ScriptRuntime,
        export: Export,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<bool, ScriptError> {
        let result = self.invoke_module_export(export, arguments)?;
        self.finish_call_before_operation(runtime, result)
    }

    #[cfg(test)]
    fn call_named_export_before_operation(
        &self,
        runtime: &mut ScriptRuntime,
        specifier: &str,
        export_name: &str,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<bool, ScriptError> {
        let (object, member) = resolve_export(&self.realm, specifier, export_name)?;
        let result = call_resolved(&self.realm, &object, &member, arguments);
        self.finish_call_before_operation(runtime, result)
    }

    fn finish_call_before_operation(
        &self,
        runtime: &mut ScriptRuntime,
        result: Result<bool, ScriptError>,
    ) -> Result<bool, ScriptError> {
        match result {
            Ok(called) => Ok(called),
            failed @ Err(_) => {
                self.finish_operation(runtime, failed, ScriptErrorPhase::CallModuleExport)
            }
        }
    }

    pub(crate) fn checkpoint_after_calls(
        &self,
        runtime: &mut ScriptRuntime,
    ) -> Result<(), ScriptError> {
        self.finish_operation(runtime, Ok(()), ScriptErrorPhase::CallModuleExport)
    }

    fn invoke_module_export(
        &self,
        export: Export,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<Result<bool, ScriptError>, ScriptError> {
        let slot = &self.exports[export as usize];
        if slot.get().is_none() {
            let resolved = resolve_export(&self.realm, export.specifier(), export.name())?;
            let _ = slot.set(resolved);
        }
        let (object, member) = slot.get().expect("the export was just resolved");
        Ok(call_resolved(&self.realm, object, member, arguments))
    }

    pub(crate) fn call_module_export_nested(
        &self,
        export: Export,
        arguments: &[quickjs::HostArgument<'_>],
    ) -> Result<bool, ScriptError> {
        self.invoke_module_export(export, arguments)?
    }
}

#[cfg(feature = "devtools")]
impl ScriptEngine {
    pub(crate) const fn devtools_realm(&self) -> &quickjs::Context {
        &self.realm
    }

    pub(crate) fn devtools_run<T>(
        &self,
        runtime: &mut ScriptRuntime,
        run: impl FnOnce(&quickjs::Context) -> T,
    ) -> (T, Result<(), ScriptError>) {
        let value = run(&self.realm);
        (value, self.checkpoint_after_calls(runtime))
    }

    pub(crate) fn devtools_function<F>(
        &self,
        name: &str,
        arity: u32,
        handler: F,
    ) -> Result<quickjs::Value, quickjs::Error>
    where
        F: Fn(&quickjs::Context, &[quickjs::Value]) -> Result<quickjs::HostValue, String> + 'static,
    {
        let host_panic = Rc::clone(&self.host_panic);
        self.realm
            .function_with_values(name, arity, move |realm, arguments| {
                keep_host_panic(&host_panic, "the host function panicked", || {
                    handler(realm, arguments)
                })
                .map_err(quickjs::HostFunctionError::new)
            })
    }
}

fn resolve_export(
    realm: &quickjs::Context,
    specifier: &str,
    export_name: &str,
) -> Result<(quickjs::Value, quickjs::Member), ScriptError> {
    let object = realm
        .module_namespace(specifier)
        .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::CallModuleExport))?;
    let member = realm
        .member(export_name)
        .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::CallModuleExport))?;
    Ok((object, member))
}

fn call_resolved(
    realm: &quickjs::Context,
    object: &quickjs::Value,
    member: &quickjs::Member,
    arguments: &[quickjs::HostArgument<'_>],
) -> Result<bool, ScriptError> {
    realm
        .call_member(object, member, arguments)
        .map(|outcome| match outcome {
            quickjs::CallOutcome::MemberAbsent => false,
            quickjs::CallOutcome::Called(_) => true,
        })
        .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::CallModuleExport))
}

fn keep_host_panic<T>(
    host_panic: &HostPanic,
    message: &str,
    call: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    panic::catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|payload| {
        let kept = host_panic.take();
        host_panic.set(kept.or(Some(payload)));
        Err(message.to_owned())
    })
}

pub(crate) fn normalize_module_url(base: &str, specifier: &str) -> Result<String, String> {
    if specifier == crate::esm::WORKER_CLASS_MODULE_SPECIFIER
        || specifier.starts_with(crate::esm::NATIVE_MODULE_ESM_PREFIX)
        || crate::esm::ENGINE_MODULE_PREFIXES
            .iter()
            .any(|prefix| specifier.starts_with(prefix))
    {
        return Ok(specifier.to_owned());
    }
    if let Ok(url) = url::Url::parse(specifier) {
        return Ok(url.into());
    }
    if specifier.starts_with("./") || specifier.starts_with("../") || specifier.starts_with('/') {
        return url::Url::parse(base)
            .and_then(|url| url.join(specifier))
            .map(Into::into)
            .map_err(|error| {
                format!("cannot resolve module '{specifier}' from '{base}': {error}")
            });
    }
    Err(format!(
        "bare module specifier '{specifier}' is not supported"
    ))
}

impl fmt::Debug for ScriptEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptEngine")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ScriptRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScriptRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

pub(crate) fn compile_module_framed(name: &str, source: &str) -> Result<Vec<u8>, ScriptError> {
    let compiler = quickjs::Runtime::with_options(quickjs::RuntimeOptions {
        strip_source: true,
        ..QuickJsConfig::default().runtime_options
    })
    .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::Initialize))?;
    compiler
        .create_context()
        .and_then(|realm| realm.compile_module_framed(name, source))
        .map_err(|error| map_quickjs_error(error, ScriptErrorPhase::RegisterModule))
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

#[cfg(test)]
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
#[path = "quickjs_code_cache_tests.rs"]
mod code_cache_tests;

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::{panic, thread};

    use super::*;

    fn engine_with(config: QuickJsConfig) -> (ScriptRuntime, ScriptEngine) {
        let runtime = ScriptRuntime::with_config(config).expect("QuickJS runtime");
        let realm = runtime.create_realm().expect("QuickJS realm");
        (runtime, realm)
    }

    fn engine() -> (ScriptRuntime, ScriptEngine) {
        engine_with(QuickJsConfig::default())
    }

    fn with_watchdog<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = flume::bounded(1);
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
        let (mut runtime, engine) = engine();
        engine
            .execute_script(&mut runtime, "globalThis.answer = 42", "app:///main.js")
            .expect("execute");
    }

    #[test]
    fn preloaded_entry_modules_finish_before_execute_module_returns() {
        let (mut runtime, engine) = engine();
        runtime
            .register_module_source(
                "app:///entry.js",
                "globalThis.answer = await Promise.resolve(42);",
            )
            .expect("register entry");
        engine
            .execute_module(
                &mut runtime,
                "await import('app:///entry.js');",
                "bobcat:boot",
            )
            .expect("execute boot module");
        engine
            .execute_script(
                &mut runtime,
                "if (globalThis.answer !== 42) throw new Error('entry was not awaited')",
                "verify.js",
            )
            .expect("entry completion must be visible");
    }

    #[test]
    fn synchronous_module_execution_still_rejects_a_pending_evaluation() {
        let (mut runtime, engine) = engine();
        let error = engine
            .execute_module(
                &mut runtime,
                "await new Promise(() => {});",
                "app:///pending-worker.js",
            )
            .expect_err("worker boot cannot finish while its module is pending");
        assert_eq!(error.kind, ScriptErrorKind::ModuleEvaluate);
    }

    #[test]
    fn a_missing_preloaded_module_is_a_named_module_error() {
        let (mut runtime, engine) = engine();
        let error = engine
            .execute_module(
                &mut runtime,
                "await import('app:///missing.mjs');",
                "bobcat:boot",
            )
            .expect_err("the loader must reject an unknown module");

        assert_eq!(error.phase, ScriptErrorPhase::ExecuteModule);
        assert!(error.message.contains("app:///missing.mjs"));
        assert!(error.message.contains("not preloaded"));
    }

    #[test]
    fn a_nul_in_a_module_completion_still_settles_the_import() {
        let (mut runtime, mut engine) = engine();
        engine.enable_module_loading();
        engine
            .execute_script(
                &mut runtime,
                "globalThis.outcomes = {};
                 for (const name of ['error', 'url']) {
                     import(`app:///${name}.js`).then(
                         () => { outcomes[name] = 'loaded'; },
                         error => { outcomes[name] = String(error); });
                 }",
                "app:///main.js",
            )
            .expect("the imports are asked for");
        let mut requests: Vec<String> =
            std::iter::from_fn(|| engine.take_module_request()).collect();
        requests.sort();
        assert_eq!(requests, ["app:///error.js", "app:///url.js"]);

        engine
            .complete_module(&mut runtime, "app:///error.js", Err("broken\0text"))
            .expect("an error text with a NUL completes the module");
        engine
            .complete_module(
                &mut runtime,
                "app:///url.js",
                Ok(("app:///redirected\0.js", "export default 1;")),
            )
            .expect("a response URL with a NUL completes the module");
        engine
            .execute_script(
                &mut runtime,
                "if (!outcomes.error.includes('broken\\uFFFDtext'))
                     throw new Error('the error text: ' + outcomes.error);
                 if (!outcomes.url.includes('the response URL contains a NUL byte'))
                     throw new Error('the response URL: ' + outcomes.url);",
                "app:///verify.js",
            )
            .expect("both imports rejected with what the completion said");
    }

    #[test]
    fn host_functions_are_native_module_exports_not_globals() {
        let (mut runtime, engine) = engine();
        engine
            .register_host_module_function("bobcat-internal:test", "answer", 0, |_| {
                Ok(quickjs::HostValue::Number(42.0))
            })
            .expect("register");
        engine
            .execute_module(&mut runtime,
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
        let (mut runtime, engine) = engine();
        runtime
            .register_module_source(
                "bobcat:callback",
                "export function receive(value) { globalThis.received = value; }",
            )
            .expect("register callback module");
        engine
            .execute_module(&mut runtime, "import 'bobcat:callback';", "bobcat:boot")
            .expect("load callback module");

        assert!(
            engine
                .call_named_export(
                    &mut runtime,
                    "bobcat:callback",
                    "receive",
                    &[quickjs::HostArgument::String("from Rust")],
                )
                .expect("call callback export")
        );
        assert!(
            !engine
                .call_named_export(&mut runtime, "bobcat:callback", "missing", &[])
                .expect("an absent export is not an engine failure")
        );
        engine
            .execute_script(
                &mut runtime,
                "if (globalThis.received !== 'from Rust') throw new Error('callback did not run')",
                "verify.js",
            )
            .expect("callback effect must be visible");
    }

    #[test]
    fn execute_runs_a_microtask_checkpoint() {
        let (mut runtime, engine) = engine();
        engine
            .execute_script(
                &mut runtime,
                "globalThis.answer = 0; Promise.resolve().then(() => answer = 42)",
                "app:///schedule.js",
            )
            .expect("execute and checkpoint");
        engine
            .execute_script(
                &mut runtime,
                "if (answer !== 42) throw new Error('checkpoint did not run')",
                "app:///verify.js",
            )
            .expect("microtask ran");
    }

    #[test]
    fn source_name_is_preserved_in_sanitized_errors() {
        let (mut runtime, engine) = engine();
        let error = engine
            .execute_script(&mut runtime, "const = 1", "app:///broken.js")
            .expect_err("syntax error");
        assert_eq!(error.kind, ScriptErrorKind::Syntax);
        assert_eq!(
            error.location.and_then(|location| location.source),
            Some(Arc::from("app:///broken.js"))
        );
    }

    #[test]
    fn a_thrown_error_keeps_its_constructor_name_and_stays_an_exception() {
        let (mut runtime, engine) = engine();

        let error = engine
            .execute_script(
                &mut runtime,
                "throw new TypeError('invalid receiver')",
                "throw.js",
            )
            .expect_err("a throw fails");
        assert_eq!(error.kind, ScriptErrorKind::Exception);
        assert_eq!(error.message.as_ref(), "TypeError: invalid receiver");

        let thrown = engine
            .execute_script(
                &mut runtime,
                "throw new SyntaxError('a runtime object')",
                "throw-syntax.js",
            )
            .expect_err("a throw fails");
        assert_eq!(thrown.kind, ScriptErrorKind::Exception);
    }

    #[test]
    fn two_realms_do_not_share_globals() {
        let (mut runtime, first) = engine();
        let second = runtime.create_realm().expect("a second realm");

        first
            .execute_script(&mut runtime, "globalThis.answer = 42", "first.js")
            .expect("execute");
        second
            .execute_script(
                &mut runtime,
                "if (typeof answer !== 'undefined') throw new Error('realms share a global')",
                "second.js",
            )
            .expect("the second realm has its own global object");
    }

    #[test]
    fn every_boundary_primitive_round_trips_through_a_host_function() {
        let (mut runtime, engine) = engine();
        let seen: Arc<Mutex<Vec<quickjs::HostValue>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        engine
            .register_host_module_function("bobcat-internal:test", "echo", 1, move |arguments| {
                let value = arguments
                    .first()
                    .map_or(quickjs::HostValue::Undefined, |value| value.to_value());
                recorder
                    .lock()
                    .expect("the recorder is not poisoned")
                    .push(value.clone());
                Ok(value)
            })
            .expect("register");

        engine
            .execute_module(
                &mut runtime,
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
        let (mut runtime, engine) = engine();
        let error = engine
            .execute_script(
                &mut runtime,
                "Promise.reject(new Error('microtask rejection'))",
                "app:///reject.js",
            )
            .expect_err("the checkpoint reports the rejection the script left behind");

        assert_eq!(error.kind, ScriptErrorKind::Exception);
        assert_eq!(error.phase, ScriptErrorPhase::Execute);
        assert_eq!(error.message.as_ref(), "Error: microtask rejection");
    }

    #[test]
    fn a_checkpoint_error_beside_a_primary_one_is_dropped() {
        let (mut runtime, engine) = engine();
        let primary = engine
            .execute_script(
                &mut runtime,
                "Promise.reject(new Error('beside')); throw new Error('primary')",
                "app:///both.js",
            )
            .expect_err("the script's own exception wins");
        assert_eq!(primary.message.as_ref(), "Error: primary");

        engine
            .execute_script(
                &mut runtime,
                "globalThis.reentered = true",
                "app:///after.js",
            )
            .expect("the next script is not handed the rejection beside the failure");
        engine
            .execute_script(
                &mut runtime,
                "if (reentered !== true) throw new Error('the next script never ran')",
                "app:///verify.js",
            )
            .expect("and it ran its own code");
    }

    #[test]
    fn a_realms_leftover_rejection_never_reaches_a_sibling() {
        let (mut runtime, first) = engine();
        let second = runtime.create_realm().expect("a second view's realm");
        runtime
            .register_module_source(
                "app:///first-entry.js",
                "Promise.reject(new Error('floating'));\nthrow new Error('boom');",
            )
            .expect("register the first view's entry");
        runtime
            .register_module_source(
                "app:///second-module.js",
                "export function ping() { globalThis.pinged = true; }",
            )
            .expect("register the second view's module");

        first
            .execute_module(
                &mut runtime,
                "await import('app:///first-entry.js');",
                "bobcat:boot-first",
            )
            .expect_err("the first view's entry fails, and is told so");

        second
            .execute_module(
                &mut runtime,
                "import 'app:///second-module.js';",
                "bobcat:boot-second",
            )
            .expect("the second view boots on a runtime the first one failed on");
        assert!(
            second
                .call_named_export(&mut runtime, "app:///second-module.js", "ping", &[])
                .expect("an event dispatch into the second view is not the first view's failure")
        );

        first
            .execute_script(&mut runtime, "globalThis.retry = true", "retry.js")
            .expect("the failing realm is not condemned to fail forever");
    }

    #[test]
    fn a_reported_failure_takes_its_own_leftovers_with_it() {
        let (mut runtime, engine) = engine();
        runtime
            .register_module_source(
                "app:///thrower.js",
                "Promise.reject(new Error('floating'));\nthrow new Error('boom');",
            )
            .expect("register");

        let reported = engine
            .execute_module(
                &mut runtime,
                "await import('app:///thrower.js');",
                "boot.js",
            )
            .expect_err("the entry fails");
        assert_eq!(reported.kind, ScriptErrorKind::Exception);

        engine
            .execute_script(&mut runtime, "globalThis.answer = 42", "after.js")
            .expect("the realm answers the next entry");
        engine
            .execute_script(
                &mut runtime,
                "if (answer !== 42) throw new Error('the next entry never ran')",
                "verify.js",
            )
            .expect("and ran its own code");
    }

    #[test]
    fn discarding_a_failures_leftovers_does_not_discard_its_jobs() {
        let (mut runtime, engine) = engine();
        engine
            .execute_script(
                &mut runtime,
                "globalThis.order = [];
                 Promise.resolve().then(() => order.push('queued-1'));
                 Promise.resolve().then(() => order.push('queued-2'));
                 throw new Error('primary')",
                "thrower.js",
            )
            .expect_err("the script throws");

        engine
            .execute_script(
                &mut runtime,
                "order.push('next');
                 if (order.join(',') !== 'queued-1,queued-2,next') {
                     throw new Error('a queued job was dropped: ' + order.join(','));
                 }",
                "reentry.js",
            )
            .expect("every job the failing script queued still ran, and ran first");
    }

    #[test]
    fn a_checkpoint_drains_every_queued_job_in_one_entry() {
        let (mut runtime, engine) = engine();
        engine
            .execute_script(
                &mut runtime,
                "globalThis.count = 0;
                 for (let i = 0; i < 5000; i++) {
                     Promise.resolve().then(() => { count++; });
                 }
                 globalThis.cascade = 0;
                 const chain = (left) => {
                     cascade++;
                     if (left > 0) Promise.resolve().then(() => chain(left - 1));
                 };
                 Promise.resolve().then(() => chain(2000));",
                "flood.js",
            )
            .expect("a job queue far past the old 1024-job budget is not an error");
        assert!(
            !runtime.runtime.has_pending_jobs(),
            "the checkpoint must leave the runtime's job queue empty"
        );

        engine
            .execute_script(
                &mut runtime,
                "if (count !== 5000) throw new Error('jobs left over: ' + count);
                 if (cascade !== 2001) throw new Error('cascade left over: ' + cascade);",
                "verify.js",
            )
            .expect("the very next entry sees every job already done");
    }

    fn install_a_panicking_host_function(engine: &mut ScriptEngine) {
        engine
            .register_host_module_function("bobcat-internal:test", "explode", 0, |_| {
                panic!("a host bug")
            })
            .expect("register");
    }

    #[test]
    fn a_host_function_that_panics_fails_its_entry_as_that_panic_even_when_caught() {
        let (mut runtime, mut engine) = engine();
        install_a_panicking_host_function(&mut engine);
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            engine.execute_module(
                &mut runtime,
                "import { explode } from 'bobcat-internal:test';
                 try { explode(); } catch (error) { globalThis.seen = error.message; }",
                "app:///explode.js",
            )
        }))
        .expect_err("the entry unwinds with the host function's panic");
        assert_eq!(
            crate::threads::panic_message(unwound.as_ref()),
            "a host bug"
        );

        engine
            .execute_script(
                &mut runtime,
                "if (seen !== 'the host function panicked') throw new Error(String(seen))",
                "app:///verify.js",
            )
            .expect("the script saw the bridge's exception, and nothing is resumed twice");
    }

    #[test]
    fn a_host_panic_a_siblings_checkpoint_ran_into_is_resumed_by_its_own_realm() {
        let (mut runtime, mut first) = engine();
        let second = runtime.create_realm().expect("a second view's realm");
        install_a_panicking_host_function(&mut first);
        runtime
            .register_module_source(
                "app:///arm.js",
                "import { explode } from 'bobcat-internal:test';
                 export function arm() { Promise.resolve().then(() => explode()); }",
            )
            .expect("register the first view's module");
        first
            .execute_module(&mut runtime, "import 'app:///arm.js';", "bobcat:boot-first")
            .expect("load the first view's module");
        assert!(
            first
                .call_named_export_before_operation(&mut runtime, "app:///arm.js", "arm", &[])
                .expect("arm the job")
        );

        second
            .execute_script(&mut runtime, "globalThis.fine = true", "app:///second.js")
            .expect("the sibling that ran the job is not the realm that panicked");
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            first.execute_script(&mut runtime, "void 0", "app:///first.js")
        }))
        .expect_err("the realm whose host function panicked resumes it at its next checkpoint");
        assert_eq!(
            crate::threads::panic_message(unwound.as_ref()),
            "a host bug"
        );
    }

    #[test]
    fn an_execution_timeout_is_reported_and_leaves_the_engine_usable() {
        let (error, reusable) = with_watchdog(|| {
            let (mut runtime, engine) = engine_with(
                QuickJsConfig::default().with_execution_timeout(Some(Duration::from_millis(20))),
            );
            let error = engine
                .execute_script(&mut runtime, "for (;;) {}", "app:///spin.js")
                .expect_err("an endless script must be interrupted");
            let reusable = engine
                .execute_script(&mut runtime, "globalThis.answer = 6 * 7", "app:///after.js")
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
