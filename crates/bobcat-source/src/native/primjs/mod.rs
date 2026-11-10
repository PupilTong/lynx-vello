//! Reader, validator and disassembler for `PrimJS` serialized bytecode.
//!
//! A native `.lynx.bundle` carries JavaScript as `PrimJS` bytecode in four
//! places: `ROOT_LEPUS`, the entries of `LEPUS_CHUNK`, the files of
//! `JS_BYTECODE` and custom sections whose encoding is `JsBytecode`.
//! [`bundle_bytecode_sections`] finds them; [`read_payload`] (or
//! [`read_program`] for bytes without a packed header) turns one into a
//! [`Program`]. Nothing here converts or executes bytecode; the design this
//! implements is `docs/primjs-bytecode-conversion-design.md` (§2, §4.1,
//! §4.2). The format follows `JS_WriteObjectAtoms`, `JS_WriteFunction` and
//! their readers in `src/interpreter/quickjs/source/quickjs.cc` of
//! `lynx-family/primjs` at `941a365`.
//!
//! # Input robustness
//!
//! Every function here returns `Ok` or `Err` for any input: it does not
//! panic, recursion is bounded by [`NESTING_LIMIT`] levels of nested values
//! and functions, and every count is checked against the bytes that remain
//! before anything is allocated for it. A program that fails any check is an
//! error; no partial program is returned.
//!
//! # The program model
//!
//! [`Program`] holds the [`Header`], the file's atom table
//! ([`Program::atoms`], [`JsString`] each) and the top-level [`Function`].
//!
//! **Atoms.** Every atom, in a record field or an instruction operand, is an
//! [`Atom`]: `Builtin(i)` for a built-in atom (`i < ATOM_END`, text in
//! [`BUILTIN_ATOMS`]; `Builtin(0)` is `JS_ATOM_NULL`), `File(i)` for
//! `program.atoms[i]`, or `TaggedInt(n)` for an integer atom.
//! [`Program::atom_text`] resolves any of them. The reader guarantees every
//! `File` index is inside the table.
//!
//! **Functions.** [`Function`] carries the record fields in serialized order
//! (flags, `js_mode`, name, the counts, vardefs, closure variables), the
//! decoded [`Function::instructions`], the optional [`DebugInfo`] and the
//! constant pool ([`Function::cpool`], [`Value`] each). Nested functions are
//! [`Value::Function`] entries of their parent's pool. `vardefs` is either
//! empty (stripped, or never recorded) or has `arg_count + var_count`
//! entries, arguments first. [`Function::offset`] and
//! [`Function::bytecode_offset`] are byte offsets into the input, for error
//! messages and for people reading a hex dump.
//!
//! **Instructions.** [`Function::instructions`] lists every instruction in
//! bytecode order. Each [`Instruction`] has its [`Opcode`] (an enum over the
//! 242 shippable opcodes; [`Opcode::info`] gives name, size, pops, pushes and
//! [`Format`]), its original `pc` (byte offset inside the function's
//! bytecode) and its [`Operand`]s in encoding order:
//!
//! | Format | Operands |
//! | --- | --- |
//! | `none` | none |
//! | `none_int` (`push_minus1`..`push_7`) | `Immediate(n)`, implicit |
//! | `none_loc`, `none_arg`, `none_var_ref` (`get_loc0`..`set_var_ref3`) | `Local(n)`, `Arg(n)`, `ClosureVar(n)`, implicit |
//! | `npopx` (`call0`..`call3`) | `Immediate(argc)`, implicit |
//! | `u8`, `i8`, `u16`, `i16`, `i32`, `npop` | `Immediate` |
//! | `loc8`, `loc` / `arg` / `var_ref` | `Local` / `Arg` / `ClosureVar` |
//! | `const8`, `const` | `Const` |
//! | `label8`, `label16`, `label` | `Target` |
//! | `atom` | `Atom` |
//! | `atom_u8` | `Atom`, `Immediate` |
//! | `atom_u16` | `Atom`, then `Local` (`make_loc_ref`), `Arg` (`make_arg_ref`) or `ClosureVar` (`make_var_ref_ref`) |
//! | `atom_label_u8` (`with_*`) | `Atom`, `Target`, `Immediate` |
//!
//! `Target(i)` is an index into the same function's `instructions`, already
//! resolved from the relative byte offset (`pc + 1 + offset`, or
//! `pc + 5 + offset` for `with_*`). [`Instruction::pops`] includes a call's
//! arguments; [`Instruction::size`] is the encoded size. To re-encode a jump
//! after changing instruction sizes, map the target index to its new pc.
//!
//! # Checks
//!
//! Everything `JS_ReadFunctionBytecode` and `JS_ValidateChildClosureVars`
//! check: opcode below 242 and not `invalid`; each instruction inside the
//! function's bytecode; local, argument, closure-variable and constant-pool
//! operands below their counts, implicit ones included; each child
//! function's closure variables inside the parent's counts. In addition:
//! every atom, in an operand or a record, is built in or inside the file's
//! table; every jump target is an instruction start in the same function;
//! the header's version byte, version word and flag bits are exactly the
//! known ones; undefined flag bits in function records are rejected;
//! `arg_count`, `var_count`, `defined_arg_count` and `stack_size` fit in 16
//! bits (`PrimJS` truncates them); the vardef count is `0` or
//! `arg_count + var_count`; no bytes follow the top-level function.
//! `BC_TAG_BIG_FLOAT` and `BC_TAG_MODULE` values are rejected by ruling with
//! their own errors. Stack depth is not checked; the design leaves that to
//! the fork's verifying read.

mod code;
mod cursor;
mod disasm;
mod error;
mod model;
mod packed;
mod read;
mod sections;
mod tables;

pub use disasm::disassemble;
pub use error::PrimjsError;
pub use model::{
    Atom, ClosureVar, DebugInfo, Function, FunctionFlags, FunctionKind, Header, HeaderFlags,
    Instruction, JsMode, JsString, Operand, Program, Value, VarDef,
};
pub use packed::{
    PACKED_HEADER_LEN, PACKED_HEADER_VERSION, PACKED_MAGIC, PackedHeader, is_packed, unwrap_packed,
};
pub use read::{
    FLAG_EXTERNAL_DEBUG_INFO, FLAG_VARDEFS_STRIPPED, NESTING_LIMIT, VERSION_LEGACY,
    VERSION_PREFIXED, VERSION_WORD, read_program,
};
pub use sections::{BytecodeSection, SectionKind};
pub use tables::{
    ALL_OPCODES, ATOM_END, ATOM_TAG_INT, BUILTIN_ATOM_COUNT, BUILTIN_ATOMS, Format,
    NON_SHIPPABLE_INC_COVERAGE, OPCODE_COUNT, OPCODE_INFO, Opcode, OpcodeInfo, builtin_atom, tag,
};

/// Reads one bytecode payload as a bundle stores it: a packed buffer
/// (`JS_BYTECODE`) is unwrapped first, anything else is read as is.
pub fn read_payload(bytes: &[u8]) -> Result<(Option<PackedHeader>, Program), PrimjsError> {
    match unwrap_packed(bytes)? {
        Some((header, inner)) => Ok((Some(header), read_program(inner)?)),
        None => Ok((None, read_program(bytes)?)),
    }
}

/// Every bytecode payload of a flexible `.lynx.bundle`, with its kind and
/// name. The bundle's prologue, header and section routes are parsed by the
/// same code as [`super::decode`]; the payloads are returned unread.
pub fn bundle_bytecode_sections(
    bundle: &[u8],
) -> Result<Vec<BytecodeSection<'_>>, super::ConvertError> {
    super::decode::bytecode_sections(bundle)
}

#[cfg(test)]
mod tests {
    use super::super::decode::{
        EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG, EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG_TASM_0_0_53,
        EMPTY_ROOT_LEPUS_INLINE_DEBUG,
    };
    use super::{Opcode, read_program};

    /// The fingerprints `native::decode` accepts are real encoder output and
    /// read as the empty program `get_loc0; return`.
    #[test]
    fn decode_fingerprints_are_empty_programs() {
        for bytes in [
            EMPTY_ROOT_LEPUS_INLINE_DEBUG,
            EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG,
            EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG_TASM_0_0_53,
        ] {
            let program = read_program(bytes).unwrap();
            let opcodes: Vec<_> = program
                .root
                .instructions
                .iter()
                .map(|instruction| instruction.opcode)
                .collect();
            assert_eq!(opcodes, [Opcode::GetLoc0, Opcode::Return]);
        }
    }
}
