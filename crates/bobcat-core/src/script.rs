//! The script-failure vocabulary the engine reports to its embedder.
//!
//! Bobcat owns its script engine outright: it creates the realm, owns every
//! script executed in it, and installs every host function it exposes. None of
//! that crosses the public boundary. What does cross is failure — a bundle
//! that will not parse, a listener that throws — and these are the sanitized
//! details carried by [`crate::EngineEvent`].

use std::fmt;
use std::sync::Arc;

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
    ModuleLoad,
    ModuleEvaluate,
    Syntax,
    Exception,
    InvalidBoundaryValue,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorPhase {
    Initialize,
    RegisterHostModuleFunction,
    RegisterModule,
    Execute,
    ExecuteModule,
    /// Calling an ESM export the realm published back to the host.
    CallModuleExport,
    CollectGarbage,
}

/// Sanitized source location for a [`ScriptError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSourceLocation {
    pub source: Option<Arc<str>>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
