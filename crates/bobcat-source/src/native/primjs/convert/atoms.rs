//! `PrimJS` atoms to the fork's atoms, and the output file's atom table.
//!
//! A built-in `PrimJS` string atom becomes the fork's built-in atom with the
//! same text when the fork has one, else an entry of the output table; the
//! `PrimJS` private brand and well-known symbols (indices 189..=202, the end
//! of `quickjs-atom.h`) become the fork's atoms of the same name and kind.
//! A file atom whose text is a canonical array index becomes an integer
//! atom, as the fork's runtime interns it (`JS_NewAtomStr`). Interning goes
//! through a hash map, so building the table is linear in its size.

use std::collections::HashMap;

use quickjs_rust_bridge::bytecode::{self as fork, JsString as ForkString};
use quickjs_rust_bridge::bytecode_abi::{ATOMS, AtomKind, atom_int, builtin_string_atom};

use super::super::model::{Atom, JsString, Program};
use super::super::tables::builtin_atom;

/// `JS_ATOM_Private_brand` in `PrimJS`'s table.
const PRIMJS_PRIVATE_BRAND: u32 = 189;
/// The first well-known symbol (`Symbol.toPrimitive`) in `PrimJS`'s table.
const PRIMJS_FIRST_SYMBOL: u32 = 190;

/// Maps atoms of one `PrimJS` program and builds the output atom table.
pub(super) struct Atoms<'p> {
    source: &'p Program,
    /// The output file's atom table, in order of first interning.
    pub(super) table: Vec<ForkString>,
    by_text: HashMap<Vec<u16>, u32>,
    /// The mapping of each `PrimJS` file atom, once computed.
    file: Vec<Option<fork::Atom>>,
}

impl<'p> Atoms<'p> {
    pub(super) fn new(source: &'p Program) -> Self {
        Self {
            source,
            table: Vec::new(),
            by_text: HashMap::new(),
            file: vec![None; source.atoms.len()],
        }
    }

    /// The fork atom for a `PrimJS` atom. `None` only for an atom the reader
    /// would not have produced (a built-in index past the table, a file
    /// index past the file's table, an integer above `JS_ATOM_MAX_INT`).
    pub(super) fn map(&mut self, atom: Atom) -> Option<fork::Atom> {
        match atom {
            Atom::Builtin(0) => Some(fork::Atom::NULL),
            Atom::Builtin(index) if index == PRIMJS_PRIVATE_BRAND => {
                fork_builtin(|entry| entry.kind == AtomKind::PrivateSymbol)
            }
            Atom::Builtin(index) if index >= PRIMJS_FIRST_SYMBOL => {
                let text = builtin_atom(index)?;
                fork_builtin(|entry| entry.kind == AtomKind::Symbol && entry.text == text)
            }
            Atom::Builtin(index) => {
                let text = builtin_atom(index)?;
                Some(self.intern_str(text))
            }
            Atom::TaggedInt(value) => {
                (value <= atom_int::MAX_INT).then_some(fork::Atom::Int(value))
            }
            Atom::File(index) => {
                let slot = usize::try_from(index).ok()?;
                if let Some(mapped) = *self.file.get(slot)? {
                    return Some(mapped);
                }
                let text = to_fork_string(self.source.atoms.get(slot)?);
                let mapped = self.intern(text);
                self.file[slot] = Some(mapped);
                Some(mapped)
            }
        }
    }

    /// The atom the fork's runtime interns `text` to, adding it to the
    /// table when it is neither built in nor an integer.
    pub(super) fn intern(&mut self, text: ForkString) -> fork::Atom {
        let units = text.to_utf16();
        if let Some(value) = canonical_integer(&units) {
            return fork::Atom::Int(value);
        }
        if units.iter().all(|&unit| unit < 0x80) {
            let ascii: String = units
                .iter()
                .map(|&unit| char::from(u8::try_from(unit).unwrap_or(b'?')))
                .collect();
            if let Some(index) = builtin_string_atom(&ascii) {
                return fork::Atom::Builtin(index);
            }
        }
        if let Some(&index) = self.by_text.get(&units) {
            return fork::Atom::File(index);
        }
        let index = u32::try_from(self.table.len()).unwrap_or(u32::MAX);
        self.table.push(text);
        self.by_text.insert(units, index);
        fork::Atom::File(index)
    }

    /// [`Self::intern`] for Rust text.
    pub(super) fn intern_str(&mut self, text: &str) -> fork::Atom {
        self.intern(ForkString::from_str(text))
    }

    /// The text of an output atom, for error messages.
    pub(super) fn text(&self, atom: fork::Atom) -> String {
        match atom {
            fork::Atom::Builtin(index) => ATOMS
                .get(index as usize)
                .map_or_else(String::new, |entry| entry.text.to_owned()),
            fork::Atom::File(index) => self
                .table
                .get(index as usize)
                .map_or_else(String::new, |text| text.to_string_lossy().into_owned()),
            fork::Atom::Int(value) => value.to_string(),
        }
    }
}

/// The fork's built-in atom that `matches`.
fn fork_builtin(
    matches: impl Fn(&quickjs_rust_bridge::bytecode_abi::BuiltinAtom) -> bool,
) -> Option<fork::Atom> {
    ATOMS
        .iter()
        .position(matches)
        .and_then(|index| u32::try_from(index).ok())
        .map(fork::Atom::Builtin)
}

/// A `PrimJS` string as the fork's model holds it.
pub(super) fn to_fork_string(text: &JsString) -> ForkString {
    match text {
        JsString::Latin1(bytes) => ForkString::Latin1(bytes.clone()),
        JsString::Utf16(units) => ForkString::Utf16(units.clone()),
    }
}

/// The value of a canonical decimal integer string up to
/// `JS_ATOM_MAX_INT` (`is_num_string` and the test in `JS_NewAtomStr`).
fn canonical_integer(units: &[u16]) -> Option<u32> {
    if units.is_empty() || units.len() > 10 {
        return None;
    }
    let digit = |unit: u16| {
        (u16::from(b'0')..=u16::from(b'9'))
            .contains(&unit)
            .then(|| u64::from(unit - u16::from(b'0')))
    };
    let first = digit(units[0])?;
    if first == 0 && units.len() != 1 {
        return None;
    }
    let mut value = first;
    for &unit in &units[1..] {
        value = value * 10 + digit(unit)?;
    }
    let value = u32::try_from(value).ok()?;
    (value <= atom_int::MAX_INT).then_some(value)
}
