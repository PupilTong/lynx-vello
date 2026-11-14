//! The reader: `JS_ReadObjectAtoms`, `JS_ReadObjectRec` and `JS_ReadFunction`
//! with their checks, plus the ones `PrimJS` lacks.

use super::PrimjsError;
use super::code::{self, Limits};
use super::cursor::Cursor;
use super::model::{
    Atom, ClosureVar, DebugInfo, Function, FunctionFlags, FunctionKind, Header, HeaderFlags,
    JsMode, JsString, Program, Value, VarDef,
};
use super::tables::{ATOM_END, tag};

/// Nesting limit for values and functions, the same bound the native Lepus
/// value decoder uses.
pub const NESTING_LIMIT: usize = 128;

/// Low 32 bits of a version-9 header word: `0xCAB00000 | (242 + 16 + 202)`.
pub const VERSION_WORD: u32 = 0xCAB0_0000 | (242 + 16 + 202);
/// `NEW_DEBUGINFO_FLAG`
pub const FLAG_EXTERNAL_DEBUG_INFO: u64 = 1 << 32;
/// `STRIP_VARINFO_FLAG`
pub const FLAG_VARDEFS_STRIPPED: u64 = 1 << 33;
/// `BC_VERSION`
pub const VERSION_LEGACY: u8 = 1;
/// `BC_VERSION + BC_NEW_PREFIX`
pub const VERSION_PREFIXED: u8 = 9;
/// `BC_BE_VERSION`: the bit a byte-swapping writer flips in the version byte.
const BYTE_SWAP_BIT: u8 = 0x40;

/// The filename atom text `PrimJS` writes in place of a filename when a
/// legacy-header function carries external debug info
/// (`lepusng_functionid_str`).
const FUNCTION_ID_MARKER: &str = "__lepusNG_function_id__";

/// Smallest encoding of one vardef: atom, scope level, scope next, flags.
const MIN_VARDEF_BYTES: usize = 4;
/// Smallest encoding of one closure variable: atom, index, flags.
const MIN_CLOSURE_VAR_BYTES: usize = 3;
/// Smallest encoding of one object property: atom and a one-byte value.
const MIN_PROPERTY_BYTES: usize = 2;

/// Reads a whole serialized program: header, atom table, one top-level
/// function and nothing after it.
pub fn read_program(bytes: &[u8]) -> Result<Program, PrimjsError> {
    let mut cursor = Cursor::new(bytes);
    let header = read_header(&mut cursor)?;
    let atoms = read_atom_table(&mut cursor)?;
    let mut reader = Reader {
        cursor,
        header,
        atoms: &atoms,
    };
    let tag_offset = reader.cursor.offset();
    let tag = reader.cursor.u8()?;
    let root = match tag {
        tag::FUNCTION_BYTECODE => reader.function(tag_offset, 0)?,
        tag::MODULE => return Err(PrimjsError::Module { offset: tag_offset }),
        tag::BIG_FLOAT => return Err(PrimjsError::BigFloat { offset: tag_offset }),
        tag::NULL..=tag::TEMPLATE_OBJECT => {
            return Err(PrimjsError::RootNotFunction {
                offset: tag_offset,
                tag,
            });
        }
        _ => {
            return Err(PrimjsError::UnknownTag {
                offset: tag_offset,
                tag,
            });
        }
    };
    if reader.cursor.remaining() != 0 {
        return Err(PrimjsError::TrailingBytes {
            offset: reader.cursor.offset(),
            remaining: reader.cursor.remaining(),
        });
    }
    Ok(Program {
        header,
        atoms,
        root,
    })
}

fn read_header(cursor: &mut Cursor<'_>) -> Result<Header, PrimjsError> {
    let version = cursor.u8()?;
    match version {
        VERSION_LEGACY => Ok(Header::Legacy),
        VERSION_PREFIXED => {
            let word = cursor.u64()?;
            let low = (word & 0xFFFF_FFFF) as u32;
            if low != VERSION_WORD {
                return Err(PrimjsError::VersionWordMismatch {
                    found: low,
                    expected: VERSION_WORD,
                });
            }
            let unknown = word & !(0xFFFF_FFFF | FLAG_EXTERNAL_DEBUG_INFO | FLAG_VARDEFS_STRIPPED);
            if unknown != 0 {
                return Err(PrimjsError::UnknownHeaderFlags { bits: unknown });
            }
            Ok(Header::Versioned(HeaderFlags {
                external_debug_info: word & FLAG_EXTERNAL_DEBUG_INFO != 0,
                vardefs_stripped: word & FLAG_VARDEFS_STRIPPED != 0,
            }))
        }
        _ if version ^ BYTE_SWAP_BIT == VERSION_LEGACY
            || version ^ BYTE_SWAP_BIT == VERSION_PREFIXED =>
        {
            Err(PrimjsError::ByteSwapped { version })
        }
        _ => Err(PrimjsError::UnknownVersion { version }),
    }
}

fn read_atom_table(cursor: &mut Cursor<'_>) -> Result<Vec<JsString>, PrimjsError> {
    let offset = cursor.offset();
    let count = cursor.leb128()?;
    // Each atom takes at least its one-byte length.
    if count as usize > cursor.remaining() {
        return Err(PrimjsError::CountOutOfRange {
            offset,
            what: "atom",
            count: u64::from(count),
        });
    }
    let mut atoms = Vec::with_capacity(count as usize);
    for _ in 0..count {
        atoms.push(read_string(cursor)?);
    }
    Ok(atoms)
}

fn read_string(cursor: &mut Cursor<'_>) -> Result<JsString, PrimjsError> {
    let encoded = cursor.leb128()?;
    let length = (encoded >> 1) as usize;
    if encoded & 1 == 0 {
        Ok(JsString::Latin1(cursor.take(length)?.to_vec()))
    } else {
        let size = length.checked_mul(2).ok_or(PrimjsError::UnexpectedEof {
            offset: cursor.offset(),
            needed: usize::MAX,
            remaining: cursor.remaining(),
        })?;
        let bytes = cursor.take(size)?;
        Ok(JsString::Utf16(
            bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect(),
        ))
    }
}

struct Reader<'a, 'b> {
    cursor: Cursor<'a>,
    header: Header,
    atoms: &'b [JsString],
}

impl Reader<'_, '_> {
    fn file_atom_count(&self) -> u32 {
        u32::try_from(self.atoms.len()).unwrap_or(u32::MAX)
    }

    /// `bc_get_atom`: LEB128 of `index << 1`, or `value << 1 | 1` for an
    /// integer atom.
    fn atom(&mut self) -> Result<Atom, PrimjsError> {
        let offset = self.cursor.offset();
        let raw = self.cursor.leb128()?;
        if raw & 1 != 0 {
            return Ok(Atom::TaggedInt(raw >> 1));
        }
        let index = raw >> 1;
        if index < ATOM_END {
            return Ok(Atom::Builtin(index));
        }
        if index - ATOM_END < self.file_atom_count() {
            Ok(Atom::File(index - ATOM_END))
        } else {
            Err(PrimjsError::AtomOutOfRange { offset, index })
        }
    }

    fn count(&mut self, what: &'static str, min_bytes: usize) -> Result<u32, PrimjsError> {
        let offset = self.cursor.offset();
        let count = self.cursor.leb128()?;
        if (count as usize).saturating_mul(min_bytes) > self.cursor.remaining() {
            return Err(PrimjsError::CountOutOfRange {
                offset,
                what,
                count: u64::from(count),
            });
        }
        Ok(count)
    }

    fn u16_count(&mut self, what: &'static str) -> Result<u16, PrimjsError> {
        let offset = self.cursor.offset();
        let count = self.cursor.leb128()?;
        u16::try_from(count).map_err(|_| PrimjsError::CountOutOfRange {
            offset,
            what,
            count: u64::from(count),
        })
    }

    fn value(&mut self, depth: usize) -> Result<Value, PrimjsError> {
        let offset = self.cursor.offset();
        if depth >= NESTING_LIMIT {
            return Err(PrimjsError::NestingTooDeep {
                offset,
                limit: NESTING_LIMIT,
            });
        }
        let tag = self.cursor.u8()?;
        Ok(match tag {
            tag::NULL => Value::Null,
            tag::UNDEFINED => Value::Undefined,
            tag::BOOL_FALSE => Value::Bool(false),
            tag::BOOL_TRUE => Value::Bool(true),
            tag::INT32 => Value::Int32(self.cursor.sleb128()?),
            tag::FLOAT64 => Value::Float64(f64::from_bits(self.cursor.u64()?)),
            tag::STRING => Value::String(read_string(&mut self.cursor)?),
            tag::OBJECT => {
                let count = self.count("object property", MIN_PROPERTY_BYTES)?;
                let mut properties = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let key = self.atom()?;
                    properties.push((key, self.value(depth + 1)?));
                }
                Value::Object(properties)
            }
            tag::ARRAY | tag::TEMPLATE_OBJECT => {
                let count = self.count("array element", 1)?;
                let mut elements = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    elements.push(self.value(depth + 1)?);
                }
                if tag == tag::ARRAY {
                    Value::Array(elements)
                } else {
                    let raw = Box::new(self.value(depth + 1)?);
                    Value::TemplateObject { elements, raw }
                }
            }
            tag::BIG_INT => {
                let count = self.count("BigInt byte", 1)?;
                Value::BigInt(self.cursor.take(count as usize)?.to_vec())
            }
            tag::FUNCTION_BYTECODE => Value::Function(Box::new(self.function(offset, depth)?)),
            tag::BIG_FLOAT => return Err(PrimjsError::BigFloat { offset }),
            tag::MODULE => return Err(PrimjsError::Module { offset }),
            _ => return Err(PrimjsError::UnknownTag { offset, tag }),
        })
    }

    /// Reads a function record whose tag byte, at `offset`, has been read.
    #[expect(
        clippy::too_many_lines,
        reason = "the record's fields in serialized order, kept together for comparison with JS_ReadFunction"
    )]
    fn function(&mut self, offset: usize, depth: usize) -> Result<Function, PrimjsError> {
        if depth >= NESTING_LIMIT {
            return Err(PrimjsError::NestingTooDeep {
                offset,
                limit: NESTING_LIMIT,
            });
        }
        let flags_offset = self.cursor.offset();
        let raw_flags = self.cursor.u16()?;
        if raw_flags >> 11 != 0 {
            return Err(PrimjsError::UnknownFlags {
                offset: flags_offset,
                what: "function flags",
                bits: u32::from(raw_flags & !0x07ff),
            });
        }
        let bit = |index: u16| raw_flags & (1 << index) != 0;
        let flags = FunctionFlags {
            has_prototype: bit(0),
            has_simple_parameter_list: bit(1),
            is_derived_class_constructor: bit(2),
            need_home_object: bit(3),
            kind: match (raw_flags >> 4) & 3 {
                0 => FunctionKind::Normal,
                1 => FunctionKind::Generator,
                2 => FunctionKind::Async,
                _ => FunctionKind::AsyncGenerator,
            },
            new_target_allowed: bit(6),
            super_call_allowed: bit(7),
            super_allowed: bit(8),
            arguments_allowed: bit(9),
            has_debug: bit(10),
        };
        let mode_offset = self.cursor.offset();
        let js_mode = self.cursor.u8()?;
        if js_mode & !JsMode::KNOWN != 0 {
            return Err(PrimjsError::UnknownFlags {
                offset: mode_offset,
                what: "js_mode",
                bits: u32::from(js_mode & !JsMode::KNOWN),
            });
        }
        let name = self.atom()?;
        let arg_count = self.u16_count("argument")?;
        let var_count = self.u16_count("local")?;
        let defined_arg_count = self.u16_count("defined argument")?;
        let stack_size = self.u16_count("stack size")?;
        let closure_var_count = self.count("closure variable", MIN_CLOSURE_VAR_BYTES)?;
        let cpool_count = self.count("constant pool", 1)?;
        let bytecode_len = self.count("bytecode byte", 1)?;
        let vardef_offset = self.cursor.offset();
        let vardef_count = self.count("vardef", MIN_VARDEF_BYTES)?;
        let locals = u32::from(arg_count) + u32::from(var_count);
        if vardef_count != 0 && vardef_count != locals {
            return Err(PrimjsError::InvalidRecord {
                offset: vardef_offset,
                reason: format!(
                    "{vardef_count} vardefs for {arg_count} arguments and {var_count} locals"
                ),
            });
        }

        let mut vardefs = Vec::with_capacity(vardef_count as usize);
        for _ in 0..vardef_count {
            let name = self.atom()?;
            let level_offset = self.cursor.offset();
            let scope_level = self.cursor.leb128()?;
            if scope_level > 0x00FF_FFFF {
                return Err(PrimjsError::InvalidRecord {
                    offset: level_offset,
                    reason: format!("vardef scope level {scope_level} exceeds 24 bits"),
                });
            }
            // Written as `scope_next + 1` through a `uint32_t` and read back
            // as an `int`: PrimJS emits negative values below -1 (for
            // example -2 in sloppy-mode functions with a mapped `arguments`),
            // which arrive as 0xFFFFFFFF.
            let next = self.cursor.leb128()?.cast_signed();
            let flag_offset = self.cursor.offset();
            let var_flags = self.cursor.u8()?;
            if var_flags & 0x80 != 0 {
                return Err(PrimjsError::UnknownFlags {
                    offset: flag_offset,
                    what: "vardef flags",
                    bits: 0x80,
                });
            }
            vardefs.push(VarDef {
                name,
                scope_level,
                scope_next: next.wrapping_sub(1),
                var_kind: var_flags & 0x0f,
                is_const: var_flags & 0x10 != 0,
                is_lexical: var_flags & 0x20 != 0,
                is_captured: var_flags & 0x40 != 0,
            });
        }

        let mut closure_vars = Vec::with_capacity(closure_var_count as usize);
        for _ in 0..closure_var_count {
            let name = self.atom()?;
            let var_idx = self.cursor.leb128()?;
            let closure_flags = self.cursor.u8()?;
            closure_vars.push(ClosureVar {
                name,
                var_idx,
                is_local: closure_flags & 0x01 != 0,
                is_arg: closure_flags & 0x02 != 0,
                is_const: closure_flags & 0x04 != 0,
                is_lexical: closure_flags & 0x08 != 0,
                var_kind: closure_flags >> 4,
            });
        }

        let bytecode_offset = self.cursor.offset();
        let bytecode = self.cursor.take(bytecode_len as usize)?;
        let instructions = code::decode(
            bytecode,
            bytecode_offset,
            Limits {
                var_count: u32::from(var_count),
                arg_count: u32::from(arg_count),
                closure_var_count,
                cpool_count,
                file_atom_count: self.file_atom_count(),
            },
        )?;

        let debug = if flags.has_debug {
            Some(self.debug_info()?)
        } else {
            None
        };

        let mut cpool = Vec::with_capacity(cpool_count as usize);
        for _ in 0..cpool_count {
            let value = self.value(depth + 1)?;
            if let Value::Function(child) = &value {
                check_child_closure_vars(
                    child,
                    u32::from(arg_count),
                    u32::from(var_count),
                    closure_var_count,
                )?;
            }
            cpool.push(value);
        }

        Ok(Function {
            offset,
            flags,
            js_mode: JsMode(js_mode),
            name,
            arg_count,
            var_count,
            defined_arg_count,
            stack_size,
            vardefs,
            closure_vars,
            bytecode_offset,
            bytecode_len,
            instructions,
            debug,
            cpool,
        })
    }

    fn debug_info(&mut self) -> Result<DebugInfo, PrimjsError> {
        let filename = self.atom()?;
        let field = self.cursor.leb128()?;
        let length_offset = self.cursor.offset();
        let length = self.cursor.leb128()?;
        let table = self.cursor.take(length as usize)?;
        let external = self.header.flags().external_debug_info
            || match filename {
                Atom::File(index) => self.atoms.get(index as usize).is_some_and(|text| {
                    *text == JsString::Latin1(FUNCTION_ID_MARKER.as_bytes().to_vec())
                }),
                _ => false,
            };
        if !external {
            return Ok(DebugInfo::Inline {
                filename,
                line_num: field,
                pc2line: table.to_vec(),
            });
        }
        if length != 0 {
            return Err(PrimjsError::InvalidRecord {
                offset: length_offset,
                reason: format!("external debug info carries a {length}-byte line table"),
            });
        }
        Ok(DebugInfo::External {
            filename,
            function_id: field,
        })
    }
}

/// `JS_ValidateChildClosureVars`: each closure variable of `child` names an
/// argument, local or closure variable its parent has.
fn check_child_closure_vars(
    child: &Function,
    arg_count: u32,
    var_count: u32,
    closure_var_count: u32,
) -> Result<(), PrimjsError> {
    for (index, closure) in child.closure_vars.iter().enumerate() {
        let (kind, limit) = if !closure.is_local {
            ("closure variable", closure_var_count)
        } else if closure.is_arg {
            ("argument", arg_count)
        } else {
            ("local", var_count)
        };
        if closure.var_idx >= limit {
            return Err(PrimjsError::ClosureVarOutOfRange {
                child_offset: child.offset,
                closure_index: u32::try_from(index).unwrap_or(u32::MAX),
                kind,
                index: closure.var_idx,
                limit,
            });
        }
    }
    Ok(())
}
