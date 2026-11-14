/// Errors produced while reading `PrimJS` serialized bytecode.
///
/// Every `offset` is a byte offset into the buffer given to the reader (after
/// a packed header has been removed, when there was one). Every `pc` is a
/// byte offset into one function's bytecode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PrimjsError {
    /// The input ended in the middle of a field.
    #[error(
        "PrimJS bytecode ends at byte {offset}: needed {needed} more bytes, {remaining} remain"
    )]
    UnexpectedEof {
        /// Offset of the failed read.
        offset: usize,
        /// Bytes the field needs.
        needed: usize,
        /// Bytes left in the input.
        remaining: usize,
    },

    /// A LEB128 value is longer than five bytes or does not fit in 32 bits.
    #[error("malformed LEB128 value at byte {offset}")]
    BadLeb128 {
        /// Offset of the value's first byte.
        offset: usize,
    },

    /// The first byte is a byte-swapped (big-endian writer) version:
    /// `0x41` or `0x49`. `PrimJS`'s reader refuses these too.
    #[error("byte-swapped PrimJS bytecode (version byte 0x{version:02x}) is not supported")]
    ByteSwapped {
        /// The version byte.
        version: u8,
    },

    /// The first byte is neither `1` nor `9`.
    #[error("unknown PrimJS bytecode version byte 0x{version:02x}; expected 0x01 or 0x09")]
    UnknownVersion {
        /// The version byte.
        version: u8,
    },

    /// The low 32 bits of a version-9 header word differ from the frozen
    /// value `0xCAB001CC`.
    #[error(
        "PrimJS bytecode version word 0x{found:08x} does not match 0x{expected:08x} (242 opcodes, 202 built-in atoms)"
    )]
    VersionWordMismatch {
        /// Low 32 bits found.
        found: u32,
        /// The value this reader's tables correspond to.
        expected: u32,
    },

    /// A version-9 header word sets a bit above bit 31 other than the
    /// external-debug-info (32) and stripped-vardefs (33) flags.
    #[error("PrimJS bytecode header sets unknown flag bits 0x{bits:016x}")]
    UnknownHeaderFlags {
        /// The unknown bits, in place.
        bits: u64,
    },

    /// A value carries `BC_TAG_BIG_FLOAT`, which is rejected by ruling.
    #[error("BigFloat value at byte {offset} is not supported")]
    BigFloat {
        /// Offset of the tag.
        offset: usize,
    },

    /// A value carries `BC_TAG_MODULE`; `PrimJS` module input is rejected by
    /// ruling.
    #[error("PrimJS module record at byte {offset} is not supported")]
    Module {
        /// Offset of the tag.
        offset: usize,
    },

    /// A value tag outside `BCTagEnum`.
    #[error("unknown PrimJS value tag {tag} at byte {offset}")]
    UnknownTag {
        /// Offset of the tag.
        offset: usize,
        /// The tag byte.
        tag: u8,
    },

    /// The top-level value is not a function.
    #[error("top-level PrimJS value at byte {offset} has tag {tag}, not a function")]
    RootNotFunction {
        /// Offset of the tag.
        offset: usize,
        /// The tag byte.
        tag: u8,
    },

    /// Values or functions are nested deeper than the reader allows.
    #[error("PrimJS values nest deeper than {limit} levels at byte {offset}")]
    NestingTooDeep {
        /// Offset of the value that exceeds the limit.
        offset: usize,
        /// The limit.
        limit: usize,
    },

    /// A count cannot be satisfied by the bytes that remain, or exceeds the
    /// field's range.
    #[error("{what} count {count} at byte {offset} is out of range")]
    CountOutOfRange {
        /// Offset of the count.
        offset: usize,
        /// Which count.
        what: &'static str,
        /// The count read.
        count: u64,
    },

    /// An atom names neither a built-in atom nor an entry of the file's atom
    /// table.
    #[error("atom index {index} at byte {offset} is outside the built-in and file atom tables")]
    AtomOutOfRange {
        /// Offset of the atom field (for an instruction operand, of the
        /// instruction).
        offset: usize,
        /// The serialized index.
        index: u32,
    },

    /// A flag field sets bits the format does not define.
    #[error("{what} at byte {offset} sets undefined bits 0x{bits:x}")]
    UnknownFlags {
        /// Offset of the flag field.
        offset: usize,
        /// Which field.
        what: &'static str,
        /// The undefined bits, in place.
        bits: u32,
    },

    /// A record field is inconsistent with another.
    #[error("invalid PrimJS function record at byte {offset}: {reason}")]
    InvalidRecord {
        /// Offset of the field.
        offset: usize,
        /// What is wrong.
        reason: String,
    },

    /// An opcode byte is `0` (`invalid`) or at or above 242.
    #[error("invalid opcode {opcode} at pc {pc} (byte {offset})")]
    InvalidOpcode {
        /// Absolute offset of the instruction.
        offset: usize,
        /// Offset inside the function's bytecode.
        pc: u32,
        /// The opcode byte.
        opcode: u8,
    },

    /// The last instruction extends past the function's bytecode.
    #[error("instruction {opcode} at pc {pc} (byte {offset}) extends past the function's bytecode")]
    TruncatedInstruction {
        /// Absolute offset of the instruction.
        offset: usize,
        /// Offset inside the function's bytecode.
        pc: u32,
        /// The opcode's name.
        opcode: &'static str,
    },

    /// A local, argument, closure-variable or constant-pool operand is not
    /// below its count.
    #[error("{kind} index {index} of {opcode} at pc {pc} (byte {offset}) is not below {limit}")]
    IndexOutOfRange {
        /// Absolute offset of the instruction.
        offset: usize,
        /// Offset inside the function's bytecode.
        pc: u32,
        /// The opcode's name.
        opcode: &'static str,
        /// `local`, `argument`, `closure variable` or `constant pool`.
        kind: &'static str,
        /// The operand.
        index: u32,
        /// The count it must be below.
        limit: u32,
    },

    /// A jump target is not the start of an instruction of the same function.
    #[error(
        "jump of {opcode} at pc {pc} (byte {offset}) targets {target}, which is not an instruction start"
    )]
    BadJumpTarget {
        /// Absolute offset of the instruction.
        offset: usize,
        /// Offset inside the function's bytecode.
        pc: u32,
        /// The opcode's name.
        opcode: &'static str,
        /// The target as a pc (may be negative or past the end).
        target: i64,
    },

    /// A child function's closure variable names a slot its parent does not
    /// have.
    #[error(
        "closure variable {closure_index} of the function at byte {child_offset} names {kind} {index}, but its parent has {limit}"
    )]
    ClosureVarOutOfRange {
        /// Offset of the child function's tag.
        child_offset: usize,
        /// Which closure variable of the child.
        closure_index: u32,
        /// `local`, `argument` or `closure variable` of the parent.
        kind: &'static str,
        /// The index.
        index: u32,
        /// The parent's count.
        limit: u32,
    },

    /// Bytes remain after the top-level value.
    #[error("{remaining} bytes follow the top-level PrimJS value at byte {offset}")]
    TrailingBytes {
        /// Offset of the first trailing byte.
        offset: usize,
        /// Number of trailing bytes.
        remaining: usize,
    },

    /// A packed background-thread header is malformed.
    #[error("invalid packed bytecode header: {reason}")]
    BadPackedHeader {
        /// What is wrong.
        reason: String,
    },
}
