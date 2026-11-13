//! The errors of the bytecode model, reader, writer and verifier.

use std::fmt;

/// A value the model cannot hold: an operand that does not match its
/// opcode's format, or an opcode outside the final bytecode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    /// What was wrong.
    pub message: String,
}

impl ModelError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

/// Bytes [`super::read_program`] refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    /// Byte offset in the input where the problem was found.
    pub offset: usize,
    /// What was wrong.
    pub message: String,
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ReadError {}

/// A program [`super::write_program`] cannot write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    /// What was wrong.
    pub message: String,
}

impl WriteError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WriteError {}

/// The rule a program broke. Each variant is one rule of
/// [`super::Verifier`]; the module documentation lists what each protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rule {
    // Values and placement.
    /// A function value anywhere but the top level, a module's function or
    /// a constant-pool entry; a module anywhere but the top level.
    ValuePlacement,
    /// A file atom index outside the table, a built-in index past
    /// `JS_ATOM_END`, or an integer atom above `JS_ATOM_MAX_INT`.
    AtomRange,
    /// `JS_ATOM_NULL` as an instruction's atom where the instruction turns
    /// it into a property key or a value.
    NullAtom,
    /// `private_symbol` with an integer atom (`JS_NewSymbolFromAtom`
    /// asserts it is not one).
    PrivateSymbolAtom,

    // Records.
    /// Vardefs present with a count other than `arg_count + var_count`.
    VardefCount,
    /// `defined_arg_count > arg_count`.
    DefinedArgCount,
    /// A `js_mode` bit other than strict and backtrace barrier.
    JsMode,
    /// A `var_kind` outside `JSVarKindEnum`.
    VarKind,
    /// A captured variable's `var_ref_idx` not below `var_ref_count`, two
    /// captured variables with one index, or `var_ref_count` other than
    /// the number of captured variables.
    VarRefIndex,
    /// A closure type the function's position does not allow: a script's
    /// top-level function holds only `GLOBAL`/`GLOBAL_DECL`, a module's
    /// function only those and `MODULE_DECL`/`MODULE_IMPORT`, a nested
    /// function only `LOCAL`/`ARG`/`REF`/`GLOBAL_REF`.
    ClosureType,
    /// A closure variable that does not name an existing, captured,
    /// compatible variable of its parent.
    ClosureParent,
    /// A module record entry that names a missing request or a closure
    /// variable of the wrong type, an import variable no import fills, or
    /// attributes that are not `undefined` or an object.
    ModuleRecord,
    /// A module function that is not an async, strict function.
    ModuleFunction,
    /// A class constructor (`define_class`) that is not a normal function.
    ClassConstructor,

    // Instructions.
    /// A local, argument, closure-variable or constant-pool operand outside
    /// its count, or a jump target outside the function.
    OperandRange,
    /// An enumerated operand outside the values the interpreter handles.
    EnumOperand,
    /// `fclosure` of a constant that is not function bytecode.
    ClosureConstant,
    /// A `regexp` that does not directly follow two `push_const` of strings
    /// with no jump target on the second or on itself, or whose bytecode is
    /// not what the engine compiles for its pattern and flags.
    RegExp,
    /// An instruction that creates a variable reference into its frame
    /// (`make_loc_ref`, `make_arg_ref`, `close_loc`, a closure capturing a
    /// local or argument, a mapped `arguments` object, direct `eval`) on a
    /// variable that is not captured with a valid `var_ref_idx`, or in a
    /// function without vardefs.
    VarRefCapture,
    /// A read of a variable that can hold the uninitialized marker by an
    /// instruction that does not test for it.
    Uninitialized,
    /// A direct `eval` whose scope index or scope chain is not one the
    /// eval compiler can walk.
    EvalScope,
    /// `special_object` that exposes the module function to JavaScript.
    ModuleThisFunc,
    /// A global binding reached other than through `get_var`, `put_var`
    /// and the store of a declared function.
    GlobalBinding,

    // Control flow and stack slots.
    /// A pop or an addressed slot below the bottom of the stack, or a depth
    /// above `JS_STACK_SIZE_MAX`.
    StackDepth,
    /// Two paths reach one instruction with different depths or different
    /// special slots.
    Merge,
    /// The fork's own stack walk would compute a smaller depth than the
    /// interpreter reaches, or would refuse where this walk does not.
    ForkWalk,
    /// An instruction pops, copies, moves or overwrites a special slot
    /// (return address, catch offset, iterator marker, function bytecode)
    /// in a way the interpreter does not allow.
    SpecialSlot,
    /// An iterator instruction whose iterator triple is not where the
    /// interpreter addresses it.
    IteratorSlots,
    /// `nip_catch` with no catch offset or iterator marker below the top.
    NipCatch,
    /// `append` to anything but an array `array_from` made that no other
    /// slot, variable or object holds.
    AppendTarget,
    /// `ret` without a return address on top, or returning with a stack
    /// that differs from the one at the `gosub`.
    Ret,
    /// A `catch`, `gosub` or `with_*` whose target is not after it. The
    /// interpreter polls its interrupt handler only on `goto`, `if_*` and
    /// calls, so a loop closed by one of these jumps (or by an exception
    /// delivered to an earlier handler) would never reach the execution
    /// limit. It also keeps a handler off instruction 0, where its catch
    /// offset would read as the iterator marker.
    BackwardTarget,
    /// An instruction that suspends or returns in a frame of the wrong
    /// kind: generator instructions outside generators, `return` in a
    /// generator or async frame, `initial_yield` not exactly once before
    /// any suspension.
    FrameKind,
    /// Control reaches the end of the function.
    FallsOffEnd,
}

/// A program [`super::Verifier`] refused: which function, which instruction
/// and which rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// The function: its path from the top level (`root`, then
    /// `.cpool[i]` per nesting level, `module` for a module's function)
    /// and its name.
    pub function: String,
    /// The instruction index in that function, when the rule is about one.
    pub instruction: Option<u32>,
    /// The rule.
    pub rule: Rule,
    /// What was found.
    pub detail: String,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.function)?;
        if let Some(index) = self.instruction {
            write!(formatter, " instruction {index}")?;
        }
        write!(formatter, ": {:?}: {}", self.rule, self.detail)
    }
}

impl std::error::Error for VerifyError {}

/// Why untrusted bytes were refused before the engine read them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// The bytes do not read as a program.
    Read(ReadError),
    /// The program breaks a rule.
    Verify(VerifyError),
    /// The program cannot be written back.
    Write(WriteError),
    /// The top-level value is not the kind the call needs.
    Root(&'static str),
}

impl fmt::Display for CheckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "bytecode does not read: {error}"),
            Self::Verify(error) => write!(formatter, "bytecode refused: {error}"),
            Self::Write(error) => write!(formatter, "bytecode cannot be written: {error}"),
            Self::Root(expected) => write!(formatter, "bytecode is not {expected}"),
        }
    }
}

impl std::error::Error for CheckError {}

/// Why a regular expression has no bytecode: what
/// [`super::Verifier::compile_regexp`] answers instead of bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegExpError {
    /// The engine refuses the pattern or the flags.
    Refused,
    /// There is no engine to compile with: creating its runtime or realm
    /// failed, for the reason given.
    NoEngine(String),
}

impl fmt::Display for RegExpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused => formatter.write_str("the engine refuses the pattern and flags"),
            Self::NoEngine(reason) => write!(
                formatter,
                "no engine to compile regular expressions with: {reason}"
            ),
        }
    }
}

impl std::error::Error for RegExpError {}
