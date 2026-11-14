//! [`Verifier`], the one public way to check a program against every rule
//! of the module documentation, and [`VerifiedModule`], the only form in
//! which module bytecode from a writer outside the engine reaches a realm.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::error::{CheckError, RegExpError};
use super::model::{Program, Value};
use super::read::read_program;
use super::verify::{RegExpCompiler, verify};
use super::write::write_program;

/// The bytes of a module that passed every rule of the module
/// documentation.
///
/// The invariant: the bytes are exactly what [`super::write_program`]
/// wrote for a program whose root is a module and which a [`Verifier`]
/// accepted, and every regular expression literal's bytecode in them is
/// what this process's engine compiles for the literal's pattern and flags.
/// Nothing else makes one: there is no constructor from bytes or from a
/// program except [`Verifier::module`] and [`Verifier::module_from_bytes`].
///
/// [`crate::Context::complete_module_bytecode`] and
/// [`crate::RequiredBody::ModuleBytecode`] take one and run no Rust check
/// of their own. The engine reads the bytes when the module is compiled
/// with `JS_READ_OBJ_COMPUTE_STACK_SIZE`, which computes every function's
/// stack size and trusts the structure of the bytes: the invariant above
/// is what makes that trust hold.
///
/// It is immutable, `Send + Sync`, and a clone shares the bytes, so one
/// value completes the module in any number of realms on any threads.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VerifiedModule {
    bytes: Arc<[u8]>,
}

impl VerifiedModule {
    /// The module's bytes, for reading (a disassembly, a comparison).
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Bytes as a module without any check, for this crate's tests of what
    /// the fork's read refuses when a realm reads them. The fork's reader
    /// trusts the structure of its input, so the bytes must be the fork's
    /// own, or such bytes with a change that keeps their structure (an
    /// instruction replaced by one of the same size, say).
    #[cfg(test)]
    pub(crate) fn unverified(bytes: &[u8]) -> Self {
        Self {
            bytes: Arc::from(bytes),
        }
    }
}

impl fmt::Debug for VerifiedModule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedModule")
            .field("length", &self.bytes.len())
            .finish()
    }
}

/// The compilations a [`Verifier`] keeps: by pattern (UTF-16 code units)
/// and flags, the engine's bytecode, or `None` for a refusal.
type Compiled = HashMap<(Vec<u16>, String), Option<Vec<u8>>>;

/// The regular expression engine: a runtime and one realm on it.
struct Engine {
    // Dropped before the runtime: fields drop in declaration order.
    context: crate::Context,
    _runtime: crate::Runtime,
}

impl Engine {
    fn new() -> Result<Self, String> {
        let runtime = crate::Runtime::new().map_err(|error| error.to_string())?;
        let context = runtime
            .create_bare_context()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            context,
            _runtime: runtime,
        })
    }
}

/// Checks programs against every rule of the module documentation and
/// writes the ones it accepts as [`VerifiedModule`]s.
///
/// It owns the regular expression compiler the `regexp` rule compares
/// literals with: a runtime and a realm with only the engine's basic
/// objects, made on the first literal it compiles, so a verifier that sees
/// no literal makes none. Every compilation is kept, by pattern (UTF-16
/// code units) and flags, for the verifier's lifetime: a converter that
/// asks [`Self::compile_regexp`] for a literal's bytecode and the rule that
/// then compares that literal cost one compilation, and a literal repeated
/// across functions or programs compiles once.
///
/// It is bound to the thread that made it, as a [`crate::Runtime`] is; the
/// modules it writes are not.
pub struct Verifier {
    engine: OnceCell<Result<Engine, String>>,
    compiled: RefCell<Compiled>,
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Verifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Verifier")
            .field("engine", &self.engine.get().map(Result::is_ok))
            .field("compiled", &self.compiled.borrow().len())
            .finish()
    }
}

impl Verifier {
    /// A verifier with no engine yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            engine: OnceCell::new(),
            compiled: RefCell::new(HashMap::new()),
        }
    }

    /// A verifier whose engine could not be created, for the tests of that
    /// refusal.
    #[cfg(test)]
    pub(crate) fn without_engine(reason: &str) -> Self {
        let verifier = Self::new();
        let _ = verifier.engine.set(Err(reason.to_owned()));
        verifier
    }

    /// Whether the engine has been created (or its creation tried).
    #[cfg(test)]
    pub(crate) fn has_engine(&self) -> bool {
        self.engine.get().is_some()
    }

    /// The regular expression bytecode the engine compiles for `pattern`
    /// (UTF-16 code units, as a JavaScript string holds them) and `flags`
    /// (the flag letters), as [`crate::Context::compile_regexp_bytecode`]
    /// answers it, or why there is none. The answer is kept: the same
    /// pattern and flags are not compiled again by this verifier.
    ///
    /// # Errors
    ///
    /// [`RegExpError::Refused`] when the engine refuses the pattern or the
    /// flags; [`RegExpError::NoEngine`] when the engine could not be
    /// created.
    pub fn compile_regexp(&self, pattern: &[u16], flags: &str) -> Result<Vec<u8>, RegExpError> {
        let key = (pattern.to_vec(), flags.to_owned());
        if let Some(known) = self.compiled.borrow().get(&key) {
            return known.clone().ok_or(RegExpError::Refused);
        }
        let engine = self
            .engine
            .get_or_init(Engine::new)
            .as_ref()
            .map_err(|reason| RegExpError::NoEngine(reason.clone()))?;
        let compiled = engine.context.compile_regexp_bytecode(pattern, flags).ok();
        self.compiled.borrow_mut().insert(key, compiled.clone());
        compiled.ok_or(RegExpError::Refused)
    }

    /// How many distinct patterns and flags this verifier has compiled:
    /// each one once, however often it was asked for.
    #[must_use]
    pub fn compiled_regexps(&self) -> usize {
        self.compiled.borrow().len()
    }

    /// Checks `program`, whose root must be a module, against every rule,
    /// and writes it.
    ///
    /// # Errors
    ///
    /// [`CheckError::Root`] when the root is not a module,
    /// [`CheckError::Verify`] for the first rule it breaks (a regular
    /// expression literal when the engine cannot be created: the rule's
    /// detail says so), [`CheckError::Write`] when it cannot be written.
    pub fn module(&self, program: &Program) -> Result<VerifiedModule, CheckError> {
        if !matches!(program.root, Value::Module(_)) {
            return Err(CheckError::Root("a module"));
        }
        verify(program, self).map_err(CheckError::Verify)?;
        let bytes = write_program(program).map_err(CheckError::Write)?;
        Ok(VerifiedModule {
            bytes: Arc::from(bytes),
        })
    }

    /// Reads `bytes` from a writer this process need not trust, then as
    /// [`Self::module`]: the answer is the program that was checked,
    /// written again.
    ///
    /// # Errors
    ///
    /// [`CheckError::Read`] when the bytes do not read as a program, then
    /// as [`Self::module`].
    pub fn module_from_bytes(&self, bytes: &[u8]) -> Result<VerifiedModule, CheckError> {
        let program = read_program(bytes).map_err(CheckError::Read)?;
        self.module(&program)
    }
}

impl RegExpCompiler for Verifier {
    fn compile_regexp(&self, pattern: &[u16], flags: &str) -> Result<Vec<u8>, RegExpError> {
        Self::compile_regexp(self, pattern, flags)
    }
}
