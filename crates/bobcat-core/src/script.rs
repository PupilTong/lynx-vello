//! Host-injected JavaScript realm contracts.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use thiserror::Error;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ScriptValue<C, S> {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    BigInt(Arc<str>),
    String(Arc<str>),
    Symbol(S),
    Callable(C),
}

/// An isolated JavaScript engine supplied to Bobcat.
pub trait ScriptEngine {
    type Callable: fmt::Debug;

    type Symbol: fmt::Debug;

    type ImportFuture<'a>: Future<Output = Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError>>
        + 'a
    where
        Self: 'a;

    fn evaluate(
        &mut self,
        source_text: &str,
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError>;

    fn import_value<'a>(
        &'a mut self,
        specifier: &'a str,
        export_name: &'a str,
    ) -> Self::ImportFuture<'a>;

    fn call(
        &mut self,
        callable: &Self::Callable,
        this_value: &ScriptValue<Self::Callable, Self::Symbol>,
        arguments: &[ScriptValue<Self::Callable, Self::Symbol>],
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError>;
}

/// Sanitized script failure details that are safe to expose outside a realm.
#[derive(Clone, Debug, Error)]
#[error("{kind:?} during {phase:?}: {message}")]
pub struct ScriptError {
    pub kind: ScriptErrorKind,
    pub phase: ScriptErrorPhase,
    pub message: Arc<str>,
    pub location: Option<ScriptSourceLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorKind {
    EvaluationDenied,
    Syntax,
    Exception,
    ModuleResolve,
    ModuleLoad,
    ModuleParse,
    ModuleLink,
    ModuleEvaluate,
    MissingExport,
    NonTransferableValue,
    InvalidBoundaryValue,
    WrongEngine,
    StaleHandle,
    Terminated,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorPhase {
    Evaluate,
    ImportValue,
    Call,
}

/// Sanitized source location for a [`ScriptError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSourceLocation {
    pub source: Option<Arc<str>>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
