//! Frozen `PrimJS` tables: opcodes, built-in atoms and value tags.
//!
//! Transcribed from `lynx-family/primjs` at commit `941a365` (2.11.1-rc.1):
//! the opcode list from `src/interpreter/quickjs/include/quickjs-opcode.h`
//! (every `DEF` entry in file order with `SHORT_OPCODES` set to 1, as
//! `include/quickjs-inner.h` defines it; the lowercase `def` temporary
//! opcodes are not numbered in the serialized stream), the atom list from
//! `include/quickjs-atom.h` without the three `CONFIG_ATOMICS` atoms, and the
//! value tags from `BCTagEnum` in `source/quickjs.cc`.
//!
//! `PrimJS` freezes all three: `OP_SHIPPABLE_COUNT == 242` is a `static_assert`
//! in `quickjs-inner.h`, and a built-in atom is serialized as its raw index,
//! so neither list can change without changing what existing bytes mean. The
//! version word of a version-9 header is `0xCAB00000 | (242 + 16 + 202)`
//! (`LEPUS_GetPrimjsVersion`), which is how the shipping encoder's atom count
//! of 202 (a build without `CONFIG_ATOMICS`) is known.

/// Number of opcodes that can appear in serialized bytecode
/// (`OP_SHIPPABLE_COUNT`). Opcode numbers `0..OPCODE_COUNT` are shippable,
/// except `0` (`invalid`), which `PrimJS`'s reader also refuses.
pub const OPCODE_COUNT: usize = 242;

/// The opcode numbered [`OPCODE_COUNT`]: `inc_coverage`, a
/// `DEF_NON_SHIPPABLE` entry that `PrimJS`'s writer refuses to serialize and its
/// reader refuses to read.
pub const NON_SHIPPABLE_INC_COVERAGE: u8 = 242;

/// `JS_ATOM_END`: built-in atom indices are `0..ATOM_END`, where `0` is
/// `JS_ATOM_NULL`. A serialized atom index at or above it names the file's
/// own atom table at `index - ATOM_END`.
pub const ATOM_END: u32 = 203;

/// Number of built-in atoms that carry a string (`JS_ATOM_END - 1`).
pub const BUILTIN_ATOM_COUNT: usize = 202;

/// Bit 31 of a serialized instruction atom operand: the atom is a tagged
/// integer (`__JS_AtomIsTaggedInt`) whose value is the low 31 bits.
pub const ATOM_TAG_INT: u32 = 1 << 31;

/// `BCTagEnum` from `source/quickjs.cc`: the first byte of every serialized
/// value.
pub mod tag {
    /// `BC_TAG_NULL`
    pub const NULL: u8 = 1;
    /// `BC_TAG_UNDEFINED`
    pub const UNDEFINED: u8 = 2;
    /// `BC_TAG_BOOL_FALSE`
    pub const BOOL_FALSE: u8 = 3;
    /// `BC_TAG_BOOL_TRUE`
    pub const BOOL_TRUE: u8 = 4;
    /// `BC_TAG_INT32`, followed by a signed LEB128 (zigzag) value.
    pub const INT32: u8 = 5;
    /// `BC_TAG_FLOAT64`, followed by 8 little-endian bytes.
    pub const FLOAT64: u8 = 6;
    /// `BC_TAG_STRING`, followed by a string.
    pub const STRING: u8 = 7;
    /// `BC_TAG_OBJECT`, followed by a property count and (atom, value) pairs.
    pub const OBJECT: u8 = 8;
    /// `BC_TAG_ARRAY`, followed by a length and that many values.
    pub const ARRAY: u8 = 9;
    /// `BC_TAG_BIG_INT`, followed by a byte length and the two's-complement
    /// bytes, least significant first.
    pub const BIG_INT: u8 = 10;
    /// `BC_TAG_BIG_FLOAT`. `PrimJS`'s own reader does not accept it; the
    /// converter rejects it by ruling.
    pub const BIG_FLOAT: u8 = 11;
    /// `BC_TAG_TEMPLATE_OBJECT`: an array followed by its `raw` value.
    pub const TEMPLATE_OBJECT: u8 = 12;
    /// `BC_TAG_FUNCTION_BYTECODE`
    pub const FUNCTION_BYTECODE: u8 = 13;
    /// `BC_TAG_MODULE`. Rejected by ruling: `PrimJS` module input is not
    /// converted.
    pub const MODULE: u8 = 14;
}

/// Operand format of an opcode (`OPCodeFormat`, the `FMT` entries of
/// `quickjs-opcode.h`), restricted to the formats shippable opcodes use.
/// Operand bytes follow the opcode byte in little-endian order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// No operand.
    None,
    /// No operand bytes; the integer is implied by the opcode
    /// (`push_minus1` .. `push_7`).
    NoneInt,
    /// No operand bytes; the local index is implied by the opcode
    /// (`get_loc0` .. `set_loc3`).
    NoneLoc,
    /// No operand bytes; the argument index is implied by the opcode.
    NoneArg,
    /// No operand bytes; the closure-variable index is implied by the opcode.
    NoneVarRef,
    /// One unsigned byte.
    U8,
    /// One signed byte.
    I8,
    /// One-byte local index.
    Loc8,
    /// One-byte constant-pool index.
    Const8,
    /// One-byte signed jump offset, relative to `pc + 1`.
    Label8,
    /// Unsigned 16-bit immediate.
    U16,
    /// Signed 16-bit immediate.
    I16,
    /// Signed 16-bit jump offset, relative to `pc + 1`.
    Label16,
    /// 16-bit argument count of a call; the arguments are popped in addition
    /// to the table's `pops`.
    Npop,
    /// No operand bytes; the argument count is implied by the opcode
    /// (`call0` .. `call3`) and popped in addition to the table's `pops`.
    Npopx,
    /// 16-bit local index.
    Loc,
    /// 16-bit argument index.
    Arg,
    /// 16-bit closure-variable index.
    VarRef,
    /// Signed 32-bit immediate.
    I32,
    /// 32-bit constant-pool index.
    Const,
    /// Signed 32-bit jump offset, relative to `pc + 1`.
    Label,
    /// 32-bit atom.
    Atom,
    /// 32-bit atom, then one unsigned byte.
    AtomU8,
    /// 32-bit atom, then an unsigned 16-bit value. For `make_loc_ref`,
    /// `make_arg_ref` and `make_var_ref_ref` the 16-bit value is a local,
    /// argument or closure-variable index.
    AtomU16,
    /// 32-bit atom, a signed 32-bit jump offset relative to `pc + 5`, then
    /// one unsigned byte (the `with_*` family).
    AtomLabelU8,
}

/// One row of the opcode table (`JSOpCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpcodeInfo {
    /// Name as written in `quickjs-opcode.h`.
    pub name: &'static str,
    /// Encoded size in bytes, opcode byte included.
    pub size: u8,
    /// Values popped. For [`Format::Npop`] and [`Format::Npopx`] the call's
    /// arguments are popped in addition.
    pub pops: u8,
    /// Values pushed.
    pub pushes: u8,
    /// Operand format.
    pub format: Format,
}

const fn info(name: &'static str, size: u8, pops: u8, pushes: u8, format: Format) -> OpcodeInfo {
    OpcodeInfo {
        name,
        size,
        pops,
        pushes,
        format,
    }
}

impl Opcode {
    /// The opcode with this number, or `None` at or above [`OPCODE_COUNT`].
    #[must_use]
    pub fn from_u8(byte: u8) -> Option<Self> {
        ALL_OPCODES.get(usize::from(byte)).copied()
    }

    /// The opcode's number in the serialized stream.
    #[must_use]
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// The opcode's table row.
    #[must_use]
    pub fn info(self) -> &'static OpcodeInfo {
        &OPCODE_INFO[usize::from(self.number())]
    }

    /// Name as written in `quickjs-opcode.h`.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.info().name
    }

    /// Encoded size in bytes, opcode byte included.
    #[must_use]
    pub fn size(self) -> u8 {
        self.info().size
    }

    /// Values popped, not counting a call's arguments (see [`Format::Npop`]).
    #[must_use]
    pub fn pops(self) -> u8 {
        self.info().pops
    }

    /// Values pushed.
    #[must_use]
    pub fn pushes(self) -> u8 {
        self.info().pushes
    }

    /// Operand format.
    #[must_use]
    pub fn format(self) -> Format {
        self.info().format
    }
}

/// The text of built-in atom `index`, or `None` outside `0..ATOM_END`. Index
/// `0` is `JS_ATOM_NULL`, whose entry is the empty string; index 47
/// (`JS_ATOM_empty_string`) is the real empty string.
#[must_use]
pub fn builtin_atom(index: u32) -> Option<&'static str> {
    BUILTIN_ATOMS.get(usize::try_from(index).ok()?).copied()
}

/// Every shippable opcode, numbered as in the serialized stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Opcode {
    /// `invalid`
    Invalid = 0,
    /// `push_i32`
    PushI32 = 1,
    /// `push_const`
    PushConst = 2,
    /// `fclosure`
    Fclosure = 3,
    /// `push_atom_value`
    PushAtomValue = 4,
    /// `private_symbol`
    PrivateSymbol = 5,
    /// `undefined`
    Undefined = 6,
    /// `null`
    Null = 7,
    /// `push_this`
    PushThis = 8,
    /// `push_false`
    PushFalse = 9,
    /// `push_true`
    PushTrue = 10,
    /// `object`
    Object = 11,
    /// `special_object`
    SpecialObject = 12,
    /// `rest`
    Rest = 13,
    /// `drop`
    Drop = 14,
    /// `nip`
    Nip = 15,
    /// `nip1`
    Nip1 = 16,
    /// `dup`
    Dup = 17,
    /// `dup1`
    Dup1 = 18,
    /// `dup2`
    Dup2 = 19,
    /// `dup3`
    Dup3 = 20,
    /// `insert2`
    Insert2 = 21,
    /// `insert3`
    Insert3 = 22,
    /// `insert4`
    Insert4 = 23,
    /// `perm3`
    Perm3 = 24,
    /// `perm4`
    Perm4 = 25,
    /// `perm5`
    Perm5 = 26,
    /// `swap`
    Swap = 27,
    /// `swap2`
    Swap2 = 28,
    /// `rot3l`
    Rot3l = 29,
    /// `rot3r`
    Rot3r = 30,
    /// `rot4l`
    Rot4l = 31,
    /// `rot5l`
    Rot5l = 32,
    /// `call_constructor`
    CallConstructor = 33,
    /// `call`
    Call = 34,
    /// `tail_call`
    TailCall = 35,
    /// `call_method`
    CallMethod = 36,
    /// `tail_call_method`
    TailCallMethod = 37,
    /// `array_from`
    ArrayFrom = 38,
    /// `apply`
    Apply = 39,
    /// `return`
    Return = 40,
    /// `return_undef`
    ReturnUndef = 41,
    /// `check_ctor_return`
    CheckCtorReturn = 42,
    /// `check_ctor`
    CheckCtor = 43,
    /// `check_brand`
    CheckBrand = 44,
    /// `add_brand`
    AddBrand = 45,
    /// `return_async`
    ReturnAsync = 46,
    /// `throw`
    Throw = 47,
    /// `throw_var`
    ThrowVar = 48,
    /// `eval`
    Eval = 49,
    /// `regexp`
    Regexp = 50,
    /// `get_super_ctor`
    GetSuperCtor = 51,
    /// `get_super`
    GetSuper = 52,
    /// `import`
    Import = 53,
    /// `check_var`
    CheckVar = 54,
    /// `get_var_undef`
    GetVarUndef = 55,
    /// `get_var`
    GetVar = 56,
    /// `put_var`
    PutVar = 57,
    /// `put_var_init`
    PutVarInit = 58,
    /// `put_var_strict`
    PutVarStrict = 59,
    /// `get_ref_value`
    GetRefValue = 60,
    /// `put_ref_value`
    PutRefValue = 61,
    /// `define_var`
    DefineVar = 62,
    /// `check_define_var`
    CheckDefineVar = 63,
    /// `define_func`
    DefineFunc = 64,
    /// `get_field`
    GetField = 65,
    /// `get_field2`
    GetField2 = 66,
    /// `put_field`
    PutField = 67,
    /// `get_private_field`
    GetPrivateField = 68,
    /// `put_private_field`
    PutPrivateField = 69,
    /// `define_private_field`
    DefinePrivateField = 70,
    /// `get_array_el`
    GetArrayEl = 71,
    /// `get_array_el2`
    GetArrayEl2 = 72,
    /// `put_array_el`
    PutArrayEl = 73,
    /// `get_super_value`
    GetSuperValue = 74,
    /// `put_super_value`
    PutSuperValue = 75,
    /// `define_field`
    DefineField = 76,
    /// `set_name`
    SetName = 77,
    /// `set_name_computed`
    SetNameComputed = 78,
    /// `set_proto`
    SetProto = 79,
    /// `set_home_object`
    SetHomeObject = 80,
    /// `define_array_el`
    DefineArrayEl = 81,
    /// `append`
    Append = 82,
    /// `copy_data_properties`
    CopyDataProperties = 83,
    /// `define_method`
    DefineMethod = 84,
    /// `define_method_computed`
    DefineMethodComputed = 85,
    /// `define_class`
    DefineClass = 86,
    /// `get_loc`
    GetLoc = 87,
    /// `put_loc`
    PutLoc = 88,
    /// `set_loc`
    SetLoc = 89,
    /// `get_arg`
    GetArg = 90,
    /// `put_arg`
    PutArg = 91,
    /// `set_arg`
    SetArg = 92,
    /// `get_var_ref`
    GetVarRef = 93,
    /// `put_var_ref`
    PutVarRef = 94,
    /// `set_var_ref`
    SetVarRef = 95,
    /// `set_loc_uninitialized`
    SetLocUninitialized = 96,
    /// `get_loc_check`
    GetLocCheck = 97,
    /// `put_loc_check`
    PutLocCheck = 98,
    /// `put_loc_check_init`
    PutLocCheckInit = 99,
    /// `get_var_ref_check`
    GetVarRefCheck = 100,
    /// `put_var_ref_check`
    PutVarRefCheck = 101,
    /// `put_var_ref_check_init`
    PutVarRefCheckInit = 102,
    /// `close_loc`
    CloseLoc = 103,
    /// `if_false`
    IfFalse = 104,
    /// `if_true`
    IfTrue = 105,
    /// `goto`
    Goto = 106,
    /// `catch`
    Catch = 107,
    /// `gosub`
    Gosub = 108,
    /// `ret`
    Ret = 109,
    /// `to_object`
    ToObject = 110,
    /// `to_propkey`
    ToPropkey = 111,
    /// `to_propkey2`
    ToPropkey2 = 112,
    /// `with_get_var`
    WithGetVar = 113,
    /// `with_put_var`
    WithPutVar = 114,
    /// `with_delete_var`
    WithDeleteVar = 115,
    /// `with_make_ref`
    WithMakeRef = 116,
    /// `with_get_ref`
    WithGetRef = 117,
    /// `with_get_ref_undef`
    WithGetRefUndef = 118,
    /// `make_loc_ref`
    MakeLocRef = 119,
    /// `make_arg_ref`
    MakeArgRef = 120,
    /// `make_var_ref_ref`
    MakeVarRefRef = 121,
    /// `make_var_ref`
    MakeVarRef = 122,
    /// `for_in_start`
    ForInStart = 123,
    /// `for_of_start`
    ForOfStart = 124,
    /// `for_await_of_start`
    ForAwaitOfStart = 125,
    /// `for_in_next`
    ForInNext = 126,
    /// `for_of_next`
    ForOfNext = 127,
    /// `for_await_of_next`
    ForAwaitOfNext = 128,
    /// `iterator_get_value_done`
    IteratorGetValueDone = 129,
    /// `iterator_close`
    IteratorClose = 130,
    /// `iterator_close_return`
    IteratorCloseReturn = 131,
    /// `async_iterator_close`
    AsyncIteratorClose = 132,
    /// `async_iterator_next`
    AsyncIteratorNext = 133,
    /// `async_iterator_get`
    AsyncIteratorGet = 134,
    /// `initial_yield`
    InitialYield = 135,
    /// `yield`
    Yield = 136,
    /// `yield_star`
    YieldStar = 137,
    /// `async_yield_star`
    AsyncYieldStar = 138,
    /// `await`
    Await = 139,
    /// `neg`
    Neg = 140,
    /// `plus`
    Plus = 141,
    /// `dec`
    Dec = 142,
    /// `inc`
    Inc = 143,
    /// `post_dec`
    PostDec = 144,
    /// `post_inc`
    PostInc = 145,
    /// `dec_loc`
    DecLoc = 146,
    /// `inc_loc`
    IncLoc = 147,
    /// `add_loc`
    AddLoc = 148,
    /// `not`
    Not = 149,
    /// `lnot`
    Lnot = 150,
    /// `typeof`
    Typeof = 151,
    /// `delete`
    Delete = 152,
    /// `delete_var`
    DeleteVar = 153,
    /// `mul`
    Mul = 154,
    /// `div`
    Div = 155,
    /// `mod`
    Mod = 156,
    /// `add`
    Add = 157,
    /// `sub`
    Sub = 158,
    /// `pow`
    Pow = 159,
    /// `shl`
    Shl = 160,
    /// `sar`
    Sar = 161,
    /// `shr`
    Shr = 162,
    /// `lt`
    Lt = 163,
    /// `lte`
    Lte = 164,
    /// `gt`
    Gt = 165,
    /// `gte`
    Gte = 166,
    /// `instanceof`
    Instanceof = 167,
    /// `in`
    In = 168,
    /// `eq`
    Eq = 169,
    /// `neq`
    Neq = 170,
    /// `strict_eq`
    StrictEq = 171,
    /// `strict_neq`
    StrictNeq = 172,
    /// `and`
    And = 173,
    /// `xor`
    Xor = 174,
    /// `or`
    Or = 175,
    /// `nop`
    Nop = 176,
    /// `push_minus1`
    PushMinus1 = 177,
    /// `push_0`
    Push0 = 178,
    /// `push_1`
    Push1 = 179,
    /// `push_2`
    Push2 = 180,
    /// `push_3`
    Push3 = 181,
    /// `push_4`
    Push4 = 182,
    /// `push_5`
    Push5 = 183,
    /// `push_6`
    Push6 = 184,
    /// `push_7`
    Push7 = 185,
    /// `push_i8`
    PushI8 = 186,
    /// `push_i16`
    PushI16 = 187,
    /// `push_const8`
    PushConst8 = 188,
    /// `fclosure8`
    Fclosure8 = 189,
    /// `push_empty_string`
    PushEmptyString = 190,
    /// `get_loc8`
    GetLoc8 = 191,
    /// `put_loc8`
    PutLoc8 = 192,
    /// `set_loc8`
    SetLoc8 = 193,
    /// `get_loc0`
    GetLoc0 = 194,
    /// `get_loc1`
    GetLoc1 = 195,
    /// `get_loc2`
    GetLoc2 = 196,
    /// `get_loc3`
    GetLoc3 = 197,
    /// `put_loc0`
    PutLoc0 = 198,
    /// `put_loc1`
    PutLoc1 = 199,
    /// `put_loc2`
    PutLoc2 = 200,
    /// `put_loc3`
    PutLoc3 = 201,
    /// `set_loc0`
    SetLoc0 = 202,
    /// `set_loc1`
    SetLoc1 = 203,
    /// `set_loc2`
    SetLoc2 = 204,
    /// `set_loc3`
    SetLoc3 = 205,
    /// `get_arg0`
    GetArg0 = 206,
    /// `get_arg1`
    GetArg1 = 207,
    /// `get_arg2`
    GetArg2 = 208,
    /// `get_arg3`
    GetArg3 = 209,
    /// `put_arg0`
    PutArg0 = 210,
    /// `put_arg1`
    PutArg1 = 211,
    /// `put_arg2`
    PutArg2 = 212,
    /// `put_arg3`
    PutArg3 = 213,
    /// `set_arg0`
    SetArg0 = 214,
    /// `set_arg1`
    SetArg1 = 215,
    /// `set_arg2`
    SetArg2 = 216,
    /// `set_arg3`
    SetArg3 = 217,
    /// `get_var_ref0`
    GetVarRef0 = 218,
    /// `get_var_ref1`
    GetVarRef1 = 219,
    /// `get_var_ref2`
    GetVarRef2 = 220,
    /// `get_var_ref3`
    GetVarRef3 = 221,
    /// `put_var_ref0`
    PutVarRef0 = 222,
    /// `put_var_ref1`
    PutVarRef1 = 223,
    /// `put_var_ref2`
    PutVarRef2 = 224,
    /// `put_var_ref3`
    PutVarRef3 = 225,
    /// `set_var_ref0`
    SetVarRef0 = 226,
    /// `set_var_ref1`
    SetVarRef1 = 227,
    /// `set_var_ref2`
    SetVarRef2 = 228,
    /// `set_var_ref3`
    SetVarRef3 = 229,
    /// `get_length`
    GetLength = 230,
    /// `if_false8`
    IfFalse8 = 231,
    /// `if_true8`
    IfTrue8 = 232,
    /// `goto8`
    Goto8 = 233,
    /// `goto16`
    Goto16 = 234,
    /// `call0`
    Call0 = 235,
    /// `call1`
    Call1 = 236,
    /// `call2`
    Call2 = 237,
    /// `call3`
    Call3 = 238,
    /// `is_undefined`
    IsUndefined = 239,
    /// `is_null`
    IsNull = 240,
    /// `is_function`
    IsFunction = 241,
}
/// Every opcode in number order.
pub const ALL_OPCODES: [Opcode; OPCODE_COUNT] = [
    Opcode::Invalid,
    Opcode::PushI32,
    Opcode::PushConst,
    Opcode::Fclosure,
    Opcode::PushAtomValue,
    Opcode::PrivateSymbol,
    Opcode::Undefined,
    Opcode::Null,
    Opcode::PushThis,
    Opcode::PushFalse,
    Opcode::PushTrue,
    Opcode::Object,
    Opcode::SpecialObject,
    Opcode::Rest,
    Opcode::Drop,
    Opcode::Nip,
    Opcode::Nip1,
    Opcode::Dup,
    Opcode::Dup1,
    Opcode::Dup2,
    Opcode::Dup3,
    Opcode::Insert2,
    Opcode::Insert3,
    Opcode::Insert4,
    Opcode::Perm3,
    Opcode::Perm4,
    Opcode::Perm5,
    Opcode::Swap,
    Opcode::Swap2,
    Opcode::Rot3l,
    Opcode::Rot3r,
    Opcode::Rot4l,
    Opcode::Rot5l,
    Opcode::CallConstructor,
    Opcode::Call,
    Opcode::TailCall,
    Opcode::CallMethod,
    Opcode::TailCallMethod,
    Opcode::ArrayFrom,
    Opcode::Apply,
    Opcode::Return,
    Opcode::ReturnUndef,
    Opcode::CheckCtorReturn,
    Opcode::CheckCtor,
    Opcode::CheckBrand,
    Opcode::AddBrand,
    Opcode::ReturnAsync,
    Opcode::Throw,
    Opcode::ThrowVar,
    Opcode::Eval,
    Opcode::Regexp,
    Opcode::GetSuperCtor,
    Opcode::GetSuper,
    Opcode::Import,
    Opcode::CheckVar,
    Opcode::GetVarUndef,
    Opcode::GetVar,
    Opcode::PutVar,
    Opcode::PutVarInit,
    Opcode::PutVarStrict,
    Opcode::GetRefValue,
    Opcode::PutRefValue,
    Opcode::DefineVar,
    Opcode::CheckDefineVar,
    Opcode::DefineFunc,
    Opcode::GetField,
    Opcode::GetField2,
    Opcode::PutField,
    Opcode::GetPrivateField,
    Opcode::PutPrivateField,
    Opcode::DefinePrivateField,
    Opcode::GetArrayEl,
    Opcode::GetArrayEl2,
    Opcode::PutArrayEl,
    Opcode::GetSuperValue,
    Opcode::PutSuperValue,
    Opcode::DefineField,
    Opcode::SetName,
    Opcode::SetNameComputed,
    Opcode::SetProto,
    Opcode::SetHomeObject,
    Opcode::DefineArrayEl,
    Opcode::Append,
    Opcode::CopyDataProperties,
    Opcode::DefineMethod,
    Opcode::DefineMethodComputed,
    Opcode::DefineClass,
    Opcode::GetLoc,
    Opcode::PutLoc,
    Opcode::SetLoc,
    Opcode::GetArg,
    Opcode::PutArg,
    Opcode::SetArg,
    Opcode::GetVarRef,
    Opcode::PutVarRef,
    Opcode::SetVarRef,
    Opcode::SetLocUninitialized,
    Opcode::GetLocCheck,
    Opcode::PutLocCheck,
    Opcode::PutLocCheckInit,
    Opcode::GetVarRefCheck,
    Opcode::PutVarRefCheck,
    Opcode::PutVarRefCheckInit,
    Opcode::CloseLoc,
    Opcode::IfFalse,
    Opcode::IfTrue,
    Opcode::Goto,
    Opcode::Catch,
    Opcode::Gosub,
    Opcode::Ret,
    Opcode::ToObject,
    Opcode::ToPropkey,
    Opcode::ToPropkey2,
    Opcode::WithGetVar,
    Opcode::WithPutVar,
    Opcode::WithDeleteVar,
    Opcode::WithMakeRef,
    Opcode::WithGetRef,
    Opcode::WithGetRefUndef,
    Opcode::MakeLocRef,
    Opcode::MakeArgRef,
    Opcode::MakeVarRefRef,
    Opcode::MakeVarRef,
    Opcode::ForInStart,
    Opcode::ForOfStart,
    Opcode::ForAwaitOfStart,
    Opcode::ForInNext,
    Opcode::ForOfNext,
    Opcode::ForAwaitOfNext,
    Opcode::IteratorGetValueDone,
    Opcode::IteratorClose,
    Opcode::IteratorCloseReturn,
    Opcode::AsyncIteratorClose,
    Opcode::AsyncIteratorNext,
    Opcode::AsyncIteratorGet,
    Opcode::InitialYield,
    Opcode::Yield,
    Opcode::YieldStar,
    Opcode::AsyncYieldStar,
    Opcode::Await,
    Opcode::Neg,
    Opcode::Plus,
    Opcode::Dec,
    Opcode::Inc,
    Opcode::PostDec,
    Opcode::PostInc,
    Opcode::DecLoc,
    Opcode::IncLoc,
    Opcode::AddLoc,
    Opcode::Not,
    Opcode::Lnot,
    Opcode::Typeof,
    Opcode::Delete,
    Opcode::DeleteVar,
    Opcode::Mul,
    Opcode::Div,
    Opcode::Mod,
    Opcode::Add,
    Opcode::Sub,
    Opcode::Pow,
    Opcode::Shl,
    Opcode::Sar,
    Opcode::Shr,
    Opcode::Lt,
    Opcode::Lte,
    Opcode::Gt,
    Opcode::Gte,
    Opcode::Instanceof,
    Opcode::In,
    Opcode::Eq,
    Opcode::Neq,
    Opcode::StrictEq,
    Opcode::StrictNeq,
    Opcode::And,
    Opcode::Xor,
    Opcode::Or,
    Opcode::Nop,
    Opcode::PushMinus1,
    Opcode::Push0,
    Opcode::Push1,
    Opcode::Push2,
    Opcode::Push3,
    Opcode::Push4,
    Opcode::Push5,
    Opcode::Push6,
    Opcode::Push7,
    Opcode::PushI8,
    Opcode::PushI16,
    Opcode::PushConst8,
    Opcode::Fclosure8,
    Opcode::PushEmptyString,
    Opcode::GetLoc8,
    Opcode::PutLoc8,
    Opcode::SetLoc8,
    Opcode::GetLoc0,
    Opcode::GetLoc1,
    Opcode::GetLoc2,
    Opcode::GetLoc3,
    Opcode::PutLoc0,
    Opcode::PutLoc1,
    Opcode::PutLoc2,
    Opcode::PutLoc3,
    Opcode::SetLoc0,
    Opcode::SetLoc1,
    Opcode::SetLoc2,
    Opcode::SetLoc3,
    Opcode::GetArg0,
    Opcode::GetArg1,
    Opcode::GetArg2,
    Opcode::GetArg3,
    Opcode::PutArg0,
    Opcode::PutArg1,
    Opcode::PutArg2,
    Opcode::PutArg3,
    Opcode::SetArg0,
    Opcode::SetArg1,
    Opcode::SetArg2,
    Opcode::SetArg3,
    Opcode::GetVarRef0,
    Opcode::GetVarRef1,
    Opcode::GetVarRef2,
    Opcode::GetVarRef3,
    Opcode::PutVarRef0,
    Opcode::PutVarRef1,
    Opcode::PutVarRef2,
    Opcode::PutVarRef3,
    Opcode::SetVarRef0,
    Opcode::SetVarRef1,
    Opcode::SetVarRef2,
    Opcode::SetVarRef3,
    Opcode::GetLength,
    Opcode::IfFalse8,
    Opcode::IfTrue8,
    Opcode::Goto8,
    Opcode::Goto16,
    Opcode::Call0,
    Opcode::Call1,
    Opcode::Call2,
    Opcode::Call3,
    Opcode::IsUndefined,
    Opcode::IsNull,
    Opcode::IsFunction,
];
/// The opcode table, indexed by opcode number.
pub static OPCODE_INFO: [OpcodeInfo; OPCODE_COUNT] = [
    info("invalid", 1, 0, 0, Format::None),
    info("push_i32", 5, 0, 1, Format::I32),
    info("push_const", 5, 0, 1, Format::Const),
    info("fclosure", 5, 0, 1, Format::Const),
    info("push_atom_value", 5, 0, 1, Format::Atom),
    info("private_symbol", 5, 0, 1, Format::Atom),
    info("undefined", 1, 0, 1, Format::None),
    info("null", 1, 0, 1, Format::None),
    info("push_this", 1, 0, 1, Format::None),
    info("push_false", 1, 0, 1, Format::None),
    info("push_true", 1, 0, 1, Format::None),
    info("object", 1, 0, 1, Format::None),
    info("special_object", 2, 0, 1, Format::U8),
    info("rest", 3, 0, 1, Format::U16),
    info("drop", 1, 1, 0, Format::None),
    info("nip", 1, 2, 1, Format::None),
    info("nip1", 1, 3, 2, Format::None),
    info("dup", 1, 1, 2, Format::None),
    info("dup1", 1, 2, 3, Format::None),
    info("dup2", 1, 2, 4, Format::None),
    info("dup3", 1, 3, 6, Format::None),
    info("insert2", 1, 2, 3, Format::None),
    info("insert3", 1, 3, 4, Format::None),
    info("insert4", 1, 4, 5, Format::None),
    info("perm3", 1, 3, 3, Format::None),
    info("perm4", 1, 4, 4, Format::None),
    info("perm5", 1, 5, 5, Format::None),
    info("swap", 1, 2, 2, Format::None),
    info("swap2", 1, 4, 4, Format::None),
    info("rot3l", 1, 3, 3, Format::None),
    info("rot3r", 1, 3, 3, Format::None),
    info("rot4l", 1, 4, 4, Format::None),
    info("rot5l", 1, 5, 5, Format::None),
    info("call_constructor", 3, 2, 1, Format::Npop),
    info("call", 3, 1, 1, Format::Npop),
    info("tail_call", 3, 1, 0, Format::Npop),
    info("call_method", 3, 2, 1, Format::Npop),
    info("tail_call_method", 3, 2, 0, Format::Npop),
    info("array_from", 3, 0, 1, Format::Npop),
    info("apply", 3, 3, 1, Format::U16),
    info("return", 1, 1, 0, Format::None),
    info("return_undef", 1, 0, 0, Format::None),
    info("check_ctor_return", 1, 1, 2, Format::None),
    info("check_ctor", 1, 0, 0, Format::None),
    info("check_brand", 1, 2, 2, Format::None),
    info("add_brand", 1, 2, 0, Format::None),
    info("return_async", 1, 1, 0, Format::None),
    info("throw", 1, 1, 0, Format::None),
    info("throw_var", 6, 0, 0, Format::AtomU8),
    info("eval", 3, 1, 1, Format::U16),
    info("regexp", 1, 2, 1, Format::None),
    info("get_super_ctor", 1, 1, 1, Format::None),
    info("get_super", 1, 1, 1, Format::None),
    info("import", 1, 1, 1, Format::None),
    info("check_var", 5, 0, 1, Format::Atom),
    info("get_var_undef", 5, 0, 1, Format::Atom),
    info("get_var", 5, 0, 1, Format::Atom),
    info("put_var", 5, 1, 0, Format::Atom),
    info("put_var_init", 5, 1, 0, Format::Atom),
    info("put_var_strict", 5, 2, 0, Format::Atom),
    info("get_ref_value", 1, 2, 3, Format::None),
    info("put_ref_value", 1, 3, 0, Format::None),
    info("define_var", 6, 0, 0, Format::AtomU8),
    info("check_define_var", 6, 0, 0, Format::AtomU8),
    info("define_func", 6, 1, 0, Format::AtomU8),
    info("get_field", 5, 1, 1, Format::Atom),
    info("get_field2", 5, 1, 2, Format::Atom),
    info("put_field", 5, 2, 0, Format::Atom),
    info("get_private_field", 1, 2, 1, Format::None),
    info("put_private_field", 1, 3, 0, Format::None),
    info("define_private_field", 1, 3, 1, Format::None),
    info("get_array_el", 1, 2, 1, Format::None),
    info("get_array_el2", 1, 2, 2, Format::None),
    info("put_array_el", 1, 3, 0, Format::None),
    info("get_super_value", 1, 3, 1, Format::None),
    info("put_super_value", 1, 4, 0, Format::None),
    info("define_field", 5, 2, 1, Format::Atom),
    info("set_name", 5, 1, 1, Format::Atom),
    info("set_name_computed", 1, 2, 2, Format::None),
    info("set_proto", 1, 2, 1, Format::None),
    info("set_home_object", 1, 2, 2, Format::None),
    info("define_array_el", 1, 3, 2, Format::None),
    info("append", 1, 3, 2, Format::None),
    info("copy_data_properties", 2, 3, 3, Format::U8),
    info("define_method", 6, 2, 1, Format::AtomU8),
    info("define_method_computed", 2, 3, 1, Format::U8),
    info("define_class", 6, 2, 2, Format::AtomU8),
    info("get_loc", 3, 0, 1, Format::Loc),
    info("put_loc", 3, 1, 0, Format::Loc),
    info("set_loc", 3, 1, 1, Format::Loc),
    info("get_arg", 3, 0, 1, Format::Arg),
    info("put_arg", 3, 1, 0, Format::Arg),
    info("set_arg", 3, 1, 1, Format::Arg),
    info("get_var_ref", 3, 0, 1, Format::VarRef),
    info("put_var_ref", 3, 1, 0, Format::VarRef),
    info("set_var_ref", 3, 1, 1, Format::VarRef),
    info("set_loc_uninitialized", 3, 0, 0, Format::Loc),
    info("get_loc_check", 3, 0, 1, Format::Loc),
    info("put_loc_check", 3, 1, 0, Format::Loc),
    info("put_loc_check_init", 3, 1, 0, Format::Loc),
    info("get_var_ref_check", 3, 0, 1, Format::VarRef),
    info("put_var_ref_check", 3, 1, 0, Format::VarRef),
    info("put_var_ref_check_init", 3, 1, 0, Format::VarRef),
    info("close_loc", 3, 0, 0, Format::Loc),
    info("if_false", 5, 1, 0, Format::Label),
    info("if_true", 5, 1, 0, Format::Label),
    info("goto", 5, 0, 0, Format::Label),
    info("catch", 5, 0, 1, Format::Label),
    info("gosub", 5, 0, 0, Format::Label),
    info("ret", 1, 1, 0, Format::None),
    info("to_object", 1, 1, 1, Format::None),
    info("to_propkey", 1, 1, 1, Format::None),
    info("to_propkey2", 1, 2, 2, Format::None),
    info("with_get_var", 10, 1, 0, Format::AtomLabelU8),
    info("with_put_var", 10, 2, 1, Format::AtomLabelU8),
    info("with_delete_var", 10, 1, 0, Format::AtomLabelU8),
    info("with_make_ref", 10, 1, 0, Format::AtomLabelU8),
    info("with_get_ref", 10, 1, 0, Format::AtomLabelU8),
    info("with_get_ref_undef", 10, 1, 0, Format::AtomLabelU8),
    info("make_loc_ref", 7, 0, 2, Format::AtomU16),
    info("make_arg_ref", 7, 0, 2, Format::AtomU16),
    info("make_var_ref_ref", 7, 0, 2, Format::AtomU16),
    info("make_var_ref", 5, 0, 2, Format::Atom),
    info("for_in_start", 1, 1, 1, Format::None),
    info("for_of_start", 1, 1, 3, Format::None),
    info("for_await_of_start", 1, 1, 3, Format::None),
    info("for_in_next", 1, 1, 3, Format::None),
    info("for_of_next", 2, 3, 5, Format::U8),
    info("for_await_of_next", 1, 3, 4, Format::None),
    info("iterator_get_value_done", 1, 1, 2, Format::None),
    info("iterator_close", 1, 3, 0, Format::None),
    info("iterator_close_return", 1, 4, 4, Format::None),
    info("async_iterator_close", 1, 3, 2, Format::None),
    info("async_iterator_next", 1, 4, 4, Format::None),
    info("async_iterator_get", 2, 4, 5, Format::U8),
    info("initial_yield", 1, 0, 0, Format::None),
    info("yield", 1, 1, 2, Format::None),
    info("yield_star", 1, 2, 2, Format::None),
    info("async_yield_star", 1, 1, 2, Format::None),
    info("await", 1, 1, 1, Format::None),
    info("neg", 1, 1, 1, Format::None),
    info("plus", 1, 1, 1, Format::None),
    info("dec", 1, 1, 1, Format::None),
    info("inc", 1, 1, 1, Format::None),
    info("post_dec", 1, 1, 2, Format::None),
    info("post_inc", 1, 1, 2, Format::None),
    info("dec_loc", 2, 0, 0, Format::Loc8),
    info("inc_loc", 2, 0, 0, Format::Loc8),
    info("add_loc", 2, 1, 0, Format::Loc8),
    info("not", 1, 1, 1, Format::None),
    info("lnot", 1, 1, 1, Format::None),
    info("typeof", 1, 1, 1, Format::None),
    info("delete", 1, 2, 1, Format::None),
    info("delete_var", 5, 0, 1, Format::Atom),
    info("mul", 1, 2, 1, Format::None),
    info("div", 1, 2, 1, Format::None),
    info("mod", 1, 2, 1, Format::None),
    info("add", 1, 2, 1, Format::None),
    info("sub", 1, 2, 1, Format::None),
    info("pow", 1, 2, 1, Format::None),
    info("shl", 1, 2, 1, Format::None),
    info("sar", 1, 2, 1, Format::None),
    info("shr", 1, 2, 1, Format::None),
    info("lt", 1, 2, 1, Format::None),
    info("lte", 1, 2, 1, Format::None),
    info("gt", 1, 2, 1, Format::None),
    info("gte", 1, 2, 1, Format::None),
    info("instanceof", 1, 2, 1, Format::None),
    info("in", 1, 2, 1, Format::None),
    info("eq", 1, 2, 1, Format::None),
    info("neq", 1, 2, 1, Format::None),
    info("strict_eq", 1, 2, 1, Format::None),
    info("strict_neq", 1, 2, 1, Format::None),
    info("and", 1, 2, 1, Format::None),
    info("xor", 1, 2, 1, Format::None),
    info("or", 1, 2, 1, Format::None),
    info("nop", 1, 0, 0, Format::None),
    info("push_minus1", 1, 0, 1, Format::NoneInt),
    info("push_0", 1, 0, 1, Format::NoneInt),
    info("push_1", 1, 0, 1, Format::NoneInt),
    info("push_2", 1, 0, 1, Format::NoneInt),
    info("push_3", 1, 0, 1, Format::NoneInt),
    info("push_4", 1, 0, 1, Format::NoneInt),
    info("push_5", 1, 0, 1, Format::NoneInt),
    info("push_6", 1, 0, 1, Format::NoneInt),
    info("push_7", 1, 0, 1, Format::NoneInt),
    info("push_i8", 2, 0, 1, Format::I8),
    info("push_i16", 3, 0, 1, Format::I16),
    info("push_const8", 2, 0, 1, Format::Const8),
    info("fclosure8", 2, 0, 1, Format::Const8),
    info("push_empty_string", 1, 0, 1, Format::None),
    info("get_loc8", 2, 0, 1, Format::Loc8),
    info("put_loc8", 2, 1, 0, Format::Loc8),
    info("set_loc8", 2, 1, 1, Format::Loc8),
    info("get_loc0", 1, 0, 1, Format::NoneLoc),
    info("get_loc1", 1, 0, 1, Format::NoneLoc),
    info("get_loc2", 1, 0, 1, Format::NoneLoc),
    info("get_loc3", 1, 0, 1, Format::NoneLoc),
    info("put_loc0", 1, 1, 0, Format::NoneLoc),
    info("put_loc1", 1, 1, 0, Format::NoneLoc),
    info("put_loc2", 1, 1, 0, Format::NoneLoc),
    info("put_loc3", 1, 1, 0, Format::NoneLoc),
    info("set_loc0", 1, 1, 1, Format::NoneLoc),
    info("set_loc1", 1, 1, 1, Format::NoneLoc),
    info("set_loc2", 1, 1, 1, Format::NoneLoc),
    info("set_loc3", 1, 1, 1, Format::NoneLoc),
    info("get_arg0", 1, 0, 1, Format::NoneArg),
    info("get_arg1", 1, 0, 1, Format::NoneArg),
    info("get_arg2", 1, 0, 1, Format::NoneArg),
    info("get_arg3", 1, 0, 1, Format::NoneArg),
    info("put_arg0", 1, 1, 0, Format::NoneArg),
    info("put_arg1", 1, 1, 0, Format::NoneArg),
    info("put_arg2", 1, 1, 0, Format::NoneArg),
    info("put_arg3", 1, 1, 0, Format::NoneArg),
    info("set_arg0", 1, 1, 1, Format::NoneArg),
    info("set_arg1", 1, 1, 1, Format::NoneArg),
    info("set_arg2", 1, 1, 1, Format::NoneArg),
    info("set_arg3", 1, 1, 1, Format::NoneArg),
    info("get_var_ref0", 1, 0, 1, Format::NoneVarRef),
    info("get_var_ref1", 1, 0, 1, Format::NoneVarRef),
    info("get_var_ref2", 1, 0, 1, Format::NoneVarRef),
    info("get_var_ref3", 1, 0, 1, Format::NoneVarRef),
    info("put_var_ref0", 1, 1, 0, Format::NoneVarRef),
    info("put_var_ref1", 1, 1, 0, Format::NoneVarRef),
    info("put_var_ref2", 1, 1, 0, Format::NoneVarRef),
    info("put_var_ref3", 1, 1, 0, Format::NoneVarRef),
    info("set_var_ref0", 1, 1, 1, Format::NoneVarRef),
    info("set_var_ref1", 1, 1, 1, Format::NoneVarRef),
    info("set_var_ref2", 1, 1, 1, Format::NoneVarRef),
    info("set_var_ref3", 1, 1, 1, Format::NoneVarRef),
    info("get_length", 1, 1, 1, Format::None),
    info("if_false8", 2, 1, 0, Format::Label8),
    info("if_true8", 2, 1, 0, Format::Label8),
    info("goto8", 2, 0, 0, Format::Label8),
    info("goto16", 3, 0, 0, Format::Label16),
    info("call0", 1, 1, 1, Format::Npopx),
    info("call1", 1, 1, 1, Format::Npopx),
    info("call2", 1, 1, 1, Format::Npopx),
    info("call3", 1, 1, 1, Format::Npopx),
    info("is_undefined", 1, 1, 1, Format::None),
    info("is_null", 1, 1, 1, Format::None),
    info("is_function", 1, 1, 1, Format::None),
];
/// Built-in atom strings by index. Entry 0 stands for `JS_ATOM_NULL` and has
/// no string of its own.
pub static BUILTIN_ATOMS: [&str; ATOM_END as usize] = [
    "",                          // 0 JS_ATOM_NULL
    "null",                      // 1 null
    "false",                     // 2 false
    "true",                      // 3 true
    "if",                        // 4 if
    "else",                      // 5 else
    "return",                    // 6 return
    "var",                       // 7 var
    "this",                      // 8 this
    "delete",                    // 9 delete
    "void",                      // 10 void
    "typeof",                    // 11 typeof
    "new",                       // 12 new
    "in",                        // 13 in
    "instanceof",                // 14 instanceof
    "do",                        // 15 do
    "while",                     // 16 while
    "for",                       // 17 for
    "break",                     // 18 break
    "continue",                  // 19 continue
    "switch",                    // 20 switch
    "case",                      // 21 case
    "default",                   // 22 default
    "throw",                     // 23 throw
    "try",                       // 24 try
    "catch",                     // 25 catch
    "finally",                   // 26 finally
    "function",                  // 27 function
    "debugger",                  // 28 debugger
    "with",                      // 29 with
    "class",                     // 30 class
    "const",                     // 31 const
    "enum",                      // 32 enum
    "export",                    // 33 export
    "extends",                   // 34 extends
    "import",                    // 35 import
    "super",                     // 36 super
    "implements",                // 37 implements
    "interface",                 // 38 interface
    "let",                       // 39 let
    "package",                   // 40 package
    "private",                   // 41 private
    "protected",                 // 42 protected
    "public",                    // 43 public
    "static",                    // 44 static
    "yield",                     // 45 yield
    "await",                     // 46 await
    "",                          // 47 empty_string
    "length",                    // 48 length
    "fileName",                  // 49 fileName
    "lineNumber",                // 50 lineNumber
    "message",                   // 51 message
    "stack",                     // 52 stack
    "name",                      // 53 name
    "toString",                  // 54 toString
    "toLocaleString",            // 55 toLocaleString
    "valueOf",                   // 56 valueOf
    "eval",                      // 57 eval
    "prototype",                 // 58 prototype
    "constructor",               // 59 constructor
    "configurable",              // 60 configurable
    "writable",                  // 61 writable
    "enumerable",                // 62 enumerable
    "value",                     // 63 value
    "get",                       // 64 get
    "set",                       // 65 set
    "of",                        // 66 of
    "__proto__",                 // 67 __proto__
    "undefined",                 // 68 undefined
    "number",                    // 69 number
    "boolean",                   // 70 boolean
    "string",                    // 71 string
    "object",                    // 72 object
    "symbol",                    // 73 symbol
    "integer",                   // 74 integer
    "unknown",                   // 75 unknown
    "arguments",                 // 76 arguments
    "callee",                    // 77 callee
    "caller",                    // 78 caller
    "<eval>",                    // 79 _eval_
    "<ret>",                     // 80 _ret_
    "<var>",                     // 81 _var_
    "<with>",                    // 82 _with_
    "lastIndex",                 // 83 lastIndex
    "target",                    // 84 target
    "index",                     // 85 index
    "input",                     // 86 input
    "defineProperties",          // 87 defineProperties
    "apply",                     // 88 apply
    "join",                      // 89 join
    "concat",                    // 90 concat
    "split",                     // 91 split
    "construct",                 // 92 construct
    "getPrototypeOf",            // 93 getPrototypeOf
    "setPrototypeOf",            // 94 setPrototypeOf
    "isExtensible",              // 95 isExtensible
    "preventExtensions",         // 96 preventExtensions
    "has",                       // 97 has
    "deleteProperty",            // 98 deleteProperty
    "defineProperty",            // 99 defineProperty
    "getOwnPropertyDescriptor",  // 100 getOwnPropertyDescriptor
    "ownKeys",                   // 101 ownKeys
    "add",                       // 102 add
    "done",                      // 103 done
    "next",                      // 104 next
    "values",                    // 105 values
    "source",                    // 106 source
    "flags",                     // 107 flags
    "global",                    // 108 global
    "unicode",                   // 109 unicode
    "raw",                       // 110 raw
    "new.target",                // 111 new_target
    "this.active_func",          // 112 this_active_func
    "<home_object>",             // 113 home_object
    "<computed_field>",          // 114 computed_field
    "<static_computed_field>",   // 115 static_computed_field
    "<class_fields_init>",       // 116 class_fields_init
    "<brand>",                   // 117 brand
    "#constructor",              // 118 hash_constructor
    "as",                        // 119 as
    "from",                      // 120 from
    "*default*",                 // 121 _default_
    "*",                         // 122 _star_
    "Module",                    // 123 Module
    "then",                      // 124 then
    "resolve",                   // 125 resolve
    "reject",                    // 126 reject
    "promise",                   // 127 promise
    "proxy",                     // 128 proxy
    "revoke",                    // 129 revoke
    "async",                     // 130 async
    "exec",                      // 131 exec
    "groups",                    // 132 groups
    "status",                    // 133 status
    "reason",                    // 134 reason
    "toJSON",                    // 135 toJSON
    "Object",                    // 136 Object
    "Array",                     // 137 Array
    "Error",                     // 138 Error
    "Number",                    // 139 Number
    "String",                    // 140 String
    "Boolean",                   // 141 Boolean
    "Symbol",                    // 142 Symbol
    "Arguments",                 // 143 Arguments
    "Math",                      // 144 Math
    "JSON",                      // 145 JSON
    "Date",                      // 146 Date
    "Function",                  // 147 Function
    "GeneratorFunction",         // 148 GeneratorFunction
    "ForInIterator",             // 149 ForInIterator
    "RegExp",                    // 150 RegExp
    "ArrayBuffer",               // 151 ArrayBuffer
    "SharedArrayBuffer",         // 152 SharedArrayBuffer
    "Uint8ClampedArray",         // 153 Uint8ClampedArray
    "Int8Array",                 // 154 Int8Array
    "Uint8Array",                // 155 Uint8Array
    "Int16Array",                // 156 Int16Array
    "Uint16Array",               // 157 Uint16Array
    "Int32Array",                // 158 Int32Array
    "Uint32Array",               // 159 Uint32Array
    "Float32Array",              // 160 Float32Array
    "Float64Array",              // 161 Float64Array
    "DataView",                  // 162 DataView
    "Map",                       // 163 Map
    "Set",                       // 164 Set
    "WeakMap",                   // 165 WeakMap
    "WeakSet",                   // 166 WeakSet
    "Map Iterator",              // 167 Map_Iterator
    "Set Iterator",              // 168 Set_Iterator
    "Array Iterator",            // 169 Array_Iterator
    "String Iterator",           // 170 String_Iterator
    "RegExp String Iterator",    // 171 RegExp_String_Iterator
    "Generator",                 // 172 Generator
    "Proxy",                     // 173 Proxy
    "Promise",                   // 174 Promise
    "PromiseResolveFunction",    // 175 PromiseResolveFunction
    "PromiseRejectFunction",     // 176 PromiseRejectFunction
    "AsyncFunction",             // 177 AsyncFunction
    "AsyncFunctionResolve",      // 178 AsyncFunctionResolve
    "AsyncFunctionReject",       // 179 AsyncFunctionReject
    "AsyncGeneratorFunction",    // 180 AsyncGeneratorFunction
    "AsyncGenerator",            // 181 AsyncGenerator
    "EvalError",                 // 182 EvalError
    "RangeError",                // 183 RangeError
    "ReferenceError",            // 184 ReferenceError
    "SyntaxError",               // 185 SyntaxError
    "TypeError",                 // 186 TypeError
    "URIError",                  // 187 URIError
    "InternalError",             // 188 InternalError
    "<brand>",                   // 189 Private_brand
    "Symbol.toPrimitive",        // 190 Symbol_toPrimitive
    "Symbol.iterator",           // 191 Symbol_iterator
    "Symbol.match",              // 192 Symbol_match
    "Symbol.matchAll",           // 193 Symbol_matchAll
    "Symbol.replace",            // 194 Symbol_replace
    "Symbol.search",             // 195 Symbol_search
    "Symbol.split",              // 196 Symbol_split
    "Symbol.toStringTag",        // 197 Symbol_toStringTag
    "Symbol.isConcatSpreadable", // 198 Symbol_isConcatSpreadable
    "Symbol.hasInstance",        // 199 Symbol_hasInstance
    "Symbol.species",            // 200 Symbol_species
    "Symbol.unscopables",        // 201 Symbol_unscopables
    "Symbol.asyncIterator",      // 202 Symbol_asyncIterator
];
