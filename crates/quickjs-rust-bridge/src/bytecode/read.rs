//! Bytes to [`Program`].

use super::error::ReadError;
use super::model::{
    ATOM_END, Atom, ClosureType, ClosureVar, DebugInfo, Encoding, Export, Function, FunctionFlags,
    FunctionKind, Import, Instruction, JsString, Module, ModuleRequest, Operand, OperandKind,
    Program, Value, VarDef, layout,
};
use crate::bytecode_abi::{BC_VERSION, OP_COUNT, OPCODES, atom_int, export_type, limits, op, tag};

/// Levels of nested values and functions the reader follows before it
/// refuses the input. The verifier and the writer recurse no deeper.
pub const NESTING_LIMIT: usize = 256;

/// Reads a serialized program in the format `JS_ReadObject(JS_READ_OBJ_BYTECODE)`
/// reads, from any writer. This is the reader for untrusted bytes: the
/// fork's own reader trusts the structure of its input and is handed only
/// what [`super::write_program`] wrote for a checked program. This one does
/// not panic on any input, recursion is bounded by [`NESTING_LIMIT`], and
/// every count is checked against the bytes that remain before anything is
/// allocated for it.
///
/// It refuses a count or length the input cannot hold, a count the engine
/// stores in an `int` above `i32::MAX`, a field it stores in 16 bits above
/// 16 bits, a
/// function with no bytecode or with vardefs other than none or
/// `arg_count + var_count`, an opcode outside the final bytecode, an
/// instruction that runs past its function's bytecode, an atom index
/// outside the atom table, a value tag compiled code does not contain, a
/// function or a module where the fork's writer never puts one, and an
/// export type other than local and indirect; and, beyond what the format
/// needs, a LEB128 number in more bytes than it needs, a boolean byte other
/// than 0 or 1, flag bits the format does not define, a closure variable
/// index above 16 bits (the engine stores it in 16 bits and would truncate
/// it), a jump to a byte that does not start an instruction, and bytes after
/// the top-level value. What it returns therefore writes back to bytes the
/// engine reads as the same program.
pub fn read_program(bytes: &[u8]) -> Result<Program, ReadError> {
    let mut reader = Reader {
        bytes,
        position: 0,
        atom_count: 0,
        depth: 0,
    };
    let version = reader.u8()?;
    if version != BC_VERSION {
        return Err(reader.error_at(0, format!("version {version}, expected {BC_VERSION}")));
    }
    let count = reader.leb128()?;
    reader.check_count(u64::from(count), 1, "atom")?;
    let mut atoms = Vec::with_capacity(count as usize);
    for _ in 0..count {
        atoms.push(reader.string()?);
    }
    reader.atom_count = count;
    let root = reader.value(Allowed::Any)?;
    if reader.position != bytes.len() {
        return Err(reader.error(format!(
            "{} bytes follow the top-level value",
            bytes.len() - reader.position
        )));
    }
    Ok(Program { atoms, root })
}

/// What [`Reader::value`] may read here (`code_allowed` of the C reader).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Allowed {
    /// Data only.
    Data,
    /// Data or function bytecode.
    Function,
    /// Anything, a module included.
    Any,
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    atom_count: u32,
    depth: usize,
}

impl Reader<'_> {
    fn error(&self, message: impl Into<String>) -> ReadError {
        self.error_at(self.position, message)
    }

    #[allow(clippy::unused_self, reason = "kept beside `error` for symmetry")]
    fn error_at(&self, offset: usize, message: impl Into<String>) -> ReadError {
        ReadError {
            offset,
            message: message.into(),
        }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, count: usize) -> Result<&[u8], ReadError> {
        if count > self.remaining() {
            return Err(self.error("read after the end of the input"));
        }
        let slice = &self.bytes[self.position..self.position + count];
        self.position += count;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ReadError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u64(&mut self) -> Result<u64, ReadError> {
        let bytes = self.take(8)?;
        let mut array = [0; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    fn bool(&mut self) -> Result<bool, ReadError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(self.error(format!("boolean byte {other}"))),
        }
    }

    /// An unsigned LEB128 number of at most 32 bits in its shortest
    /// encoding.
    fn leb128(&mut self) -> Result<u32, ReadError> {
        let start = self.position;
        let mut value: u64 = 0;
        for index in 0..5 {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                if index > 0 && byte == 0 {
                    return Err(self.error_at(start, "LEB128 number not in its shortest form"));
                }
                return u32::try_from(value)
                    .map_err(|_| self.error_at(start, "LEB128 number above 32 bits"));
            }
        }
        Err(self.error_at(start, "LEB128 number longer than five bytes"))
    }

    fn sleb128(&mut self) -> Result<i32, ReadError> {
        let value = self.leb128()?;
        Ok(((value >> 1) as i32) ^ -((value & 1) as i32))
    }

    fn u16_field(&mut self, what: &str) -> Result<u16, ReadError> {
        let start = self.position;
        let value = self.leb128()?;
        u16::try_from(value)
            .map_err(|_| self.error_at(start, format!("{what} {value} above 16 bits")))
    }

    /// A count the reader stores in an `int`.
    fn int_field(&mut self, what: &str) -> Result<u32, ReadError> {
        let start = self.position;
        let value = self.leb128()?;
        if value > i32::MAX as u32 {
            return Err(self.error_at(start, format!("{what} {value} is negative as an int")));
        }
        Ok(value)
    }

    /// Fails unless `count` items of at least `item_size` bytes each can
    /// still follow.
    fn check_count(&self, count: u64, item_size: u64, what: &str) -> Result<(), ReadError> {
        if count.saturating_mul(item_size) > self.remaining() as u64 {
            return Err(self.error(format!("{count} {what} entries cannot fit in the input")));
        }
        Ok(())
    }

    fn string(&mut self) -> Result<JsString, ReadError> {
        let header = self.leb128()?;
        let wide = header & 1 != 0;
        let len = header >> 1;
        if len > limits::JS_STRING_LEN_MAX {
            return Err(self.error("string too long"));
        }
        let len = len as usize;
        if wide {
            let bytes = self.take(len * 2)?;
            Ok(JsString::Utf16(
                bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect(),
            ))
        } else {
            Ok(JsString::Latin1(self.take(len)?.to_vec()))
        }
    }

    fn atom_index(&self, start: usize, index: u32) -> Result<Atom, ReadError> {
        if index < ATOM_END {
            Ok(Atom::Builtin(index))
        } else if index - ATOM_END < self.atom_count {
            Ok(Atom::File(index - ATOM_END))
        } else {
            Err(self.error_at(start, format!("atom index {index} outside the atom table")))
        }
    }

    /// A record field atom (`bc_get_atom`).
    fn atom(&mut self) -> Result<Atom, ReadError> {
        let start = self.position;
        let value = self.leb128()?;
        if value & 1 != 0 {
            Ok(Atom::Int(value >> 1))
        } else {
            self.atom_index(start, value >> 1)
        }
    }

    /// An instruction operand atom (a raw `u32`).
    fn operand_atom(&self, start: usize, value: u32) -> Result<Atom, ReadError> {
        if value & atom_int::TAG_INT != 0 {
            Ok(Atom::Int(value & atom_int::MAX_INT))
        } else {
            self.atom_index(start, value)
        }
    }

    fn enter(&mut self) -> Result<(), ReadError> {
        self.depth += 1;
        if self.depth > NESTING_LIMIT {
            return Err(self.error(format!("values nested deeper than {NESTING_LIMIT}")));
        }
        Ok(())
    }

    fn value(&mut self, allowed: Allowed) -> Result<Value, ReadError> {
        self.enter()?;
        let value = self.value_inner(allowed);
        self.depth -= 1;
        value
    }

    fn value_inner(&mut self, allowed: Allowed) -> Result<Value, ReadError> {
        let start = self.position;
        let tag_byte = self.u8()?;
        Ok(match tag_byte {
            tag::NULL => Value::Null,
            tag::UNDEFINED => Value::Undefined,
            tag::BOOL_FALSE => Value::Bool(false),
            tag::BOOL_TRUE => Value::Bool(true),
            tag::INT32 => Value::Int32(self.sleb128()?),
            tag::FLOAT64 => Value::Float64(f64::from_bits(self.u64()?)),
            tag::STRING => Value::String(self.string()?),
            tag::OBJECT => {
                let count = self.leb128()?;
                self.check_count(u64::from(count), 2, "property")?;
                let mut properties = Vec::new();
                for _ in 0..count {
                    let name = self.atom()?;
                    properties.push((name, self.value(Allowed::Data)?));
                }
                Value::Object(properties)
            }
            tag::ARRAY | tag::TEMPLATE_OBJECT => {
                let count = self.leb128()?;
                self.check_count(u64::from(count), 1, "element")?;
                let mut elements = Vec::new();
                for _ in 0..count {
                    elements.push(self.value(Allowed::Data)?);
                }
                if tag_byte == tag::ARRAY {
                    Value::Array(elements)
                } else {
                    Value::TemplateObject {
                        elements,
                        raw: Box::new(self.value(Allowed::Data)?),
                    }
                }
            }
            tag::BIG_INT => {
                let len = self.leb128()?;
                Value::BigInt(self.take(len as usize)?.to_vec())
            }
            tag::FUNCTION_BYTECODE if allowed != Allowed::Data => {
                Value::Function(Box::new(self.function()?))
            }
            tag::MODULE if allowed == Allowed::Any => Value::Module(Box::new(self.module()?)),
            other => {
                return Err(self.error_at(start, format!("value tag {other} not allowed here")));
            }
        })
    }

    fn module(&mut self) -> Result<Module, ReadError> {
        let name = self.atom()?;
        let count = self.int_field("request count")?;
        self.check_count(u64::from(count), 2, "request")?;
        let mut requests = Vec::new();
        for _ in 0..count {
            let specifier = self.atom()?;
            let attributes = self.value(Allowed::Data)?;
            requests.push(ModuleRequest {
                specifier,
                attributes,
            });
        }
        let count = self.int_field("export count")?;
        self.check_count(u64::from(count), 3, "export")?;
        let mut exports = Vec::new();
        for _ in 0..count {
            let start = self.position;
            let kind = self.u8()?;
            exports.push(match kind {
                export_type::LOCAL => {
                    let var_idx = self.int_field("export variable")?;
                    Export::Local {
                        var_idx,
                        name: self.atom()?,
                    }
                }
                export_type::INDIRECT => {
                    let request = self.int_field("export request")?;
                    let local_name = self.atom()?;
                    Export::Indirect {
                        request,
                        local_name,
                        name: self.atom()?,
                    }
                }
                other => return Err(self.error_at(start, format!("export type {other}"))),
            });
        }
        let count = self.int_field("star export count")?;
        self.check_count(u64::from(count), 1, "star export")?;
        let mut star_exports = Vec::new();
        for _ in 0..count {
            star_exports.push(self.int_field("star export request")?);
        }
        let count = self.int_field("import count")?;
        self.check_count(u64::from(count), 4, "import")?;
        let mut imports = Vec::new();
        for _ in 0..count {
            let var_idx = self.int_field("import variable")?;
            let is_star = self.bool()?;
            let name = self.atom()?;
            let request = self.int_field("import request")?;
            imports.push(Import {
                var_idx,
                is_star,
                name,
                request,
            });
        }
        let has_tla = self.bool()?;
        let start = self.position;
        let Value::Function(function) = self.value(Allowed::Function)? else {
            return Err(self.error_at(start, "a module's function is not function bytecode"));
        };
        Ok(Module {
            name,
            requests,
            exports,
            star_exports,
            imports,
            has_tla,
            function: *function,
        })
    }

    #[allow(clippy::too_many_lines, reason = "one record, field by field")]
    fn function(&mut self) -> Result<Function, ReadError> {
        let start = self.position;
        let bits = self.u16()?;
        if bits >> 12 != 0 {
            return Err(self.error_at(start, format!("function flag bits {bits:#06x}")));
        }
        let bit = |index: u16| bits & (1 << index) != 0;
        let flags = FunctionFlags {
            has_prototype: bit(0),
            has_simple_parameter_list: bit(1),
            is_derived_class_constructor: bit(2),
            need_home_object: bit(3),
            kind: FunctionKind::from_bits(((bits >> 4) & 3) as u8),
            new_target_allowed: bit(6),
            super_call_allowed: bit(7),
            super_allowed: bit(8),
            arguments_allowed: bit(9),
            is_direct_or_indirect_eval: bit(11),
        };
        let has_debug = bit(10);
        let js_mode = self.u8()?;
        let name = self.atom()?;
        let arg_count = self.u16_field("arg_count")?;
        let var_count = self.u16_field("var_count")?;
        let defined_arg_count = self.u16_field("defined_arg_count")?;
        let stack_size = self.u16_field("stack_size")?;
        let var_ref_count = self.u16_field("var_ref_count")?;
        let closure_var_count = self.int_field("closure_var_count")?;
        let cpool_count = self.int_field("cpool_count")?;
        let byte_code_len = self.int_field("byte_code_len")?;
        let local_count_at = self.position;
        let local_count = self.int_field("vardef count")?;
        if byte_code_len == 0 {
            return Err(self.error_at(local_count_at, "empty bytecode"));
        }
        if local_count != 0 && local_count != u32::from(arg_count) + u32::from(var_count) {
            return Err(self.error_at(
                local_count_at,
                format!("{local_count} vardefs for {arg_count} arguments and {var_count} locals"),
            ));
        }
        self.check_count(
            u64::from(local_count) * 4
                + u64::from(closure_var_count) * 4
                + u64::from(cpool_count)
                + u64::from(byte_code_len),
            1,
            "record byte",
        )?;
        let mut vardefs = Vec::with_capacity(local_count as usize);
        for _ in 0..local_count {
            let name = self.atom()?;
            let scope_next = (self.leb128()? as i32).wrapping_sub(1);
            let var_ref_idx = self.u16_field("var_ref_idx")?;
            let bits = self.u8()?;
            vardefs.push(VarDef {
                name,
                scope_next,
                var_ref_idx,
                var_kind: bits & 0xf,
                is_const: bits & 0x10 != 0,
                is_lexical: bits & 0x20 != 0,
                is_captured: bits & 0x40 != 0,
                has_scope: bits & 0x80 != 0,
            });
        }
        let mut closure_vars = Vec::with_capacity(closure_var_count as usize);
        for _ in 0..closure_var_count {
            let name = self.atom()?;
            let var_idx = self.u16_field("closure variable index")?;
            let at = self.position;
            let bits = self.u16()?;
            if bits >> 9 != 0 {
                return Err(self.error_at(at, format!("closure variable flag bits {bits:#06x}")));
            }
            closure_vars.push(ClosureVar {
                name,
                var_idx,
                closure_type: ClosureType::from_bits((bits & 7) as u8),
                is_const: bits & 8 != 0,
                is_lexical: bits & 0x10 != 0,
                var_kind: ((bits >> 5) & 0xf) as u8,
            });
        }
        let code_start = self.position;
        let code = self.take(byte_code_len as usize)?.to_vec();
        let instructions = self.instructions(code_start, &code)?;
        let debug = if has_debug {
            let filename = self.atom()?;
            let len = self.int_field("pc2line length")?;
            let pc2line = self.take(len as usize)?.to_vec();
            let len = self.int_field("source length")?;
            let source = self.take(len as usize)?.to_vec();
            Some(DebugInfo {
                filename,
                pc2line,
                source,
            })
        } else {
            None
        };
        let mut cpool = Vec::with_capacity(cpool_count as usize);
        for _ in 0..cpool_count {
            cpool.push(self.value(Allowed::Function)?);
        }
        Ok(Function {
            flags,
            js_mode,
            name,
            arg_count,
            var_count,
            defined_arg_count,
            stack_size,
            var_ref_count,
            vardefs,
            closure_vars,
            instructions,
            debug,
            cpool,
        })
    }

    /// Decodes one function's bytecode. Jump offsets become instruction
    /// indices; a jump to a byte that does not start an instruction is an
    /// error.
    fn instructions(&self, base: usize, code: &[u8]) -> Result<Vec<Instruction>, ReadError> {
        // First pass: instruction starts.
        let mut starts = Vec::new();
        let mut pc = 0usize;
        while pc < code.len() {
            let opcode = code[pc];
            if opcode == op::INVALID || usize::from(opcode) >= OP_COUNT {
                return Err(self.error_at(base + pc, format!("invalid opcode {opcode}")));
            }
            let size = usize::from(OPCODES[usize::from(opcode)].size);
            if size > code.len() - pc {
                return Err(self.error_at(base + pc, "truncated instruction"));
            }
            starts.push(pc);
            pc += size;
        }
        let mut instructions = Vec::with_capacity(starts.len());
        for &pc in &starts {
            let opcode = code[pc];
            let mut operands = [Operand::Immediate(0); 3];
            let shape = layout(opcode);
            let mut at = pc + 1;
            for (slot, &(kind, encoding)) in shape.iter().enumerate() {
                let (raw, width): (i64, usize) = match encoding {
                    Encoding::Implicit(value) => (i64::from(value), 0),
                    Encoding::U8 => (i64::from(code[at]), 1),
                    Encoding::I8 => (i64::from(code[at] as i8), 1),
                    Encoding::U16 => (i64::from(u16::from_le_bytes([code[at], code[at + 1]])), 2),
                    Encoding::I16 => (i64::from(i16::from_le_bytes([code[at], code[at + 1]])), 2),
                    Encoding::U32 | Encoding::I32 => {
                        let mut bytes = [0; 4];
                        bytes.copy_from_slice(&code[at..at + 4]);
                        let value = u32::from_le_bytes(bytes);
                        let value = if kind == OperandKind::Atom || kind == OperandKind::Const {
                            i64::from(value)
                        } else {
                            i64::from(value as i32)
                        };
                        (value, 4)
                    }
                    Encoding::Label(bytes) => {
                        let width = usize::from(bytes);
                        let offset = match bytes {
                            1 => i64::from(code[at] as i8),
                            2 => i64::from(i16::from_le_bytes([code[at], code[at + 1]])),
                            _ => {
                                let mut array = [0; 4];
                                array.copy_from_slice(&code[at..at + 4]);
                                i64::from(i32::from_le_bytes(array))
                            }
                        };
                        (at as i64 + offset, width)
                    }
                };
                operands[slot] = match kind {
                    OperandKind::Atom => Operand::Atom(self.operand_atom(base + at, raw as u32)?),
                    OperandKind::Local => Operand::Local(raw as u16),
                    OperandKind::Arg => Operand::Arg(raw as u16),
                    OperandKind::ClosureVar => Operand::ClosureVar(raw as u16),
                    OperandKind::Const => Operand::Const(raw as u32),
                    OperandKind::Immediate => Operand::Immediate(raw as i32),
                    OperandKind::Target => {
                        let index = usize::try_from(raw)
                            .ok()
                            .and_then(|target| starts.binary_search(&target).ok())
                            .ok_or_else(|| {
                                self.error_at(
                                    base + pc,
                                    format!("jump to byte {raw}, not an instruction start"),
                                )
                            })?;
                        Operand::Target(index as u32)
                    }
                };
                at += width;
            }
            let instruction = Instruction::new(opcode, &operands[..shape.len()])
                .map_err(|error| self.error_at(base + pc, error.message))?;
            instructions.push(instruction);
        }
        Ok(instructions)
    }
}
