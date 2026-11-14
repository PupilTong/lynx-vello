//! Instruction decoding and the per-instruction checks of
//! `JS_ReadFunctionBytecode`, plus atom and jump-target checks `PrimJS` lacks.

use super::PrimjsError;
use super::model::{Atom, Instruction, Operand};
use super::tables::{ATOM_END, ATOM_TAG_INT, Format, NON_SHIPPABLE_INC_COVERAGE, Opcode};

/// The counts operands are checked against.
#[derive(Debug, Clone, Copy)]
#[expect(
    clippy::struct_field_names,
    reason = "the names of the PrimJS record fields they come from"
)]
pub(super) struct Limits {
    pub(super) var_count: u32,
    pub(super) arg_count: u32,
    pub(super) closure_var_count: u32,
    pub(super) cpool_count: u32,
    pub(super) file_atom_count: u32,
}

/// Converts a serialized atom index (instruction form: raw index, tagged
/// integer with bit 31) to an [`Atom`].
pub(super) fn atom_from_operand(
    raw: u32,
    file_atom_count: u32,
    offset: usize,
) -> Result<Atom, PrimjsError> {
    if raw & ATOM_TAG_INT != 0 {
        return Ok(Atom::TaggedInt(raw & !ATOM_TAG_INT));
    }
    if raw < ATOM_END {
        return Ok(Atom::Builtin(raw));
    }
    let index = raw - ATOM_END;
    if index < file_atom_count {
        Ok(Atom::File(index))
    } else {
        Err(PrimjsError::AtomOutOfRange { offset, index: raw })
    }
}

/// Decodes `code`, whose first byte is at input offset `base`.
pub(super) fn decode(
    code: &[u8],
    base: usize,
    limits: Limits,
) -> Result<Vec<Instruction>, PrimjsError> {
    // One entry per bytecode byte: the index of the instruction starting
    // there, or `u32::MAX`.
    let mut starts = vec![u32::MAX; code.len()];
    let mut instructions = Vec::new();
    let mut pc = 0usize;
    while pc < code.len() {
        let offset = base + pc;
        let pc32 = u32::try_from(pc).map_err(|_| PrimjsError::CountOutOfRange {
            offset,
            what: "bytecode length",
            count: code.len() as u64,
        })?;
        let byte = code[pc];
        let opcode = match Opcode::from_u8(byte) {
            Some(opcode) if opcode != Opcode::Invalid => opcode,
            _ => {
                debug_assert!(byte == 0 || byte >= NON_SHIPPABLE_INC_COVERAGE);
                return Err(PrimjsError::InvalidOpcode {
                    offset,
                    pc: pc32,
                    opcode: byte,
                });
            }
        };
        let size = usize::from(opcode.size());
        let Some(bytes) = code.get(pc..pc + size) else {
            return Err(PrimjsError::TruncatedInstruction {
                offset,
                pc: pc32,
                opcode: opcode.name(),
            });
        };
        let instruction = decode_one(opcode, bytes, pc32, offset, code.len(), limits)?;
        starts[pc] = u32::try_from(instructions.len()).unwrap_or(u32::MAX);
        instructions.push(instruction);
        pc += size;
    }

    // Jump targets were stored as pcs; resolve them to instruction indices.
    for instruction in &mut instructions {
        let pc = instruction.pc;
        let name = instruction.opcode.name();
        for operand in instruction.operands_mut() {
            if let Operand::Target(target_pc) = *operand {
                let index = starts
                    .get(target_pc as usize)
                    .copied()
                    .filter(|index| *index != u32::MAX)
                    .ok_or(PrimjsError::BadJumpTarget {
                        offset: base + pc as usize,
                        pc,
                        opcode: name,
                        target: i64::from(target_pc),
                    })?;
                *operand = Operand::Target(index);
            }
        }
    }
    Ok(instructions)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per operand format, kept together for comparison with PrimJS"
)]
fn decode_one(
    opcode: Opcode,
    bytes: &[u8],
    pc: u32,
    offset: usize,
    code_len: usize,
    limits: Limits,
) -> Result<Instruction, PrimjsError> {
    let check = |kind: &'static str, index: u32, limit: u32| {
        if index < limit {
            Ok(())
        } else {
            Err(PrimjsError::IndexOutOfRange {
                offset,
                pc,
                opcode: opcode.name(),
                kind,
                index,
                limit,
            })
        }
    };
    let local = |index: u32| -> Result<Operand, PrimjsError> {
        check("local", index, limits.var_count)?;
        Ok(Operand::Local(u16::try_from(index).unwrap_or(u16::MAX)))
    };
    let arg = |index: u32| -> Result<Operand, PrimjsError> {
        check("argument", index, limits.arg_count)?;
        Ok(Operand::Arg(u16::try_from(index).unwrap_or(u16::MAX)))
    };
    let closure = |index: u32| -> Result<Operand, PrimjsError> {
        check("closure variable", index, limits.closure_var_count)?;
        Ok(Operand::ClosureVar(
            u16::try_from(index).unwrap_or(u16::MAX),
        ))
    };
    let constant = |index: u32| -> Result<Operand, PrimjsError> {
        check("constant pool", index, limits.cpool_count)?;
        Ok(Operand::Const(index))
    };
    // A jump offset is relative to `pc + base`. The target is kept as a pc
    // here and resolved once every instruction start is known.
    let target = |base: u32, delta: i32| -> Result<Operand, PrimjsError> {
        let target = i64::from(pc) + i64::from(base) + i64::from(delta);
        match u32::try_from(target) {
            Ok(target_pc) if (target_pc as usize) < code_len => Ok(Operand::Target(target_pc)),
            _ => Err(PrimjsError::BadJumpTarget {
                offset,
                pc,
                opcode: opcode.name(),
                target,
            }),
        }
    };
    let atom = |at: usize| atom_from_operand(u32_at(bytes, at), limits.file_atom_count, offset);
    let relative = |first: Opcode| u32::from((opcode.number() - first.number()) % 4);
    let immediate = |value: i32| Operand::Immediate(value);

    let mut operands = [Operand::Immediate(0); 3];
    let count = match opcode.format() {
        Format::None => 0,
        Format::NoneInt => {
            operands[0] = immediate(i32::from(opcode.number()) - i32::from(Opcode::Push0.number()));
            1
        }
        Format::NoneLoc => {
            operands[0] = local(relative(Opcode::GetLoc0))?;
            1
        }
        Format::NoneArg => {
            operands[0] = arg(relative(Opcode::GetArg0))?;
            1
        }
        Format::NoneVarRef => {
            operands[0] = closure(relative(Opcode::GetVarRef0))?;
            1
        }
        Format::U8 => {
            operands[0] = immediate(i32::from(bytes[1]));
            1
        }
        Format::I8 => {
            operands[0] = immediate(i32::from(bytes[1].cast_signed()));
            1
        }
        Format::Loc8 => {
            operands[0] = local(u32::from(bytes[1]))?;
            1
        }
        Format::Const8 => {
            operands[0] = constant(u32::from(bytes[1]))?;
            1
        }
        Format::Label8 => {
            operands[0] = target(1, i32::from(bytes[1].cast_signed()))?;
            1
        }
        Format::U16 | Format::Npop => {
            operands[0] = immediate(i32::from(u16_at(bytes, 1)));
            1
        }
        Format::I16 => {
            operands[0] = immediate(i32::from(u16_at(bytes, 1).cast_signed()));
            1
        }
        Format::Label16 => {
            operands[0] = target(1, i32::from(u16_at(bytes, 1).cast_signed()))?;
            1
        }
        Format::Npopx => {
            operands[0] = immediate(i32::from(opcode.number() - Opcode::Call0.number()));
            1
        }
        Format::Loc => {
            operands[0] = local(u32::from(u16_at(bytes, 1)))?;
            1
        }
        Format::Arg => {
            operands[0] = arg(u32::from(u16_at(bytes, 1)))?;
            1
        }
        Format::VarRef => {
            operands[0] = closure(u32::from(u16_at(bytes, 1)))?;
            1
        }
        Format::I32 => {
            operands[0] = immediate(u32_at(bytes, 1).cast_signed());
            1
        }
        Format::Const => {
            operands[0] = constant(u32_at(bytes, 1))?;
            1
        }
        Format::Label => {
            operands[0] = target(1, u32_at(bytes, 1).cast_signed())?;
            1
        }
        Format::Atom => {
            operands[0] = Operand::Atom(atom(1)?);
            1
        }
        Format::AtomU8 => {
            operands[0] = Operand::Atom(atom(1)?);
            operands[1] = immediate(i32::from(bytes[5]));
            2
        }
        Format::AtomU16 => {
            operands[0] = Operand::Atom(atom(1)?);
            let value = u32::from(u16_at(bytes, 5));
            operands[1] = match opcode {
                Opcode::MakeLocRef => local(value)?,
                Opcode::MakeArgRef => arg(value)?,
                Opcode::MakeVarRefRef => closure(value)?,
                _ => immediate(i32::from(u16_at(bytes, 5))),
            };
            2
        }
        Format::AtomLabelU8 => {
            operands[0] = Operand::Atom(atom(1)?);
            operands[1] = target(5, u32_at(bytes, 5).cast_signed())?;
            operands[2] = immediate(i32::from(bytes[9]));
            3
        }
    };
    Ok(Instruction::new(pc, opcode, &operands[..count]))
}
