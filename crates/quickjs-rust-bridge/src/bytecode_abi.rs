//! The pinned `QuickJS` fork's bytecode format, as data: the format version,
//! the opcode table of the final bytecode, the built-in atom table, and the
//! enumerations a writer of the serialized format needs.
//!
//! Everything here is generated at build time from `vendor/quickjs`
//! (`quickjs-opcode.h`, `quickjs-atom.h`, and the `enum`s and `#define`s of
//! `quickjs.c` and `libregexp.h`), so a rebase of the fork regenerates it.
//! The build compiles `quickjs.c` in one translation unit with a static
//! assertion for every value generated here, so a table that disagrees with
//! the compiled engine fails the build; the atom texts and kinds, which C
//! cannot compare at compile time, are compared with a new runtime's atom
//! table by this crate's tests.
//!
//! Each opcode's `size` counts the opcode byte and its operands; `pops` and
//! `pushes` are its fixed stack effect, to which the `npop` formats add the
//! 16-bit operand and the `npopx` format (`call0`..`call3`) the opcode's
//! distance from `call0`, as the fork's stack-size computation does.

/// One opcode of the final bytecode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opcode {
    /// The `id` of its `DEF` in `quickjs-opcode.h`.
    pub name: &'static str,
    /// Instruction length in bytes, the opcode byte included.
    pub size: u8,
    pub pops: u8,
    pub pushes: u8,
    pub format: OperandFormat,
}

/// What kind of atom a built-in atom is (`JS_ATOM_TYPE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AtomKind {
    /// Atom number 0, `JS_ATOM_NULL`.
    Null,
    /// A string, found by its text when a string is interned.
    String,
    /// A well-known symbol (`Symbol.iterator`, ...); its text is the
    /// description.
    Symbol,
    /// A private symbol (`<brand>`).
    PrivateSymbol,
}

/// One built-in atom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinAtom {
    /// The `name` of its `DEF` in `quickjs-atom.h` (`JS_ATOM_<name>`); empty
    /// for the null atom.
    pub name: &'static str,
    pub text: &'static str,
    pub kind: AtomKind,
}

/// The built-in atom a string with this text interns to, as the runtime
/// interns it (`JS_NewAtom`): only string atoms are found by text.
#[must_use]
pub fn builtin_string_atom(text: &str) -> Option<u32> {
    ATOMS
        .iter()
        .position(|atom| atom.kind == AtomKind::String && atom.text == text)
        .and_then(|index| u32::try_from(index).ok())
}

include!(concat!(env!("OUT_DIR"), "/bytecode_abi.rs"));
