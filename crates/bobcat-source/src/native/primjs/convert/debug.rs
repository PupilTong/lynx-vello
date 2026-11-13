//! Line tables: `PrimJS`'s pc2line to the fork's.
//!
//! `PrimJS` (`compute_pc2line_info`, `find_line_num` in
//! `source/quickjs.cc`) starts from the record's `line_num` and stores, per
//! change, a pc delta and a delta of a packed 64-bit position: bits 62..63
//! are a type, type 1 holds the line in bits 0..23 and the column above
//! them, type 0 holds the line in bits 0..11 and the column above them.
//! The fork (`compute_pc2line_info`, `find_line_num` in `quickjs.c`) opens
//! with the function's line and column, each minus one, as LEB128, and
//! stores per change a pc delta, a line delta and a column delta.
//!
//! Both tables are read the same way: an entry at pc `p` gives the position
//! of every pc from `p` until the next entry.

use quickjs_rust_bridge::bytecode_abi::pc2line;

/// A source position: line and column as `PrimJS` stores them, both
/// counted from 1 (`compute_column` adds one to the byte offset in the
/// line); `0` where `PrimJS` had no position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Position {
    pub line: i64,
    pub column: i64,
}

/// Unpacks a `PrimJS` position word; `None` for a type other than 0 and 1
/// (which `PrimJS` prints as `-1:-1`).
pub(super) fn unpack(word: i64) -> Option<Position> {
    let bits = word.cast_unsigned();
    match bits >> 62 {
        1 => Some(Position {
            line: (bits & ((1 << 24) - 1)).cast_signed(),
            column: ((bits ^ (1 << 62)) >> 24).cast_signed(),
        }),
        0 => Some(Position {
            line: (bits & ((1 << 12) - 1)).cast_signed(),
            column: (bits >> 12).cast_signed(),
        }),
        _ => None,
    }
}

/// Decodes a `PrimJS` pc2line table: the start position and the changes as
/// `(pc, packed position)`. `None` for a table that does not decode.
pub(super) fn decode_primjs(line_num: i64, table: &[u8]) -> Option<Vec<(u32, i64)>> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    let mut pc: u64 = 0;
    let mut line = line_num;
    while at < table.len() {
        let op = table[at];
        at += 1;
        if op == 0 {
            let (delta_pc, used) = leb128_u64(&table[at..])?;
            at += used;
            let (delta_line, used) = leb128_u64(&table[at..])?;
            at += used;
            let delta_line = (delta_line >> 1).cast_signed() ^ -(delta_line & 1).cast_signed();
            pc = pc.checked_add(delta_pc)?;
            line = line.wrapping_add(delta_line);
        } else {
            let op = i64::from(op) - i64::from(pc2line::OP_FIRST);
            let range = i64::from(pc2line::RANGE);
            pc = pc.checked_add(u64::try_from(op / range).ok()?)?;
            line = line.wrapping_add(op % range + i64::from(pc2line::BASE));
        }
        entries.push((u32::try_from(pc).ok()?, line));
    }
    Some(entries)
}

/// Encodes a fork pc2line table from the function's start position and
/// the changes `(pc, position)` in pc order. The fork stores line and
/// column minus one (`find_line_num` adds one back), so a `0` of `PrimJS`
/// is written as 1. `None` when a value does not fit the fork's 32-bit
/// fields.
pub(super) fn encode_fork(start: Position, changes: &[(u32, Position)]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut last_line = i32::try_from(start.line.max(1)).ok()?;
    let mut last_column = i32::try_from(start.column.max(1)).ok()?;
    put_leb128(&mut out, u32::try_from(last_line - 1).ok()?);
    put_leb128(&mut out, u32::try_from(last_column - 1).ok()?);
    let mut last_pc = 0u32;
    for &(pc, position) in changes {
        let Some(diff_pc) = pc.checked_sub(last_pc) else {
            continue;
        };
        let line = i32::try_from(position.line.max(1)).ok()?;
        let column = i32::try_from(position.column.max(1)).ok()?;
        let diff_line = line.checked_sub(last_line)?;
        let diff_column = column.checked_sub(last_column)?;
        if diff_line == 0 && diff_column == 0 {
            continue;
        }
        let diff_pc_signed = i32::try_from(diff_pc).ok()?;
        if (pc2line::BASE..pc2line::BASE + pc2line::RANGE).contains(&diff_line)
            && diff_pc_signed <= pc2line::DIFF_PC_MAX
        {
            let byte =
                (diff_line - pc2line::BASE) + diff_pc_signed * pc2line::RANGE + pc2line::OP_FIRST;
            out.push(u8::try_from(byte).ok()?);
        } else {
            out.push(0);
            put_leb128(&mut out, diff_pc);
            put_sleb128(&mut out, diff_line);
        }
        put_sleb128(&mut out, diff_column);
        last_pc = pc;
        last_line = line;
        last_column = column;
    }
    Some(out)
}

fn leb128_u64(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

fn put_leb128(out: &mut Vec<u8>, mut value: u32) {
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
    put_leb128(out, ((value << 1) ^ (value >> 31)).cast_unsigned());
}

#[cfg(test)]
mod tests {
    use super::{Position, decode_primjs, encode_fork, unpack};

    #[test]
    fn unpacks_both_position_types() {
        let word = (1i64 << 62) | (7 << 24) | 3;
        assert_eq!(unpack(word), Some(Position { line: 3, column: 7 }));
        assert_eq!(unpack((5 << 12) | 9), Some(Position { line: 9, column: 5 }));
        assert_eq!(unpack(-1), None);
    }

    #[test]
    fn decodes_short_and_long_entries() {
        // op 1 + 2 * 5 + (1 - -1) = 13: pc + 2, line + 1; then a long
        // entry: pc + 100, line - 3.
        let table = [13, 0, 100, 5];
        assert_eq!(decode_primjs(10, &table), Some(vec![(2, 11), (102, 8)]));
        assert_eq!(decode_primjs(0, &[0, 0x80]), None);
    }

    #[test]
    fn encodes_the_fork_form() {
        let start = Position { line: 1, column: 1 };
        let table = encode_fork(
            start,
            &[
                (4, Position { line: 2, column: 5 }),
                (200, Position { line: 2, column: 1 }),
            ],
        )
        .unwrap();
        // line 0, column 0, then pc + 4 line + 1 (short), column + 4,
        // then pc + 196 (long), line + 0, column - 4.
        assert_eq!(table, vec![0, 0, 23, 8, 0, 196, 1, 0, 7]);
    }
}
