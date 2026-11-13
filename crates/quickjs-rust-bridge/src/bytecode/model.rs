//! The program model: what [`super::read_program`] returns, what
//! [`super::write_program`] writes and what [`super::Verifier`] checks.

use std::borrow::Cow;

use super::error::ModelError;
use crate::bytecode_abi::{
    ATOMS, AtomKind, JS_ATOM_END, OPCODES, Opcode as OpcodeInfo, OperandFormat, atom_int,
    builtin_string_atom, closure_type, func_kind, op,
};

/// A serialized program: the file's atom table and the top-level value.
///
/// The top-level value of compiled code is a [`Value::Function`] (a global
/// script) or a [`Value::Module`]. Any other [`Value`] is a plain data
/// value, which the format also carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// Atoms that are neither built in nor integers. [`Atom::File`] indexes
    /// this vector. The writer renumbers the entries in the order of first
    /// use, as the fork's writer numbers them, and drops unused ones.
    pub atoms: Vec<JsString>,
    /// The top-level value.
    pub root: Value,
}

impl Program {
    /// A program with an empty atom table.
    #[must_use]
    pub fn new(root: Value) -> Self {
        Self {
            atoms: Vec::new(),
            root,
        }
    }

    /// The atom the runtime interns `text` to, adding it to
    /// [`Self::atoms`] when it is neither built in nor an integer: a
    /// canonical decimal integer up to `JS_ATOM_MAX_INT` is [`Atom::Int`],
    /// the text of a built-in string atom is [`Atom::Builtin`], anything
    /// else is [`Atom::File`] (an existing entry with the same text is
    /// reused).
    pub fn intern(&mut self, text: JsString) -> Atom {
        if let Some(atom) = runtime_atom_of(&text) {
            return atom;
        }
        if let Some(index) = self.atoms.iter().position(|entry| entry.same_text(&text)) {
            return Atom::File(u32::try_from(index).unwrap_or(u32::MAX));
        }
        self.atoms.push(text);
        Atom::File(u32::try_from(self.atoms.len() - 1).unwrap_or(u32::MAX))
    }

    /// [`Self::intern`] for Rust text.
    pub fn intern_str(&mut self, text: &str) -> Atom {
        self.intern(JsString::from_str(text))
    }

    /// The text of `atom`; `None` for a file index outside the table and
    /// for `JS_ATOM_NULL`.
    #[must_use]
    pub fn atom_text(&self, atom: Atom) -> Option<Cow<'_, str>> {
        match atom {
            Atom::Builtin(0) => None,
            Atom::Builtin(index) => ATOMS
                .get(usize::try_from(index).ok()?)
                .map(|atom| Cow::Borrowed(atom.text)),
            Atom::File(index) => self
                .atoms
                .get(usize::try_from(index).ok()?)
                .map(JsString::to_string_lossy),
            Atom::Int(value) => Some(Cow::Owned(value.to_string())),
        }
    }

    /// What the runtime holds for `atom` once a verifying read has interned
    /// it: a file atom whose text is a built-in string or a canonical
    /// integer becomes that atom (`JS_NewAtomStr`). `None` for a file index
    /// outside the table or a built-in index past `JS_ATOM_END`.
    #[must_use]
    pub fn resolve(&self, atom: Atom) -> Option<ResolvedAtom> {
        match atom {
            Atom::Builtin(0) => Some(ResolvedAtom::Null),
            Atom::Builtin(index) => {
                let info = ATOMS.get(usize::try_from(index).ok()?)?;
                Some(match info.kind {
                    AtomKind::Null => ResolvedAtom::Null,
                    AtomKind::String => ResolvedAtom::BuiltinString(index),
                    AtomKind::Symbol | AtomKind::PrivateSymbol => {
                        ResolvedAtom::BuiltinSymbol(index)
                    }
                })
            }
            Atom::Int(value) => (value <= atom_int::MAX_INT).then_some(ResolvedAtom::Int(value)),
            Atom::File(index) => {
                let text = self.atoms.get(usize::try_from(index).ok()?)?;
                Some(match runtime_atom_of(text) {
                    Some(Atom::Int(value)) => ResolvedAtom::Int(value),
                    Some(Atom::Builtin(index)) => ResolvedAtom::BuiltinString(index),
                    _ => ResolvedAtom::String,
                })
            }
        }
    }
}

/// The built-in or integer atom a string interns to, if it is one.
pub(crate) fn runtime_atom_of(text: &JsString) -> Option<Atom> {
    if let Some(value) = canonical_integer(text) {
        return Some(Atom::Int(value));
    }
    let utf8 = match text {
        JsString::Latin1(bytes) if bytes.is_ascii() => std::str::from_utf8(bytes).ok()?.to_owned(),
        JsString::Latin1(_) => return None,
        JsString::Utf16(units) => {
            if units.iter().any(|&unit| unit >= 0x80) {
                return None;
            }
            units.iter().map(|&unit| char::from(unit as u8)).collect()
        }
    };
    builtin_string_atom(&utf8).map(Atom::Builtin)
}

/// `is_num_string` followed by the `JS_ATOM_MAX_INT` test of
/// `JS_NewAtomStr`: the value of a canonical decimal integer string.
fn canonical_integer(text: &JsString) -> Option<u32> {
    let len = text.len();
    if len == 0 || len > 10 {
        return None;
    }
    let digit = |index: usize| {
        let unit = text.unit(index);
        (u32::from(b'0')..=u32::from(b'9'))
            .contains(&unit)
            .then(|| unit - u32::from(b'0'))
    };
    let first = digit(0)?;
    if first == 0 && len != 1 {
        return None;
    }
    let mut value = u64::from(first);
    for index in 1..len {
        value = value * 10 + u64::from(digit(index)?);
        if value >> 32 != 0 {
            return None;
        }
    }
    let value = u32::try_from(value).ok()?;
    (value <= atom_int::MAX_INT).then_some(value)
}

/// What an atom is once the runtime holds it ([`Program::resolve`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolvedAtom {
    /// `JS_ATOM_NULL`.
    Null,
    /// An integer atom.
    Int(u32),
    /// A built-in string atom.
    BuiltinString(u32),
    /// A built-in symbol (`Symbol.iterator`, ...) or private symbol
    /// (`<brand>`).
    BuiltinSymbol(u32),
    /// A string atom of the file's table.
    String,
}

/// A JavaScript string as the format stores it (`JS_WriteString`): Latin-1
/// bytes or UTF-16 code units. UTF-16 strings may hold unpaired surrogates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsString {
    /// One byte per character.
    Latin1(Vec<u8>),
    /// One code unit per character.
    Utf16(Vec<u16>),
}

impl JsString {
    /// The string for Rust text: Latin-1 when every character fits in a
    /// byte, UTF-16 otherwise, as the engine stores strings.
    #[must_use]
    #[allow(
        clippy::should_implement_trait,
        reason = "infallible; FromStr would add an error type for nothing"
    )]
    pub fn from_str(text: &str) -> Self {
        if text.chars().all(|ch| u32::from(ch) < 0x100) {
            Self::Latin1(text.chars().map(|ch| ch as u8).collect())
        } else {
            Self::Utf16(text.encode_utf16().collect())
        }
    }

    /// Number of characters (code units).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Latin1(bytes) => bytes.len(),
            Self::Utf16(units) => units.len(),
        }
    }

    /// Whether the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The code unit at `index`.
    fn unit(&self, index: usize) -> u32 {
        match self {
            Self::Latin1(bytes) => u32::from(bytes[index]),
            Self::Utf16(units) => u32::from(units[index]),
        }
    }

    /// The code units, as the engine compares strings.
    #[must_use]
    pub fn to_utf16(&self) -> Vec<u16> {
        match self {
            Self::Latin1(bytes) => bytes.iter().map(|&byte| u16::from(byte)).collect(),
            Self::Utf16(units) => units.clone(),
        }
    }

    /// Whether both hold the same code units, whatever their widths.
    #[must_use]
    pub fn same_text(&self, other: &Self) -> bool {
        self.len() == other.len()
            && (0..self.len()).all(|index| self.unit(index) == other.unit(index))
    }

    /// The string as Rust text; unpaired surrogates become U+FFFD.
    #[must_use]
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        match self {
            Self::Latin1(bytes) if bytes.is_ascii() => {
                Cow::Borrowed(std::str::from_utf8(bytes).unwrap_or_default())
            }
            Self::Latin1(bytes) => Cow::Owned(bytes.iter().copied().map(char::from).collect()),
            Self::Utf16(units) => Cow::Owned(String::from_utf16_lossy(units)),
        }
    }
}

/// An atom as a record field or an instruction operand holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Atom {
    /// A built-in atom, below `JS_ATOM_END`; `0` is `JS_ATOM_NULL`. See
    /// [`crate::bytecode_abi::ATOMS`].
    Builtin(u32),
    /// An index into [`Program::atoms`].
    File(u32),
    /// An integer atom, at most `JS_ATOM_MAX_INT`.
    Int(u32),
}

impl Atom {
    /// `JS_ATOM_NULL`.
    pub const NULL: Self = Self::Builtin(0);
}

/// A serialized value (`JS_WriteObjectRec`), restricted to the kinds a
/// verifying read accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`
    Null,
    /// `undefined`
    Undefined,
    /// `true` or `false`
    Bool(bool),
    /// A 32-bit integer.
    Int32(i32),
    /// A double; its bits are written as they are.
    Float64(f64),
    /// A string.
    String(JsString),
    /// A plain object: its enumerable string-keyed value properties in
    /// order.
    Object(Vec<(Atom, Value)>),
    /// An array.
    Array(Vec<Value>),
    /// A `BigInt`: two's-complement bytes, least significant first; empty
    /// for zero.
    BigInt(Vec<u8>),
    /// A template object: its elements, then its `raw` property.
    TemplateObject {
        /// The cooked strings.
        elements: Vec<Value>,
        /// The `raw` property, normally an array of strings.
        raw: Box<Value>,
    },
    /// Function bytecode: a constant-pool entry, a module's function or the
    /// top-level value.
    Function(Box<Function>),
    /// A module record: only as the top-level value.
    Module(Box<Module>),
}

/// `func_kind` of a function record (`JSFunctionKindEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionKind {
    /// `JS_FUNC_NORMAL`
    Normal,
    /// `JS_FUNC_GENERATOR`
    Generator,
    /// `JS_FUNC_ASYNC`
    Async,
    /// `JS_FUNC_ASYNC_GENERATOR`
    AsyncGenerator,
}

impl FunctionKind {
    /// The 2-bit field value.
    #[must_use]
    pub const fn bits(self) -> u8 {
        match self {
            Self::Normal => func_kind::NORMAL,
            Self::Generator => func_kind::GENERATOR,
            Self::Async => func_kind::ASYNC,
            Self::AsyncGenerator => func_kind::ASYNC_GENERATOR,
        }
    }

    /// The kind for a 2-bit field value.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 3 {
            func_kind::GENERATOR => Self::Generator,
            func_kind::ASYNC => Self::Async,
            func_kind::ASYNC_GENERATOR => Self::AsyncGenerator,
            _ => Self::Normal,
        }
    }
}

/// The 16-bit flag word of a function record, bit 0 first. `has_debug`
/// (bit 10) is [`Function::debug`] being present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per bit of the serialized flag word"
)]
pub struct FunctionFlags {
    /// Bit 0.
    pub has_prototype: bool,
    /// Bit 1.
    pub has_simple_parameter_list: bool,
    /// Bit 2.
    pub is_derived_class_constructor: bool,
    /// Bit 3.
    pub need_home_object: bool,
    /// Bits 4 and 5.
    pub kind: FunctionKind,
    /// Bit 6.
    pub new_target_allowed: bool,
    /// Bit 7.
    pub super_call_allowed: bool,
    /// Bit 8.
    pub super_allowed: bool,
    /// Bit 9.
    pub arguments_allowed: bool,
    /// Bit 11.
    pub is_direct_or_indirect_eval: bool,
}

impl FunctionFlags {
    /// Every flag false, of the given kind.
    #[must_use]
    pub const fn new(kind: FunctionKind) -> Self {
        Self {
            has_prototype: false,
            has_simple_parameter_list: false,
            is_derived_class_constructor: false,
            need_home_object: false,
            kind,
            new_target_allowed: false,
            super_call_allowed: false,
            super_allowed: false,
            arguments_allowed: false,
            is_direct_or_indirect_eval: false,
        }
    }
}

/// One vardef (`JSBytecodeVarDef`): arguments first, then locals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per bit of the serialized flag byte"
)]
pub struct VarDef {
    /// Variable name.
    pub name: Atom,
    /// `scope_next`: the next variable of the scope chain, `-1` for none,
    /// `ARG_SCOPE_END` (`-2`) at the end of the argument scope. Serialized
    /// plus one. Read only by a direct `eval`.
    pub scope_next: i32,
    /// For a captured variable, its slot in the frame's variable
    /// references, below [`Function::var_ref_count`].
    pub var_ref_idx: u16,
    /// `JSVarKindEnum`, four bits.
    pub var_kind: u8,
    /// `const` binding.
    pub is_const: bool,
    /// Lexical binding.
    pub is_lexical: bool,
    /// Captured by a closure, an `arguments` object or a reference.
    pub is_captured: bool,
    /// Declared in a block scope (read by a direct `eval`).
    pub has_scope: bool,
}

impl VarDef {
    /// An uncaptured, non-lexical variable.
    #[must_use]
    pub const fn new(name: Atom) -> Self {
        Self {
            name,
            scope_next: -1,
            var_ref_idx: 0,
            var_kind: 0,
            is_const: false,
            is_lexical: false,
            is_captured: false,
            has_scope: false,
        }
    }
}

/// `closure_type` of a closure variable (`JSClosureTypeEnum`). The 3-bit
/// field has no other values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClosureType {
    /// A local of the parent: `var_idx` is its local index.
    Local,
    /// An argument of the parent.
    Arg,
    /// A closure variable of the parent.
    Ref,
    /// A closure variable of the parent that is a global binding.
    GlobalRef,
    /// A global declaration of a script.
    GlobalDecl,
    /// A global binding a script or module reads.
    Global,
    /// A module binding.
    ModuleDecl,
    /// A module import, filled when the module links.
    ModuleImport,
}

impl ClosureType {
    /// The field value.
    #[must_use]
    pub const fn bits(self) -> u8 {
        match self {
            Self::Local => closure_type::LOCAL,
            Self::Arg => closure_type::ARG,
            Self::Ref => closure_type::REF,
            Self::GlobalRef => closure_type::GLOBAL_REF,
            Self::GlobalDecl => closure_type::GLOBAL_DECL,
            Self::Global => closure_type::GLOBAL,
            Self::ModuleDecl => closure_type::MODULE_DECL,
            Self::ModuleImport => closure_type::MODULE_IMPORT,
        }
    }

    /// The type for a 3-bit field value.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 7 {
            closure_type::LOCAL => Self::Local,
            closure_type::ARG => Self::Arg,
            closure_type::REF => Self::Ref,
            closure_type::GLOBAL_REF => Self::GlobalRef,
            closure_type::GLOBAL_DECL => Self::GlobalDecl,
            closure_type::GLOBAL => Self::Global,
            closure_type::MODULE_DECL => Self::ModuleDecl,
            _ => Self::ModuleImport,
        }
    }

    /// One of the three global types.
    #[must_use]
    pub const fn is_global(self) -> bool {
        matches!(self, Self::GlobalRef | Self::GlobalDecl | Self::Global)
    }
}

/// One closure variable (`JSClosureVar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClosureVar {
    /// Variable name.
    pub name: Atom,
    /// What [`Self::closure_type`] says it indexes.
    pub var_idx: u16,
    /// Where the binding comes from.
    pub closure_type: ClosureType,
    /// `const` binding.
    pub is_const: bool,
    /// Lexical binding.
    pub is_lexical: bool,
    /// `JSVarKindEnum`, four bits.
    pub var_kind: u8,
}

impl ClosureVar {
    /// A non-lexical binding of the given type.
    #[must_use]
    pub const fn new(name: Atom, closure_type: ClosureType, var_idx: u16) -> Self {
        Self {
            name,
            var_idx,
            closure_type,
            is_const: false,
            is_lexical: false,
            var_kind: 0,
        }
    }
}

/// A function's debug block.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugInfo {
    /// Source file name.
    pub filename: Atom,
    /// The fork's pc-to-line table, as written (`compute_pc2line_info`).
    pub pc2line: Vec<u8>,
    /// Source text (`Function.prototype.toString`); empty when stripped.
    pub source: Vec<u8>,
}

/// One function record (`JS_WriteFunctionTag`).
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The flag word.
    pub flags: FunctionFlags,
    /// `js_mode`: `JS_MODE_STRICT` and `JS_MODE_BACKTRACE_BARRIER` are the
    /// bits a record may carry.
    pub js_mode: u8,
    /// Function name; [`Atom::NULL`] for an anonymous function.
    pub name: Atom,
    /// Number of arguments.
    pub arg_count: u16,
    /// Number of locals.
    pub var_count: u16,
    /// `length` of the function.
    pub defined_arg_count: u16,
    /// The stack size the bytes carry. A verifying read ignores it and
    /// computes its own, so a writer leaves it `0`; the reader keeps what
    /// it read so that read and write round-trip.
    pub stack_size: u16,
    /// Number of captured arguments and locals.
    pub var_ref_count: u16,
    /// Vardefs: `arg_count + var_count` entries, or none.
    pub vardefs: Vec<VarDef>,
    /// Closure variables.
    pub closure_vars: Vec<ClosureVar>,
    /// The instructions in bytecode order.
    pub instructions: Vec<Instruction>,
    /// The debug block (`has_debug`).
    pub debug: Option<DebugInfo>,
    /// Constant pool.
    pub cpool: Vec<Value>,
}

impl Function {
    /// An anonymous function of `kind` with no variables, no instructions
    /// and no debug block.
    #[must_use]
    pub fn new(kind: FunctionKind) -> Self {
        Self {
            flags: FunctionFlags::new(kind),
            js_mode: 0,
            name: Atom::NULL,
            arg_count: 0,
            var_count: 0,
            defined_arg_count: 0,
            stack_size: 0,
            var_ref_count: 0,
            vardefs: Vec::new(),
            closure_vars: Vec::new(),
            instructions: Vec::new(),
            debug: None,
            cpool: Vec::new(),
        }
    }

    /// The vardef of local `index`, if vardefs are present.
    #[must_use]
    pub fn local_def(&self, index: u16) -> Option<&VarDef> {
        if self.vardefs.is_empty() {
            return None;
        }
        self.vardefs
            .get(usize::from(self.arg_count) + usize::from(index))
    }

    /// The vardef of argument `index`, if vardefs are present.
    #[must_use]
    pub fn arg_def(&self, index: u16) -> Option<&VarDef> {
        if self.vardefs.is_empty() || index >= self.arg_count {
            return None;
        }
        self.vardefs.get(usize::from(index))
    }
}

/// One module request.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleRequest {
    /// The specifier as written.
    pub specifier: Atom,
    /// The import attributes: `undefined`, or an object of strings.
    pub attributes: Value,
}

/// One export entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Export {
    /// A binding of this module: a closure variable of its function.
    Local {
        /// The closure variable.
        var_idx: u32,
        /// The exported name.
        name: Atom,
    },
    /// A binding re-exported from a requested module.
    Indirect {
        /// The request.
        request: u32,
        /// The name in that module (`*` for a namespace).
        local_name: Atom,
        /// The exported name.
        name: Atom,
    },
}

/// One import entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Import {
    /// The closure variable of the module's function it fills.
    pub var_idx: u32,
    /// A namespace import (`import * as`).
    pub is_star: bool,
    /// The imported name.
    pub name: Atom,
    /// The request.
    pub request: u32,
}

/// A module record (`JS_WriteModule`).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// The module's name, which its relative imports resolve from.
    pub name: Atom,
    /// Requested modules.
    pub requests: Vec<ModuleRequest>,
    /// Export entries.
    pub exports: Vec<Export>,
    /// `export *` entries: request indices.
    pub star_exports: Vec<u32>,
    /// Import entries.
    pub imports: Vec<Import>,
    /// Whether the module body awaits at top level.
    pub has_tla: bool,
    /// The module's function.
    pub function: Function,
}

/// A typed instruction operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand {
    /// An atom.
    Atom(Atom),
    /// A local index.
    Local(u16),
    /// An argument index.
    Arg(u16),
    /// A closure-variable index.
    ClosureVar(u16),
    /// A constant-pool index.
    Const(u32),
    /// Any other numeric operand, sign- or zero-extended as its format
    /// says.
    Immediate(i32),
    /// A jump target: an index into the same function's instructions.
    Target(u32),
}

/// The kind of one operand position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperandKind {
    /// [`Operand::Atom`]
    Atom,
    /// [`Operand::Local`]
    Local,
    /// [`Operand::Arg`]
    Arg,
    /// [`Operand::ClosureVar`]
    ClosureVar,
    /// [`Operand::Const`]
    Const,
    /// [`Operand::Immediate`]
    Immediate,
    /// [`Operand::Target`]
    Target,
}

impl Operand {
    /// The kind of this operand.
    #[must_use]
    pub const fn kind(self) -> OperandKind {
        match self {
            Self::Atom(_) => OperandKind::Atom,
            Self::Local(_) => OperandKind::Local,
            Self::Arg(_) => OperandKind::Arg,
            Self::ClosureVar(_) => OperandKind::ClosureVar,
            Self::Const(_) => OperandKind::Const,
            Self::Immediate(_) => OperandKind::Immediate,
            Self::Target(_) => OperandKind::Target,
        }
    }
}

/// How one operand position is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Encoding {
    /// Not encoded: implied by the opcode.
    Implicit(i32),
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    /// A relative jump offset of this many bytes, relative to the operand's
    /// own position.
    Label(u8),
}

/// The operand positions of `opcode`, in encoding order.
pub(crate) fn layout(opcode: u8) -> &'static [(OperandKind, Encoding)] {
    use Encoding as E;
    use OperandKind as K;
    let info = &OPCODES[usize::from(opcode)];
    match info.format {
        OperandFormat::None => &[],
        OperandFormat::NoneInt
        | OperandFormat::NoneLoc
        | OperandFormat::NoneArg
        | OperandFormat::NoneVarRef
        | OperandFormat::Npopx => IMPLICIT[usize::from(opcode)],
        OperandFormat::U8 => &[(K::Immediate, E::U8)],
        OperandFormat::I8 => &[(K::Immediate, E::I8)],
        OperandFormat::Loc8 => &[(K::Local, E::U8)],
        OperandFormat::Const8 => &[(K::Const, E::U8)],
        OperandFormat::Label8 => &[(K::Target, E::Label(1))],
        OperandFormat::U16 | OperandFormat::Npop => &[(K::Immediate, E::U16)],
        OperandFormat::I16 => &[(K::Immediate, E::I16)],
        OperandFormat::Label16 => &[(K::Target, E::Label(2))],
        OperandFormat::NpopU16 => &[(K::Immediate, E::U16), (K::Immediate, E::U16)],
        OperandFormat::Loc => &[(K::Local, E::U16)],
        OperandFormat::Arg => &[(K::Arg, E::U16)],
        OperandFormat::VarRef => &[(K::ClosureVar, E::U16)],
        OperandFormat::U32 => &[(K::Immediate, E::U32)],
        OperandFormat::I32 => &[(K::Immediate, E::I32)],
        OperandFormat::Const => &[(K::Const, E::U32)],
        OperandFormat::Label => &[(K::Target, E::Label(4))],
        OperandFormat::Atom => &[(K::Atom, E::U32)],
        OperandFormat::AtomU8 => &[(K::Atom, E::U32), (K::Immediate, E::U8)],
        OperandFormat::AtomU16 => match opcode {
            op::MAKE_LOC_REF => &[(K::Atom, E::U32), (K::Local, E::U16)],
            op::MAKE_ARG_REF => &[(K::Atom, E::U32), (K::Arg, E::U16)],
            op::MAKE_VAR_REF_REF => &[(K::Atom, E::U32), (K::ClosureVar, E::U16)],
            _ => &[(K::Atom, E::U32), (K::Immediate, E::U16)],
        },
        OperandFormat::AtomLabelU8 => &[
            (K::Atom, E::U32),
            (K::Target, E::Label(4)),
            (K::Immediate, E::U8),
        ],
        OperandFormat::AtomLabelU16 => &[
            (K::Atom, E::U32),
            (K::Target, E::Label(4)),
            (K::Immediate, E::U16),
        ],
        OperandFormat::LabelU16 => &[(K::Target, E::Label(4)), (K::Immediate, E::U16)],
    }
}

/// The implicit operand of each short form, indexed by opcode byte.
static IMPLICIT: [&[(OperandKind, Encoding)]; 256] = implicit_table();

const fn implicit_table() -> [&'static [(OperandKind, Encoding)]; 256] {
    use Encoding as E;
    use OperandKind as K;
    const EMPTY: &[(OperandKind, Encoding)] = &[];
    const fn imm(value: i32) -> &'static [(OperandKind, Encoding)] {
        match value {
            -1 => &[(K::Immediate, E::Implicit(-1))],
            0 => &[(K::Immediate, E::Implicit(0))],
            1 => &[(K::Immediate, E::Implicit(1))],
            2 => &[(K::Immediate, E::Implicit(2))],
            3 => &[(K::Immediate, E::Implicit(3))],
            4 => &[(K::Immediate, E::Implicit(4))],
            5 => &[(K::Immediate, E::Implicit(5))],
            6 => &[(K::Immediate, E::Implicit(6))],
            _ => &[(K::Immediate, E::Implicit(7))],
        }
    }
    const fn indexed(kind: OperandKind, value: i32) -> &'static [(OperandKind, Encoding)] {
        match (kind, value) {
            (K::Local, 0) => &[(K::Local, E::Implicit(0))],
            (K::Local, 1) => &[(K::Local, E::Implicit(1))],
            (K::Local, 2) => &[(K::Local, E::Implicit(2))],
            (K::Local, _) => &[(K::Local, E::Implicit(3))],
            (K::Arg, 0) => &[(K::Arg, E::Implicit(0))],
            (K::Arg, 1) => &[(K::Arg, E::Implicit(1))],
            (K::Arg, 2) => &[(K::Arg, E::Implicit(2))],
            (K::Arg, _) => &[(K::Arg, E::Implicit(3))],
            (_, 0) => &[(K::ClosureVar, E::Implicit(0))],
            (_, 1) => &[(K::ClosureVar, E::Implicit(1))],
            (_, 2) => &[(K::ClosureVar, E::Implicit(2))],
            (_, _) => &[(K::ClosureVar, E::Implicit(3))],
        }
    }
    let mut table = [EMPTY; 256];
    let mut i = 0;
    while i < 8 {
        table[(op::PUSH_0 as usize) + i] = imm(i as i32);
        i += 1;
    }
    table[op::PUSH_MINUS1 as usize] = imm(-1);
    let mut n = 0;
    while n < 4 {
        table[op::GET_LOC0 as usize + n] = indexed(K::Local, n as i32);
        table[op::PUT_LOC0 as usize + n] = indexed(K::Local, n as i32);
        table[op::SET_LOC0 as usize + n] = indexed(K::Local, n as i32);
        table[op::GET_ARG0 as usize + n] = indexed(K::Arg, n as i32);
        table[op::PUT_ARG0 as usize + n] = indexed(K::Arg, n as i32);
        table[op::SET_ARG0 as usize + n] = indexed(K::Arg, n as i32);
        table[op::GET_VAR_REF0 as usize + n] = indexed(K::ClosureVar, n as i32);
        table[op::PUT_VAR_REF0 as usize + n] = indexed(K::ClosureVar, n as i32);
        table[op::SET_VAR_REF0 as usize + n] = indexed(K::ClosureVar, n as i32);
        table[op::CALL0 as usize + n] = imm(n as i32);
        n += 1;
    }
    table
}

/// One instruction: an opcode of the final bytecode and its typed operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instruction {
    opcode: u8,
    operands: [Operand; 3],
    operand_count: u8,
}

impl Instruction {
    /// An instruction with `opcode` (an [`op`] constant) and `operands` in
    /// encoding order, implicit operands of the short forms included
    /// (`get_loc2` takes `[Local(2)]`, `call1` takes `[Immediate(1)]`).
    /// Refuses an opcode outside the final bytecode (`invalid` included),
    /// operands that do not match the opcode's format, an implicit operand
    /// other than the opcode's own, and an immediate outside its encoded
    /// width.
    pub fn new(opcode: u8, operands: &[Operand]) -> Result<Self, ModelError> {
        if opcode == op::INVALID || usize::from(opcode) >= OPCODES.len() {
            return Err(ModelError::new(format!(
                "opcode {opcode} is not in the final bytecode"
            )));
        }
        let shape = layout(opcode);
        let name = OPCODES[usize::from(opcode)].name;
        if shape.len() != operands.len() {
            return Err(ModelError::new(format!(
                "{name} takes {} operands, not {}",
                shape.len(),
                operands.len()
            )));
        }
        for (&(kind, encoding), &operand) in shape.iter().zip(operands) {
            if operand.kind() != kind {
                return Err(ModelError::new(format!(
                    "{name}: operand {operand:?} where a {kind:?} goes"
                )));
            }
            if !fits(operand, encoding) {
                return Err(ModelError::new(format!(
                    "{name}: operand {operand:?} does not fit its encoding"
                )));
            }
        }
        let mut stored = [Operand::Immediate(0); 3];
        stored[..operands.len()].copy_from_slice(operands);
        Ok(Self {
            opcode,
            operands: stored,
            operand_count: u8::try_from(operands.len()).unwrap_or(3),
        })
    }

    /// An instruction whose operands are all implicit or absent
    /// (`add`, `get_loc0`, `call2`): the operands follow from the opcode.
    pub fn simple(opcode: u8) -> Result<Self, ModelError> {
        if opcode == op::INVALID || usize::from(opcode) >= OPCODES.len() {
            return Err(ModelError::new(format!(
                "opcode {opcode} is not in the final bytecode"
            )));
        }
        let operands: Vec<Operand> = layout(opcode)
            .iter()
            .map(|&(kind, encoding)| match (kind, encoding) {
                (OperandKind::Local, Encoding::Implicit(n)) => Operand::Local(n as u16),
                (OperandKind::Arg, Encoding::Implicit(n)) => Operand::Arg(n as u16),
                (OperandKind::ClosureVar, Encoding::Implicit(n)) => Operand::ClosureVar(n as u16),
                (_, Encoding::Implicit(n)) => Operand::Immediate(n),
                _ => Operand::Immediate(i32::MIN),
            })
            .collect();
        Self::new(opcode, &operands)
    }

    /// The opcode byte.
    #[must_use]
    pub const fn opcode(&self) -> u8 {
        self.opcode
    }

    /// The opcode's table entry: name, size, fixed stack effect, format.
    #[must_use]
    pub fn info(&self) -> &'static OpcodeInfo {
        &OPCODES[usize::from(self.opcode)]
    }

    /// The operands in encoding order, implicit ones included.
    #[must_use]
    pub fn operands(&self) -> &[Operand] {
        &self.operands[..usize::from(self.operand_count)]
    }

    /// Mutable operands, for retargeting jumps. A changed operand must keep
    /// its kind and fit its encoding, which the writer checks.
    pub fn operands_mut(&mut self) -> &mut [Operand] {
        &mut self.operands[..usize::from(self.operand_count)]
    }

    /// Encoded size in bytes.
    #[must_use]
    pub fn size(&self) -> u8 {
        self.info().size
    }

    /// Values popped: the fixed count plus a call's arguments, as the
    /// fork's stack-size computation counts them.
    #[must_use]
    pub fn pops(&self) -> u32 {
        let info = self.info();
        let base = u32::from(info.pops);
        match info.format {
            OperandFormat::Npop | OperandFormat::NpopU16 | OperandFormat::Npopx => {
                base + self
                    .first_immediate()
                    .and_then(|count| u32::try_from(count).ok())
                    .unwrap_or(0)
            }
            _ => base,
        }
    }

    /// Values pushed.
    #[must_use]
    pub fn pushes(&self) -> u32 {
        u32::from(self.info().pushes)
    }

    /// The atom operand, if the format has one.
    #[must_use]
    pub fn atom(&self) -> Option<Atom> {
        self.operands().iter().find_map(|operand| match operand {
            Operand::Atom(atom) => Some(*atom),
            _ => None,
        })
    }

    /// The jump target (an instruction index), if the format has one.
    #[must_use]
    pub fn target(&self) -> Option<u32> {
        self.operands().iter().find_map(|operand| match operand {
            Operand::Target(index) => Some(*index),
            _ => None,
        })
    }

    /// The first immediate operand, if the format has one.
    #[must_use]
    pub fn first_immediate(&self) -> Option<i32> {
        self.operands().iter().find_map(|operand| match operand {
            Operand::Immediate(value) => Some(*value),
            _ => None,
        })
    }

    /// The last immediate operand (the flags byte of `atom_u8` formats,
    /// the scope of `eval`), if the format has one.
    #[must_use]
    pub fn last_immediate(&self) -> Option<i32> {
        self.operands()
            .iter()
            .rev()
            .find_map(|operand| match operand {
                Operand::Immediate(value) => Some(*value),
                _ => None,
            })
    }
}

/// Whether `operand` can be encoded as `encoding`.
pub(crate) fn fits(operand: Operand, encoding: Encoding) -> bool {
    let value: i64 = match operand {
        Operand::Atom(_) | Operand::Target(_) => return true,
        Operand::Local(n) | Operand::Arg(n) | Operand::ClosureVar(n) => i64::from(n),
        Operand::Const(n) => i64::from(n),
        Operand::Immediate(n) => i64::from(n),
    };
    match encoding {
        Encoding::Implicit(implied) => value == i64::from(implied),
        Encoding::U8 => (0..=0xff).contains(&value),
        Encoding::I8 => (-0x80..=0x7f).contains(&value),
        Encoding::U16 => (0..=0xffff).contains(&value),
        Encoding::I16 => (-0x8000..=0x7fff).contains(&value),
        // A u32 immediate is held as its bits in an i32.
        Encoding::U32 => match operand {
            Operand::Const(_) | Operand::Immediate(_) => true,
            _ => (0..=0xffff_ffff).contains(&value),
        },
        Encoding::I32 => true,
        Encoding::Label(_) => false,
    }
}

/// The built-in atom count, as a `u32`.
pub(crate) const ATOM_END: u32 = JS_ATOM_END;
