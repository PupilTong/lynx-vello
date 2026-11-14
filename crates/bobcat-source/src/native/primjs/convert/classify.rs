//! Which `PrimJS` opcodes the converter copies, which it rewrites and which it
//! refuses. Every one of the 242 shippable opcodes is in exactly one of the
//! three lists below; the converter looks an opcode up here and treats an
//! opcode in none of them as refused, and a test checks the lists against
//! both opcode tables.

use std::sync::OnceLock;

use quickjs_rust_bridge::bytecode_abi::OPCODES;

use super::super::tables::{ALL_OPCODES, OPCODE_COUNT, Opcode};

/// How the converter handles one `PrimJS` opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpcodeClass {
    /// The fork has an opcode of the same name with the same pops, pushes
    /// and operand format; the instruction is copied with its operands
    /// mapped (atoms re-interned, jump targets re-addressed).
    Same(u8),
    /// The converter writes a fork sequence for it; the text says which.
    Translated(&'static str),
    /// The converter refuses it; the text says why.
    Rejected(&'static str),
}

/// Opcodes copied by name. The test `opcode_classes` checks that the fork
/// has each name with the same stack effect and operand format.
pub const SAME: &[Opcode] = &[
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
    Opcode::Dup,
    Opcode::Dup1,
    Opcode::Dup2,
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
    Opcode::GetSuper,
    Opcode::GetRefValue,
    Opcode::PutRefValue,
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
    Opcode::WithGetVar,
    Opcode::WithPutVar,
    Opcode::WithDeleteVar,
    Opcode::WithMakeRef,
    Opcode::WithGetRef,
    Opcode::MakeLocRef,
    Opcode::MakeArgRef,
    Opcode::MakeVarRefRef,
    Opcode::MakeVarRef,
    Opcode::ForInStart,
    Opcode::ForOfStart,
    Opcode::ForAwaitOfStart,
    Opcode::ForInNext,
    Opcode::ForOfNext,
    Opcode::IteratorClose,
    Opcode::InitialYield,
    Opcode::Yield,
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
    Opcode::Call0,
    Opcode::Call1,
    Opcode::Call2,
    Opcode::Call3,
    Opcode::IsUndefined,
    Opcode::IsNull,
];

/// Opcodes the converter rewrites, with what each becomes.
pub const TRANSLATED: &[(Opcode, &str)] = &[
    (
        Opcode::GetVar,
        "get_var of the name's closure variable (a global), or get_var_ref_check (an import or \
         a name declared undefined)",
    ),
    (
        Opcode::GetVarUndef,
        "get_var_undef of the name's closure variable, or get_var_ref_check",
    ),
    (
        Opcode::PutVar,
        "put_var of the name's closure variable, or throw_error VAR_RO for an import or a name \
         declared undefined",
    ),
    (
        Opcode::PutVarInit,
        "put_var_init of a name the script declares",
    ),
    (
        Opcode::CheckVar,
        "undefined: keeps the slot put_var_strict consumes; the fork's put_var makes the \
         strict-mode existence check",
    ),
    (
        Opcode::PutVarStrict,
        "nip; put_var (or nip; throw_error VAR_RO for an import or a name declared undefined)",
    ),
    (
        Opcode::DefineVar,
        "nothing: the declaration is a GLOBAL_DECL closure variable of the module function",
    ),
    (
        Opcode::CheckDefineVar,
        "nothing: the fork checks the declaration when the module links",
    ),
    (
        Opcode::DefineFunc,
        "put_var_ref of the GLOBAL_DECL closure variable (GLOBAL_FUNCTION_DECL)",
    ),
    (Opcode::ThrowVar, "throw_error with the same atom and type"),
    (
        Opcode::GetSuperCtor,
        "get_super: the fork checks for a constructor when it constructs",
    ),
    (
        Opcode::ToPropkey2,
        "swap; dup; is_undefined_or_null; if_false L; get_length; L: swap; to_propkey",
    ),
    (Opcode::IsFunction, "typeof_is_function"),
    (
        Opcode::IteratorGetValueDone,
        "iterator_check_object; get_field2 done; swap; get_field value; swap (inside the \
         for-await pattern: the fork's iterator_get_value_done)",
    ),
    (Opcode::IteratorCloseReturn, "nip_catch; rot3r; undefined"),
    (
        Opcode::AsyncIteratorClose,
        "drop; drop; get_field2 return; dup; is_undefined_or_null; if_true A; call_method 0; \
         push_false; goto B; A: drop; drop; undefined; push_true; B:",
    ),
    (Opcode::AsyncIteratorNext, "iterator_next"),
    (
        Opcode::AsyncIteratorGet,
        "iterator_call 0 or 1; flag 2 is throw_error ITERATOR_THROW",
    ),
    (
        Opcode::YieldStar,
        "the fork's delegation loop, entered with [iterator, next] and left with [value, \
         is_return]",
    ),
    (
        Opcode::Regexp,
        "regexp after push_const pattern and push_const of the fork's bytecode for the pattern \
         and the flags of the PrimJS bytecode's header",
    ),
    (
        Opcode::Dup3,
        "dup3; inside `dup3; drop; call_method 0; await; iterator_get_value_done` (a for-await \
         step) the fork's `for_await_of_next; await; iterator_get_value_done`",
    ),
    (Opcode::IfFalse8, "if_false"),
    (Opcode::IfTrue8, "if_true"),
    (Opcode::Goto8, "goto"),
    (Opcode::Goto16, "goto"),
];

/// Opcodes the converter refuses, with the reason.
pub const REJECTED: &[(Opcode, &str)] = &[
    (Opcode::Invalid, "not an instruction"),
    (
        Opcode::Eval,
        "direct eval: the fork's eval takes a different stack and compiles against the \
         enclosing function's scope metadata in its own encoding",
    ),
    (
        Opcode::WithGetRefUndef,
        "exists only for the variable object of a function with a direct eval",
    ),
    (
        Opcode::Import,
        "dynamic import(): no corpus program exercises it",
    ),
    (Opcode::Nip1, "PrimJS's compiler never emits it"),
    (
        Opcode::ForAwaitOfNext,
        "PrimJS's compiler never emits it (for-await calls next with dup3; drop; call_method 0)",
    ),
    (
        Opcode::Nop,
        "PrimJS's compiler removes every nop before writing",
    ),
];

/// The class of `opcode`.
#[must_use]
pub fn opcode_class(opcode: Opcode) -> OpcodeClass {
    table()[usize::from(opcode.number())]
}

fn table() -> &'static [OpcodeClass; OPCODE_COUNT] {
    static TABLE: OnceLock<[OpcodeClass; OPCODE_COUNT]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [OpcodeClass::Rejected("not classified"); OPCODE_COUNT];
        for opcode in ALL_OPCODES {
            let index = usize::from(opcode.number());
            if SAME.contains(&opcode) {
                table[index] = fork_opcode(opcode.name()).map_or(
                    OpcodeClass::Rejected("the fork has no opcode of this name"),
                    OpcodeClass::Same,
                );
            } else if let Some((_, what)) = TRANSLATED.iter().find(|(op, _)| *op == opcode) {
                table[index] = OpcodeClass::Translated(what);
            } else if let Some((_, why)) = REJECTED.iter().find(|(op, _)| *op == opcode) {
                table[index] = OpcodeClass::Rejected(why);
            }
        }
        table
    })
}

/// The fork's opcode byte for `name`.
pub fn fork_opcode(name: &str) -> Option<u8> {
    OPCODES
        .iter()
        .position(|entry| entry.name == name)
        .and_then(|index| u8::try_from(index).ok())
}
