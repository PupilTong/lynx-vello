//! Host-injected JavaScript virtual-machine contracts.
//!
//! The embedder supplies a [`ScriptEngineFactory`], while Bobcat owns every
//! script executed in the resulting VM and every host function installed in
//! it.  The VM is intentionally created by the factory on the caller's
//! thread: factories are transferable, VM instances need not be.

use std::fmt;
use std::sync::Arc;

/// A value allowed across a Bobcat host-function boundary.
///
/// Objects, functions, symbols and VM handles deliberately have no
/// representation here. This keeps an injected VM from exposing its realm to
/// the runtime and keeps DOM identity private to Bobcat's callbacks.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum HostValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(Arc<str>),
}

/// A leaf host callback installed by Bobcat in an injected VM.
///
/// The VM must turn a returned message into a JavaScript exception and must
/// not invoke the callback re-entrantly. On unwind-capable targets it should
/// also translate a caught callback panic; an abort-only platform treats an
/// unexpected panic as a fatal VM-owner failure instead.
pub type HostCallback = Box<dyn FnMut(&[HostValue]) -> Result<HostValue, String> + 'static>;

/// One owner-thread-bound JavaScript VM supplied to Bobcat.
///
/// `register_host_function` creates the namespace when it does not exist and
/// adds/replaces the named member; registration must retain the callback but
/// must not invoke it. `register_module_source` adds one source to the VM's
/// synchronous preloaded module graph. `execute_module` compiles, links, and
/// evaluates an entry module, including its top-level-await promise, before
/// returning. A VM with an explicitly owned job queue may drain its checkpoint
/// at either execution boundary. The VM instance itself is deliberately not
/// required to be `Send`.
pub trait ScriptEngine: fmt::Debug {
    fn register_host_function(
        &mut self,
        namespace: &str,
        name: &str,
        arity: u8,
        callback: HostCallback,
    ) -> Result<(), ScriptError>;

    /// Evaluates auxiliary classic source. Bobcat's startup path does not call
    /// this operation; entries and built-ins use the ESM operations below.
    fn execute_script(&mut self, source: &str, source_name: &str) -> Result<(), ScriptError>;

    /// Registers one exact, normalized module name and its UTF-8 source.
    ///
    /// Bobcat's ESM runtime currently requires the built-in `QuickJS` adapter;
    /// the default keeps older injected test VMs source-compatible while
    /// rejecting module startup precisely.
    fn register_module_source(
        &mut self,
        _specifier: &str,
        _source: &str,
    ) -> Result<(), ScriptError> {
        Err(module_support_required(ScriptErrorPhase::RegisterModule))
    }

    /// Evaluates one ESM entry and waits for its evaluation promise to settle.
    fn execute_module(&mut self, _source: &str, _source_name: &str) -> Result<(), ScriptError> {
        Err(module_support_required(ScriptErrorPhase::ExecuteModule))
    }

    /// Calls a function the realm published on the host namespace, if it
    /// published one.
    ///
    /// `Ok(false)` means the realm installed no such member — a bundle with no
    /// listener runtime, not a failure. `Ok(true)` means it ran and returned.
    ///
    /// The arguments are [`HostValue`]s for the same reason a host callback's
    /// are: element identity crosses as a number and nothing else crosses at
    /// all. There is no return value, because the one thing a callee needs to
    /// tell the host — that a walk should end — is a host call of its own, and
    /// making it a return value would mean the host could only hear it once
    /// the callee finished.
    fn call_host_member(
        &mut self,
        namespace: &str,
        name: &str,
        arguments: &[HostValue],
    ) -> Result<bool, ScriptError>;

    fn collect_garbage(&mut self) -> Result<(), ScriptError>;
}

/// Transferable capability for constructing a VM on its eventual owner
/// thread.
pub trait ScriptEngineFactory: fmt::Debug + Send + Sync {
    fn create(&self) -> Result<Box<dyn ScriptEngine>, ScriptError>;
}

/// Sanitized script failure details that are safe to expose outside a realm.
#[derive(Clone, Debug)]
pub struct ScriptError {
    pub kind: ScriptErrorKind,
    pub phase: ScriptErrorPhase,
    pub message: Arc<str>,
    pub location: Option<ScriptSourceLocation>,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} during {:?}: {}",
            self.kind, self.phase, self.message
        )?;
        if let Some(location) = &self.location {
            let source = location.source.as_deref().unwrap_or("<unknown>");
            match (location.line, location.column) {
                (Some(line), Some(column)) => write!(formatter, " (at {source}:{line}:{column})")?,
                (Some(line), None) => write!(formatter, " (at {source}:{line})")?,
                _ => write!(formatter, " (at {source})")?,
            }
        }
        Ok(())
    }
}

impl std::error::Error for ScriptError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorKind {
    EvaluationDenied,
    ModuleLoad,
    ModuleEvaluate,
    Syntax,
    Exception,
    InvalidBoundaryValue,
    Terminated,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorPhase {
    Initialize,
    RegisterHostFunction,
    RegisterModule,
    Execute,
    ExecuteModule,
    /// Calling a member the realm published back to the host.
    CallHostMember,
    CollectGarbage,
}

fn module_support_required(phase: ScriptErrorPhase) -> ScriptError {
    ScriptError {
        kind: match phase {
            ScriptErrorPhase::RegisterModule => ScriptErrorKind::ModuleLoad,
            _ => ScriptErrorKind::ModuleEvaluate,
        },
        phase,
        message: Arc::from("the script engine does not support Bobcat's ESM module graph"),
        location: None,
    }
}

/// Sanitized source location for a [`ScriptError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSourceLocation {
    pub source: Option<Arc<str>>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
