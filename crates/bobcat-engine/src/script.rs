//! Host-injected JavaScript realm contracts.
//!
//! [`ScriptEngine`] is inspired by the TC39
//! [ShadowRealm proposal](https://tc39.es/proposal-shadowrealm/): one engine
//! value represents one realm with its own global object, intrinsics and
//! module graph. Evaluation and wrapped-function calls are synchronous;
//! importing one exported module binding is asynchronous.
//!
//! The boundary deliberately cannot represent ordinary JavaScript objects.
//! Only ECMAScript primitive values and opaque, engine-owned symbol/callable
//! handles may cross it. That prevents object identity from leaking between
//! realm instances. This is an isolation boundary, not an availability or
//! security sandbox: an implementation may still share a VM, heap and owner
//! thread between realms.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use thiserror::Error;

/// An asynchronous operation on one script-engine instance.
///
/// The future is intentionally not required to be `Send`: JavaScript realm
/// handles are commonly owner-thread-bound. Tokio applications can drive such
/// operations on a [`tokio::task::LocalSet`].
pub type ScriptFuture<'a, C, S> =
    Pin<Box<dyn Future<Output = Result<ScriptValue<C, S>, ScriptError>> + 'a>>;

/// A value allowed to cross a ShadowRealm-like engine boundary.
///
/// `C` and `S` are opaque, rooted callable and Symbol handles owned by one
/// [`ScriptEngine`] instance. An engine must reject a handle created by another
/// instance with [`ScriptErrorKind::WrongEngine`]. Handles must never expose a
/// raw engine pointer or an ordinary JavaScript object to the host.
/// Implementations must validate host-constructed values before entering the
/// realm, including the canonical representation of [`ScriptValue::BigInt`],
/// and reject malformed input with [`ScriptErrorKind::InvalidBoundaryValue`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ScriptValue<C, S> {
    /// ECMAScript `undefined`.
    Undefined,
    /// ECMAScript `null`.
    Null,
    /// An ECMAScript Boolean.
    Boolean(bool),
    /// An ECMAScript Number, including `NaN`, infinities and negative zero.
    Number(f64),
    /// A canonical signed decimal ECMAScript `BigInt` without the `n` suffix.
    BigInt(Arc<str>),
    /// An ECMAScript String.
    String(Arc<str>),
    /// An engine-owned ECMAScript Symbol handle.
    Symbol(S),
    /// An engine-owned wrapped callable handle.
    ///
    /// A wrapped callable is callable but is not a constructor. Its `this`
    /// value, arguments and return value are all subject to this same boundary.
    Callable(C),
}

/// One isolated JavaScript realm/engine instance.
///
/// Implementations must create a distinct global object, intrinsic set and
/// module graph for every value implementing this trait. The trait does not
/// require `Clone`, `Send` or `Sync`: [`crate::view::LynxView`] consumes the
/// instance and owns it for the view's lifetime.
///
/// Runtime exceptions crossing the boundary must not expose the original
/// thrown object or stack object. They are reported as a sanitized
/// [`ScriptError`], matching `ShadowRealm`'s fresh-error boundary semantics.
pub trait ScriptEngine {
    /// Opaque rooted handle for engine-owned wrapped callables.
    ///
    /// A handle is scoped to exactly one engine instance. The implementation
    /// is responsible for keeping its target alive until the handle is
    /// released, or for returning a stable [`ScriptErrorKind::StaleHandle`]
    /// error after invalidation.
    type Callable: fmt::Debug;

    /// Opaque rooted handle for engine-owned Symbols.
    ///
    /// As with [`Self::Callable`], a Symbol handle is scoped to exactly one
    /// engine instance and must remain valid or fail with a stable stale-handle
    /// error.
    type Symbol: fmt::Debug;

    /// Synchronously evaluate `source_text` as a top-level Script.
    ///
    /// Evaluation happens in this instance's isolated global environment. A
    /// normal result that is an ordinary object, array, Promise or other
    /// non-callable object must fail with
    /// [`ScriptErrorKind::NonTransferableValue`].
    fn evaluate(
        &mut self,
        source_text: &str,
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError>;

    /// Import one named module export into this instance.
    ///
    /// This is the asynchronous counterpart of `ShadowRealm` `importValue`, not
    /// a general module-namespace escape hatch. The selected binding must be a
    /// primitive or callable boundary value.
    fn import_value<'a>(
        &'a mut self,
        specifier: &'a str,
        export_name: &'a str,
    ) -> ScriptFuture<'a, Self::Callable, Self::Symbol>;

    /// Synchronously invoke a wrapped callable owned by this instance.
    ///
    /// `this_value`, every argument and the returned value obey the same
    /// primitive-or-callable restriction. The implementation must return
    /// [`ScriptErrorKind::WrongEngine`] for a handle from another instance.
    /// Construction is deliberately not exposed.
    fn call(
        &mut self,
        callable: &Self::Callable,
        this_value: &ScriptValue<Self::Callable, Self::Symbol>,
        arguments: &[ScriptValue<Self::Callable, Self::Symbol>],
    ) -> Result<ScriptValue<Self::Callable, Self::Symbol>, ScriptError>;
}

/// Sanitized script failure details that are safe to expose outside a realm.
///
/// This type must never contain a raw engine-local exception or object handle.
#[derive(Clone, Debug, Error)]
#[error("{kind:?} during {phase:?}: {message}")]
pub struct ScriptError {
    /// Stable failure category.
    pub kind: ScriptErrorKind,
    /// Public operation that failed.
    pub phase: ScriptErrorPhase,
    /// Sanitized diagnostic text; callers must not branch on it.
    pub message: Arc<str>,
    /// Optional source location with no engine-local object identity.
    pub location: Option<ScriptSourceLocation>,
}

/// Stable script-engine failure categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorKind {
    /// Host policy rejected dynamic source compilation.
    EvaluationDenied,
    /// Source text could not be parsed as a Script.
    Syntax,
    /// Evaluated code or a wrapped callable threw.
    Exception,
    /// The module specifier could not be resolved.
    ModuleResolve,
    /// Module source could not be loaded.
    ModuleLoad,
    /// Module source could not be parsed.
    ModuleParse,
    /// The module graph could not be linked.
    ModuleLink,
    /// Module evaluation failed.
    ModuleEvaluate,
    /// The requested named export does not exist.
    MissingExport,
    /// An ordinary object attempted to cross the realm boundary.
    NonTransferableValue,
    /// A host-created boundary value is malformed for its declared primitive.
    InvalidBoundaryValue,
    /// A handle belongs to another engine instance.
    WrongEngine,
    /// A previously valid engine-owned handle is no longer alive.
    StaleHandle,
    /// The engine instance was terminated or closed.
    Terminated,
    /// An implementation-specific failure with no more stable category.
    Other,
}

/// Script operation during which an error occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScriptErrorPhase {
    /// [`ScriptEngine::evaluate`].
    Evaluate,
    /// [`ScriptEngine::import_value`].
    ImportValue,
    /// [`ScriptEngine::call`].
    Call,
}

/// Sanitized source location for a [`ScriptError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptSourceLocation {
    /// Module specifier or implementation-provided source label.
    pub source: Option<Arc<str>>,
    /// One-based line number, when available.
    pub line: Option<u32>,
    /// One-based column number, when available.
    pub column: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::ScriptEngine;

    fn accepts_object_safe_trait(_: Option<&mut dyn ScriptEngine<Callable = (), Symbol = ()>>) {}

    #[test]
    fn script_engine_is_object_safe() {
        accepts_object_safe_trait(None);
    }
}
