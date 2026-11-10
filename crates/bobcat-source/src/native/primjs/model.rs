//! The decoded program: what [`super::read_program`] returns.

use std::borrow::Cow;

use super::tables::{ATOM_END, Format, Opcode, builtin_atom};

/// A decoded `PrimJS` serialized program.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// The header's version byte and flags.
    pub header: Header,
    /// The file's atom table: every atom that is not built in, in file order.
    /// [`Atom::File`] indexes this vector.
    pub atoms: Vec<JsString>,
    /// The top-level function (a global script for every section a bundle
    /// carries).
    pub root: Function,
}

impl Program {
    /// The text of `atom`, or `None` for a file index outside the table
    /// (which a program returned by the reader never holds).
    #[must_use]
    pub fn atom_text(&self, atom: Atom) -> Option<Cow<'_, str>> {
        match atom {
            Atom::Builtin(index) => builtin_atom(index).map(Cow::Borrowed),
            Atom::File(index) => self
                .atoms
                .get(usize::try_from(index).ok()?)
                .map(JsString::to_string_lossy),
            Atom::TaggedInt(value) => Some(Cow::Owned(value.to_string())),
        }
    }
}

/// The serialized header (`JS_WriteObjectAtoms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Header {
    /// Version byte `1`, no version word: written for a target SDK below 2.14.
    Legacy,
    /// Version byte `9` followed by a 64-bit word whose low 32 bits are
    /// `0xCAB001CC`.
    Versioned(HeaderFlags),
}

impl Header {
    /// The version byte as written.
    #[must_use]
    pub const fn version_byte(self) -> u8 {
        match self {
            Self::Legacy => super::VERSION_LEGACY,
            Self::Versioned(_) => super::VERSION_PREFIXED,
        }
    }

    /// The flags of a version-9 header; all false for a legacy header.
    #[must_use]
    pub const fn flags(self) -> HeaderFlags {
        match self {
            Self::Legacy => HeaderFlags {
                external_debug_info: false,
                vardefs_stripped: false,
            },
            Self::Versioned(flags) => flags,
        }
    }
}

/// Bits 32 and 33 of a version-9 header word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct HeaderFlags {
    /// `NEW_DEBUGINFO_FLAG` (bit 32): debug blocks carry a function id, not
    /// line tables.
    pub external_debug_info: bool,
    /// `STRIP_VARINFO_FLAG` (bit 33): functions without a direct `eval` carry
    /// no vardefs.
    pub vardefs_stripped: bool,
}

/// A JavaScript string as `PrimJS` serializes it: Latin-1 bytes or UTF-16 code
/// units (`JS_WriteString`). UTF-16 strings may hold unpaired surrogates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JsString {
    /// One byte per character, Latin-1.
    Latin1(Vec<u8>),
    /// One `u16` code unit per character.
    Utf16(Vec<u16>),
}

impl JsString {
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

/// An atom as it appears in a record field or an instruction operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Atom {
    /// A built-in atom, `0..ATOM_END`; `0` is `JS_ATOM_NULL`. See
    /// [`super::BUILTIN_ATOMS`].
    Builtin(u32),
    /// Index into [`Program::atoms`].
    File(u32),
    /// An integer atom (an array index), `0..2^31`.
    TaggedInt(u32),
}

impl Atom {
    /// `JS_ATOM_NULL`.
    pub const NULL: Self = Self::Builtin(0);

    /// The serialized index form used in instruction operands: built-in
    /// index, `ATOM_END + file index`, or the integer with bit 31 set.
    #[must_use]
    pub const fn to_operand(self) -> u32 {
        match self {
            Self::Builtin(index) => index,
            Self::File(index) => ATOM_END + index,
            Self::TaggedInt(value) => value | super::ATOM_TAG_INT,
        }
    }
}

/// A serialized value (`JS_WriteObjectRec`).
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
    /// A double; the bits are kept as read.
    Float64(f64),
    /// A string.
    String(JsString),
    /// A plain object: enumerable string-keyed value properties in order.
    Object(Vec<(Atom, Value)>),
    /// An array.
    Array(Vec<Value>),
    /// A `BigInt`: two's-complement bytes, least significant first. Empty for
    /// zero.
    BigInt(Vec<u8>),
    /// A template object (the strings array of a tagged template): its
    /// elements, then the value of its `raw` property.
    TemplateObject {
        /// The cooked strings.
        elements: Vec<Value>,
        /// The `raw` property, normally an array of strings.
        raw: Box<Value>,
    },
    /// Function bytecode.
    Function(Box<Function>),
}

/// `func_kind` of a function record.
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

/// The 16-bit flag word of a function record, bit 0 first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
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
    /// Bit 10: a debug block follows the bytecode.
    pub has_debug: bool,
}

/// `js_mode` bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JsMode(pub u8);

impl JsMode {
    /// `JS_MODE_STRICT`
    pub const STRICT: u8 = 1 << 0;
    /// `JS_MODE_STRIP`
    pub const STRIP: u8 = 1 << 1;
    /// `JS_MODE_BIGINT`
    pub const BIGINT: u8 = 1 << 2;
    /// `JS_MODE_MATH`
    pub const MATH: u8 = 1 << 3;
    /// `JS_MODE_ASYNC`
    pub const ASYNC: u8 = 1 << 4;
    /// Every bit `PrimJS` defines.
    pub const KNOWN: u8 = 0x1f;

    /// Whether the function is strict-mode code.
    #[must_use]
    pub const fn is_strict(self) -> bool {
        self.0 & Self::STRICT != 0
    }
}

/// One entry of a function's vardefs: arguments first, then locals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VarDef {
    /// Variable name.
    pub name: Atom,
    /// Index of the variable's lexical scope (at most `0xFFFFFF`).
    pub scope_level: u32,
    /// `scope_next`: index of the next variable in the same or an enclosing
    /// scope, or a negative value (`-1` for none; `PrimJS` also writes `-2`).
    /// Serialized plus one as a 32-bit value.
    pub scope_next: i32,
    /// `JSVarKindEnum`, four bits.
    pub var_kind: u8,
    /// `const` binding.
    pub is_const: bool,
    /// Lexical (`let`, `const`, class) binding.
    pub is_lexical: bool,
    /// Captured by a closure.
    pub is_captured: bool,
}

/// One closure variable of a function (`LEPUSClosureVar`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per bit of the serialized flag byte"
)]
pub struct ClosureVar {
    /// Variable name.
    pub name: Atom,
    /// With `is_local`, an argument (`is_arg`) or local of the parent;
    /// otherwise a closure variable of the parent.
    pub var_idx: u32,
    /// Refers to the parent's own argument or local.
    pub is_local: bool,
    /// With `is_local`, refers to an argument.
    pub is_arg: bool,
    /// `const` binding.
    pub is_const: bool,
    /// Lexical binding.
    pub is_lexical: bool,
    /// `JSVarKindEnum`, four bits.
    pub var_kind: u8,
}

/// A function's debug block. Both forms share one byte layout (atom, LEB128
/// field, LEB128 length and that many bytes); which one was written follows
/// from the header and, for a legacy header, from the filename.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DebugInfo {
    /// Line information inside the file.
    Inline {
        /// Source file name.
        filename: Atom,
        /// First line of the function.
        line_num: u32,
        /// `PrimJS` pc-to-line table, as written.
        pc2line: Vec<u8>,
    },
    /// Line information kept outside the bytecode (`debuginfo_outside`):
    /// either a version-9 header with `NEW_DEBUGINFO_FLAG`, or a legacy header
    /// whose filename atom is `__lepusNG_function_id__`. The length field is
    /// always zero.
    External {
        /// Source file name, or the `__lepusNG_function_id__` marker.
        filename: Atom,
        /// The function's id in the external debug info.
        function_id: u32,
    },
}

/// One function record (`JS_WriteFunction`).
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// Offset of the record's tag byte in the input.
    pub offset: usize,
    /// The flag word.
    pub flags: FunctionFlags,
    /// `js_mode`.
    pub js_mode: JsMode,
    /// Function name; [`Atom::NULL`] for an anonymous function.
    pub name: Atom,
    /// Number of arguments.
    pub arg_count: u16,
    /// Number of locals.
    pub var_count: u16,
    /// `length` of the function.
    pub defined_arg_count: u16,
    /// Maximum stack depth the `PrimJS` compiler computed.
    pub stack_size: u16,
    /// Vardefs, `arg_count + var_count` entries, or empty when stripped.
    pub vardefs: Vec<VarDef>,
    /// Closure variables.
    pub closure_vars: Vec<ClosureVar>,
    /// Offset of the first bytecode byte in the input.
    pub bytecode_offset: usize,
    /// Bytecode length in bytes.
    pub bytecode_len: u32,
    /// The decoded instructions in bytecode order.
    pub instructions: Vec<Instruction>,
    /// The debug block, when `flags.has_debug`.
    pub debug: Option<DebugInfo>,
    /// Constant pool. Entries referenced by `fclosure` are functions whose
    /// closure variables name slots of this function.
    pub cpool: Vec<Value>,
}

impl Function {
    /// Number of closure variables.
    #[must_use]
    pub fn closure_var_count(&self) -> u32 {
        u32::try_from(self.closure_vars.len()).unwrap_or(u32::MAX)
    }

    /// The index of the instruction that starts at `pc`, if one does.
    #[must_use]
    pub fn instruction_at(&self, pc: u32) -> Option<usize> {
        self.instructions
            .binary_search_by_key(&pc, |instruction| instruction.pc)
            .ok()
    }
}

/// A typed instruction operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand {
    /// An atom.
    Atom(Atom),
    /// A local index, below the function's `var_count`.
    Local(u16),
    /// An argument index, below the function's `arg_count`.
    Arg(u16),
    /// A closure-variable index, below the function's closure-variable count.
    ClosureVar(u16),
    /// A constant-pool index, below the function's pool length.
    Const(u32),
    /// Any other numeric operand, sign-extended or zero-extended as its
    /// format says (an `i8`, `u8`, `i16`, `u16` or `i32`; a call's argument
    /// count; a flags byte).
    Immediate(i32),
    /// A jump target: the index in [`Function::instructions`] of the
    /// instruction the jump lands on.
    Target(u32),
}

/// One decoded instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instruction {
    /// Offset of the opcode byte in the function's bytecode.
    pub pc: u32,
    /// The opcode.
    pub opcode: Opcode,
    operands: [Operand; 3],
    operand_count: u8,
}

impl Instruction {
    pub(super) fn new(pc: u32, opcode: Opcode, operands: &[Operand]) -> Self {
        let mut stored = [Operand::Immediate(0); 3];
        stored[..operands.len()].copy_from_slice(operands);
        Self {
            pc,
            opcode,
            operands: stored,
            operand_count: u8::try_from(operands.len()).unwrap_or(3),
        }
    }

    /// The operands in encoding order. Implicit operands of short forms are
    /// included: `get_loc2` has `[Local(2)]`, `push_3` has `[Immediate(3)]`,
    /// `call1` has `[Immediate(1)]`.
    #[must_use]
    pub fn operands(&self) -> &[Operand] {
        &self.operands[..usize::from(self.operand_count)]
    }

    pub(super) fn operands_mut(&mut self) -> &mut [Operand] {
        &mut self.operands[..usize::from(self.operand_count)]
    }

    /// Encoded size in bytes.
    #[must_use]
    pub fn size(&self) -> u8 {
        self.opcode.size()
    }

    /// Values popped, including a call's arguments.
    #[must_use]
    pub fn pops(&self) -> u32 {
        let base = u32::from(self.opcode.pops());
        match self.opcode.format() {
            Format::Npop | Format::Npopx => {
                base + self
                    .immediate()
                    .and_then(|count| u32::try_from(count).ok())
                    .unwrap_or(0)
            }
            _ => base,
        }
    }

    /// Values pushed.
    #[must_use]
    pub fn pushes(&self) -> u32 {
        u32::from(self.opcode.pushes())
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
    pub fn immediate(&self) -> Option<i32> {
        self.operands().iter().find_map(|operand| match operand {
            Operand::Immediate(value) => Some(*value),
            _ => None,
        })
    }
}
