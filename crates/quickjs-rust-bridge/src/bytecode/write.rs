//! [`Program`] to bytes.
//!
//! The output is what `JS_WriteObject(JS_WRITE_OBJ_BYTECODE)` writes for
//! the same program: atoms are numbered in the order the fork's writer
//! first meets them, LEB128 numbers are in their shortest form, and every
//! instruction keeps the opcode the model gives it.
//!
//! **Jumps.** The writer never changes an opcode. A jump's offset is
//! computed from the instruction positions; a short form (`goto8`,
//! `goto16`, `if_false8`, `if_true8`) whose target is out of its range is
//! an error, not widened. A writer that changes instruction sizes, as a
//! converter does, uses the 32-bit forms (`goto`, `if_false`, `if_true`)
//! and has no range to respect.

use std::collections::HashMap;

use super::error::WriteError;
use super::model::{
    ATOM_END, Atom, DebugInfo, Encoding, Export, Function, Instruction, JsString, Module, Operand,
    Program, Value, fits, layout, runtime_atom_of,
};
use super::read::NESTING_LIMIT;
use crate::bytecode_abi::{BC_VERSION, atom_int, export_type, tag};

/// Writes `program` as bytes `JS_ReadObject(JS_READ_OBJ_BYTECODE |
/// JS_READ_OBJ_VERIFY)` reads back as the same program. A file atom whose
/// text the runtime interns as a built-in or integer atom is written as
/// that atom, which is what the engine reads it as either way. The program
/// is not verified here; see [`super::Verifier`].
pub fn write_program(program: &Program) -> Result<Vec<u8>, WriteError> {
    let mut writer = Writer {
        program,
        out: Vec::new(),
        file_index: HashMap::new(),
        file_order: Vec::new(),
        depth: 0,
    };
    writer.value(&program.root)?;
    let body = std::mem::take(&mut writer.out);
    let mut out = vec![BC_VERSION];
    put_leb128(
        &mut out,
        u32::try_from(writer.file_order.len()).map_err(too_many)?,
    );
    for &index in &writer.file_order {
        put_string(&mut out, &program.atoms[index as usize])?;
    }
    out.extend_from_slice(&body);
    Ok(out)
}

fn too_many<E>(_: E) -> WriteError {
    WriteError::new("a count does not fit in 32 bits")
}

pub(crate) fn put_leb128(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_sleb128(out: &mut Vec<u8>, value: i32) {
    let value = value as u32;
    put_leb128(
        out,
        value.wrapping_mul(2) ^ (0u32.wrapping_sub(value >> 31)),
    );
}

fn put_string(out: &mut Vec<u8>, string: &JsString) -> Result<(), WriteError> {
    let len = u32::try_from(string.len())
        .ok()
        .filter(|&len| len <= crate::bytecode_abi::limits::JS_STRING_LEN_MAX)
        .ok_or_else(|| WriteError::new("string too long"))?;
    match string {
        JsString::Latin1(bytes) => {
            put_leb128(out, len << 1);
            out.extend_from_slice(bytes);
        }
        JsString::Utf16(units) => {
            put_leb128(out, (len << 1) | 1);
            for unit in units {
                out.extend_from_slice(&unit.to_le_bytes());
            }
        }
    }
    Ok(())
}

fn count(value: usize, what: &str) -> Result<u32, WriteError> {
    u32::try_from(value)
        .ok()
        .filter(|&value| i32::try_from(value).is_ok())
        .ok_or_else(|| WriteError::new(format!("{what} {value} does not fit")))
}

struct Writer<'a> {
    program: &'a Program,
    out: Vec<u8>,
    /// Model file index to output file index.
    file_index: HashMap<u32, u32>,
    /// Output file index to model file index.
    file_order: Vec<u32>,
    depth: usize,
}

/// An atom as the output encodes it.
enum Encoded {
    /// A built-in index or an output file index plus `JS_ATOM_END`.
    Index(u32),
    /// An integer atom.
    Int(u32),
}

impl Writer<'_> {
    fn encode_atom(&mut self, atom: Atom) -> Result<Encoded, WriteError> {
        match atom {
            Atom::Builtin(index) if index < ATOM_END => Ok(Encoded::Index(index)),
            Atom::Builtin(index) => Err(WriteError::new(format!(
                "built-in atom {index} past the end"
            ))),
            Atom::Int(value) if value <= atom_int::MAX_INT => Ok(Encoded::Int(value)),
            Atom::Int(value) => Err(WriteError::new(format!("integer atom {value} too large"))),
            Atom::File(index) => {
                let text = self.program.atoms.get(index as usize).ok_or_else(|| {
                    WriteError::new(format!("file atom {index} outside the table"))
                })?;
                match runtime_atom_of(text) {
                    Some(Atom::Builtin(builtin)) => return Ok(Encoded::Index(builtin)),
                    Some(Atom::Int(value)) => return Ok(Encoded::Int(value)),
                    _ => {}
                }
                if let Some(&output) = self.file_index.get(&index) {
                    return Ok(Encoded::Index(ATOM_END + output));
                }
                let output = u32::try_from(self.file_order.len()).map_err(too_many)?;
                self.file_index.insert(index, output);
                self.file_order.push(index);
                Ok(Encoded::Index(ATOM_END + output))
            }
        }
    }

    /// A record field atom (`bc_put_atom`).
    fn atom(&mut self, atom: Atom) -> Result<(), WriteError> {
        let value = match self.encode_atom(atom)? {
            Encoded::Index(index) => index << 1,
            Encoded::Int(value) => (value << 1) | 1,
        };
        put_leb128(&mut self.out, value);
        Ok(())
    }

    fn enter(&mut self) -> Result<(), WriteError> {
        self.depth += 1;
        if self.depth > NESTING_LIMIT {
            return Err(WriteError::new(format!(
                "values nested deeper than {NESTING_LIMIT}"
            )));
        }
        Ok(())
    }

    fn value(&mut self, value: &Value) -> Result<(), WriteError> {
        self.enter()?;
        let result = self.value_inner(value);
        self.depth -= 1;
        result
    }

    fn value_inner(&mut self, value: &Value) -> Result<(), WriteError> {
        match value {
            Value::Null => self.out.push(tag::NULL),
            Value::Undefined => self.out.push(tag::UNDEFINED),
            Value::Bool(false) => self.out.push(tag::BOOL_FALSE),
            Value::Bool(true) => self.out.push(tag::BOOL_TRUE),
            Value::Int32(value) => {
                self.out.push(tag::INT32);
                put_sleb128(&mut self.out, *value);
            }
            Value::Float64(value) => {
                self.out.push(tag::FLOAT64);
                self.out.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            Value::String(string) => {
                self.out.push(tag::STRING);
                put_string(&mut self.out, string)?;
            }
            Value::Object(properties) => {
                self.out.push(tag::OBJECT);
                put_leb128(&mut self.out, count(properties.len(), "property count")?);
                for (name, value) in properties {
                    self.atom(*name)?;
                    self.value(value)?;
                }
            }
            Value::Array(elements) => {
                self.out.push(tag::ARRAY);
                put_leb128(&mut self.out, count(elements.len(), "array length")?);
                for element in elements {
                    self.value(element)?;
                }
            }
            Value::TemplateObject { elements, raw } => {
                self.out.push(tag::TEMPLATE_OBJECT);
                put_leb128(&mut self.out, count(elements.len(), "template length")?);
                for element in elements {
                    self.value(element)?;
                }
                self.value(raw)?;
            }
            Value::BigInt(bytes) => {
                self.out.push(tag::BIG_INT);
                put_leb128(&mut self.out, count(bytes.len(), "BigInt length")?);
                self.out.extend_from_slice(bytes);
            }
            Value::Function(function) => self.function(function)?,
            Value::Module(module) => self.module(module)?,
        }
        Ok(())
    }

    fn module(&mut self, module: &Module) -> Result<(), WriteError> {
        self.out.push(tag::MODULE);
        self.atom(module.name)?;
        put_leb128(
            &mut self.out,
            count(module.requests.len(), "request count")?,
        );
        for request in &module.requests {
            self.atom(request.specifier)?;
            self.value(&request.attributes)?;
        }
        put_leb128(&mut self.out, count(module.exports.len(), "export count")?);
        for export in &module.exports {
            match *export {
                Export::Local { var_idx, name } => {
                    self.out.push(export_type::LOCAL);
                    put_leb128(&mut self.out, count(var_idx as usize, "export variable")?);
                    self.atom(name)?;
                }
                Export::Indirect {
                    request,
                    local_name,
                    name,
                } => {
                    self.out.push(export_type::INDIRECT);
                    put_leb128(&mut self.out, count(request as usize, "export request")?);
                    self.atom(local_name)?;
                    self.atom(name)?;
                }
            }
        }
        put_leb128(
            &mut self.out,
            count(module.star_exports.len(), "star export count")?,
        );
        for &request in &module.star_exports {
            put_leb128(
                &mut self.out,
                count(request as usize, "star export request")?,
            );
        }
        put_leb128(&mut self.out, count(module.imports.len(), "import count")?);
        for import in &module.imports {
            put_leb128(
                &mut self.out,
                count(import.var_idx as usize, "import variable")?,
            );
            self.out.push(u8::from(import.is_star));
            self.atom(import.name)?;
            put_leb128(
                &mut self.out,
                count(import.request as usize, "import request")?,
            );
        }
        self.out.push(u8::from(module.has_tla));
        self.enter()?;
        let result = self.function(&module.function);
        self.depth -= 1;
        result
    }

    fn function(&mut self, function: &Function) -> Result<(), WriteError> {
        let code = encode_code(function)?;
        self.out.push(tag::FUNCTION_BYTECODE);
        let flags = function.flags;
        let mut bits: u16 = 0;
        let mut set = |index: u16, on: bool| {
            if on {
                bits |= 1 << index;
            }
        };
        set(0, flags.has_prototype);
        set(1, flags.has_simple_parameter_list);
        set(2, flags.is_derived_class_constructor);
        set(3, flags.need_home_object);
        set(6, flags.new_target_allowed);
        set(7, flags.super_call_allowed);
        set(8, flags.super_allowed);
        set(9, flags.arguments_allowed);
        set(10, function.debug.is_some());
        set(11, flags.is_direct_or_indirect_eval);
        bits |= u16::from(flags.kind.bits()) << 4;
        self.out.extend_from_slice(&bits.to_le_bytes());
        self.out.push(function.js_mode);
        self.atom(function.name)?;
        put_leb128(&mut self.out, u32::from(function.arg_count));
        put_leb128(&mut self.out, u32::from(function.var_count));
        put_leb128(&mut self.out, u32::from(function.defined_arg_count));
        put_leb128(&mut self.out, u32::from(function.stack_size));
        put_leb128(&mut self.out, u32::from(function.var_ref_count));
        put_leb128(
            &mut self.out,
            count(function.closure_vars.len(), "closure_var_count")?,
        );
        put_leb128(&mut self.out, count(function.cpool.len(), "cpool_count")?);
        put_leb128(&mut self.out, count(code.bytes.len(), "byte_code_len")?);
        if !function.vardefs.is_empty()
            && function.vardefs.len()
                != usize::from(function.arg_count) + usize::from(function.var_count)
        {
            return Err(WriteError::new(format!(
                "{} vardefs for {} arguments and {} locals",
                function.vardefs.len(),
                function.arg_count,
                function.var_count
            )));
        }
        put_leb128(
            &mut self.out,
            count(function.vardefs.len(), "vardef count")?,
        );
        for vardef in &function.vardefs {
            self.atom(vardef.name)?;
            put_leb128(&mut self.out, vardef.scope_next.wrapping_add(1) as u32);
            put_leb128(&mut self.out, u32::from(vardef.var_ref_idx));
            if vardef.var_kind > 0xf {
                return Err(WriteError::new("vardef var_kind above four bits"));
            }
            let mut bits = vardef.var_kind;
            bits |= u8::from(vardef.is_const) << 4;
            bits |= u8::from(vardef.is_lexical) << 5;
            bits |= u8::from(vardef.is_captured) << 6;
            bits |= u8::from(vardef.has_scope) << 7;
            self.out.push(bits);
        }
        for closure_var in &function.closure_vars {
            self.atom(closure_var.name)?;
            put_leb128(&mut self.out, u32::from(closure_var.var_idx));
            if closure_var.var_kind > 0xf {
                return Err(WriteError::new("closure variable var_kind above four bits"));
            }
            let bits = u16::from(closure_var.closure_type.bits())
                | (u16::from(closure_var.is_const) << 3)
                | (u16::from(closure_var.is_lexical) << 4)
                | (u16::from(closure_var.var_kind) << 5);
            self.out.extend_from_slice(&bits.to_le_bytes());
        }
        // Atom operands are numbered in instruction order, as the fork's
        // writer numbers them.
        let mut bytes = code.bytes;
        for &(at, atom) in &code.atoms {
            let value = match self.encode_atom(atom)? {
                Encoded::Index(index) => index,
                Encoded::Int(value) => value | atom_int::TAG_INT,
            };
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        self.out.extend_from_slice(&bytes);
        if let Some(DebugInfo {
            filename,
            pc2line,
            source,
        }) = &function.debug
        {
            self.atom(*filename)?;
            put_leb128(&mut self.out, count(pc2line.len(), "pc2line length")?);
            self.out.extend_from_slice(pc2line);
            put_leb128(&mut self.out, count(source.len(), "source length")?);
            self.out.extend_from_slice(source);
        }
        for entry in &function.cpool {
            self.value(entry)?;
        }
        Ok(())
    }
}

/// A function's bytecode with its atom operands left to fill.
struct Code {
    bytes: Vec<u8>,
    /// Offset of each atom operand and the atom.
    atoms: Vec<(usize, Atom)>,
}

/// Instruction positions in bytes.
pub(crate) fn positions(instructions: &[Instruction]) -> Vec<usize> {
    let mut positions = Vec::with_capacity(instructions.len() + 1);
    let mut pc = 0usize;
    for instruction in instructions {
        positions.push(pc);
        pc += usize::from(instruction.size());
    }
    positions.push(pc);
    positions
}

fn encode_code(function: &Function) -> Result<Code, WriteError> {
    let instructions = &function.instructions;
    if instructions.is_empty() {
        return Err(WriteError::new("a function needs at least one instruction"));
    }
    let positions = positions(instructions);
    let mut bytes = Vec::with_capacity(positions[instructions.len()]);
    let mut atoms = Vec::new();
    for (index, instruction) in instructions.iter().enumerate() {
        let shape = layout(instruction.opcode());
        if shape.len() != instruction.operands().len() {
            return Err(WriteError::new(format!(
                "instruction {index}: operands do not match {}",
                instruction.info().name
            )));
        }
        bytes.push(instruction.opcode());
        for (&(kind, encoding), &operand) in shape.iter().zip(instruction.operands()) {
            if operand.kind() != kind {
                return Err(WriteError::new(format!(
                    "instruction {index}: operand {operand:?} where a {kind:?} goes"
                )));
            }
            let at = bytes.len();
            match (operand, encoding) {
                (Operand::Target(target), Encoding::Label(width)) => {
                    let to = positions
                        .get(target as usize)
                        .filter(|_| (target as usize) < instructions.len())
                        .ok_or_else(|| {
                            WriteError::new(format!(
                                "instruction {index}: jump to instruction {target}, past the end"
                            ))
                        })?;
                    let offset = *to as i64 - at as i64;
                    let out_of_range = || {
                        WriteError::new(format!(
                            "instruction {index}: {} cannot reach instruction {target} \
                             ({offset} bytes)",
                            instruction.info().name
                        ))
                    };
                    match width {
                        1 => bytes.push(i8::try_from(offset).map_err(|_| out_of_range())? as u8),
                        2 => bytes.extend_from_slice(
                            &i16::try_from(offset)
                                .map_err(|_| out_of_range())?
                                .to_le_bytes(),
                        ),
                        _ => bytes.extend_from_slice(
                            &i32::try_from(offset)
                                .map_err(|_| out_of_range())?
                                .to_le_bytes(),
                        ),
                    }
                }
                (Operand::Atom(atom), _) => {
                    atoms.push((at, atom));
                    bytes.extend_from_slice(&[0; 4]);
                }
                (_, Encoding::Implicit(_)) if fits(operand, encoding) => {}
                _ if !fits(operand, encoding) => {
                    return Err(WriteError::new(format!(
                        "instruction {index}: operand {operand:?} does not fit its encoding"
                    )));
                }
                (operand, encoding) => {
                    let value: i64 = match operand {
                        Operand::Local(n) | Operand::Arg(n) | Operand::ClosureVar(n) => {
                            i64::from(n)
                        }
                        Operand::Const(n) => i64::from(n),
                        Operand::Immediate(n) => i64::from(n),
                        Operand::Atom(_) | Operand::Target(_) => 0,
                    };
                    match encoding {
                        Encoding::U8 | Encoding::I8 => bytes.push(value as u8),
                        Encoding::U16 | Encoding::I16 => {
                            bytes.extend_from_slice(&(value as u16).to_le_bytes());
                        }
                        _ => bytes.extend_from_slice(&(value as u32).to_le_bytes()),
                    }
                }
            }
        }
    }
    Ok(Code { bytes, atoms })
}
