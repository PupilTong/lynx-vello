//! The structure the fork's reader trusts, as the writer guarantees it.
//!
//! The engine reads module bytecode with `JS_READ_OBJ_COMPUTE_STACK_SIZE`,
//! which checks nothing of the structure of its input. These tests hold the
//! writer to what that read assumes: each instruction in its opcode's size,
//! counts that match what follows, and a refusal for each program it could
//! otherwise write wrongly. Placement and module record indices, which the
//! writer leaves to the rules, are in `rules.rs`.

use super::corpus::compiled_programs;
use super::{check, realm};
use crate::bytecode::model::{Encoding, layout};
use crate::bytecode::{
    Atom, ClosureType, ClosureVar, Function, FunctionKind, Instruction, JsString, Operand,
    OperandKind, Program, Value, VarDef, read_program, write_program,
};
use crate::bytecode_abi::{JS_ATOM_END, OP_COUNT, OPCODES, atom_int, op};
use crate::{Context, Runtime};

/// An instruction of `opcode` with operands of the right kinds: implicit
/// ones as the opcode says, the others small, jumps to instruction 0.
fn sample(opcode: u8) -> Instruction {
    let operands: Vec<Operand> = layout(opcode)
        .iter()
        .map(|&(kind, encoding)| match (kind, encoding) {
            (OperandKind::Local, Encoding::Implicit(n)) => Operand::Local(n as u16),
            (OperandKind::Arg, Encoding::Implicit(n)) => Operand::Arg(n as u16),
            (OperandKind::ClosureVar, Encoding::Implicit(n)) => Operand::ClosureVar(n as u16),
            (_, Encoding::Implicit(n)) => Operand::Immediate(n),
            (OperandKind::Atom, _) => Operand::Atom(Atom::Builtin(1)),
            (OperandKind::Local, _) => Operand::Local(1),
            (OperandKind::Arg, _) => Operand::Arg(1),
            (OperandKind::ClosureVar, _) => Operand::ClosureVar(1),
            (OperandKind::Const, _) => Operand::Const(1),
            (OperandKind::Target, _) => Operand::Target(0),
            (OperandKind::Immediate, _) => Operand::Immediate(1),
        })
        .collect();
    Instruction::new(opcode, &operands)
        .unwrap_or_else(|error| panic!("{}: {error}", OPCODES[usize::from(opcode)].name))
}

fn script(function: Function) -> Program {
    Program::new(Value::Function(Box::new(function)))
}

/// The root function of bytes the reader read.
fn root(program: &Program) -> &Function {
    let Value::Function(function) = &program.root else {
        panic!("a script")
    };
    function
}

#[test]
fn every_opcode_is_written_in_its_size() {
    let mut total = 0usize;
    for opcode in 1..u8::try_from(OP_COUNT).unwrap() {
        let instruction = sample(opcode);
        let mut function = Function::new(FunctionKind::Normal);
        function.instructions = vec![instruction, Instruction::simple(op::RETURN).unwrap()];
        let bytes = write_program(&script(function.clone())).unwrap_or_else(|error| {
            panic!("{}: {error}", instruction.info().name);
        });
        // The reader steps through the code by the same table and refuses
        // an instruction that runs past the end or a jump into an operand:
        // it reads back exactly the two instructions.
        let program = read_program(&bytes)
            .unwrap_or_else(|error| panic!("{}: {error}", instruction.info().name));
        assert_eq!(
            root(&program).instructions,
            function.instructions,
            "{}",
            instruction.info().name
        );
        total += usize::from(instruction.size());
    }
    assert!(total > OP_COUNT, "the table has multi-byte instructions");
}

#[test]
fn the_writer_refuses_what_the_fork_reader_would_misread() {
    let ret = Instruction::simple(op::RETURN_UNDEF).unwrap();
    let base = || {
        let mut function = Function::new(FunctionKind::Normal);
        function.instructions = vec![ret];
        function
    };
    let refused = |program: &Program, what: &str| {
        let error = write_program(program).expect_err(what);
        println!("{what}: {error}");
    };

    // A function the reader would allocate no code for.
    refused(
        &script(Function::new(FunctionKind::Normal)),
        "no instructions",
    );

    // Vardefs other than none or `arg_count + var_count`: the engine frees
    // `arg_count + var_count` of them.
    let mut function = base();
    function.var_count = 1;
    function.vardefs = vec![VarDef::new(Atom::NULL), VarDef::new(Atom::NULL)];
    refused(&script(function), "two vardefs for one variable");

    // Atoms the reader would index its atom table, or the runtime's, with.
    for atom in [
        Atom::File(0),
        Atom::Builtin(JS_ATOM_END),
        Atom::Int(atom_int::MAX_INT + 1),
    ] {
        let mut function = base();
        function.name = atom;
        refused(&script(function), &format!("record atom {atom:?}"));
        let mut function = base();
        function.instructions = vec![
            Instruction::new(op::PUSH_ATOM_VALUE, &[Operand::Atom(atom)]).unwrap(),
            Instruction::simple(op::RETURN).unwrap(),
        ];
        refused(&script(function), &format!("operand atom {atom:?}"));
    }

    // An operand changed to another kind, or beyond its encoding, after
    // the instruction was made.
    let mut function = base();
    let mut get = Instruction::new(op::GET_LOC, &[Operand::Local(0)]).unwrap();
    get.operands_mut()[0] = Operand::Const(0);
    function.instructions = vec![get, ret];
    refused(&script(function), "an operand of another kind");
    let mut function = base();
    let mut get = Instruction::new(op::GET_LOC8, &[Operand::Local(0)]).unwrap();
    get.operands_mut()[0] = Operand::Local(256);
    function.instructions = vec![get, ret];
    refused(&script(function), "an operand beyond its encoding");

    // A jump to no instruction, and a short jump out of its range.
    let mut function = base();
    function.instructions = vec![
        Instruction::new(op::GOTO, &[Operand::Target(2)]).unwrap(),
        ret,
    ];
    refused(&script(function), "a jump past the end");
    let mut function = base();
    let mut code = vec![Instruction::new(op::GOTO8, &[Operand::Target(200)]).unwrap()];
    code.extend(std::iter::repeat_n(
        Instruction::simple(op::NOP).unwrap(),
        200,
    ));
    code.push(ret);
    function.instructions = code;
    refused(&script(function), "a short jump out of range");

    // Four-bit fields above four bits.
    let mut function = base();
    function.var_count = 1;
    let mut vardef = VarDef::new(Atom::NULL);
    vardef.var_kind = 0x10;
    function.vardefs = vec![vardef];
    refused(&script(function), "a vardef var_kind above four bits");
    let mut function = base();
    let mut closure_var = ClosureVar::new(Atom::NULL, ClosureType::Global, 0);
    closure_var.var_kind = 0x10;
    function.closure_vars = vec![closure_var];
    refused(
        &script(function),
        "a closure variable var_kind above four bits",
    );

    // Values nested deeper than the reader's limit.
    let mut value = Value::Null;
    for _ in 0..=crate::bytecode::NESTING_LIMIT {
        value = Value::Array(vec![value]);
    }
    refused(&Program::new(value), "values nested too deep");
}

fn new_realm() -> Context {
    let runtime = Box::leak(Box::new(Runtime::new().unwrap()));
    runtime.create_context().unwrap()
}

/// What the writer writes for a program the rules accept reads back with
/// the reader for untrusted bytes, which checks every count, length, tag,
/// opcode and instruction bound against the input, and writes again to the
/// same bytes. The mutation run (`fuzz.rs`) asserts the same of every
/// mutant the rules accept.
#[test]
fn what_the_writer_writes_for_accepted_programs_reads_back() {
    let (_runtime, checker) = realm();
    let mut programs = compiled_programs(&new_realm);
    // A program built through the model rather than read: strings of both
    // kinds, a BigInt, a template object and a nested function.
    let mut built = Function::new(FunctionKind::Normal);
    built.instructions = vec![Instruction::simple(op::RETURN_UNDEF).unwrap()];
    let mut root = built.clone();
    root.cpool = vec![
        Value::String(JsString::Utf16(vec![0x00e9, 0xd800])),
        Value::String(JsString::from_str("plain")),
        Value::BigInt(vec![0xff, 0x7f, 1, 2, 3, 4, 5, 6, 7]),
        Value::TemplateObject {
            elements: vec![Value::String(JsString::from_str("a"))],
            raw: Box::new(Value::Array(vec![Value::String(JsString::from_str("a"))])),
        },
        Value::Function(Box::new(built)),
    ];
    programs.push(("built".to_owned(), write_program(&script(root)).unwrap()));
    for (name, bytes) in &programs {
        let program = read_program(bytes).unwrap();
        check(&program, &checker).unwrap_or_else(|error| panic!("{name}: {error}"));
        let written = write_program(&program).unwrap();
        let again = read_program(&written).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(write_program(&again).unwrap() == written, "{name}");
    }
}
