//! Each rule refuses a program built through the model that breaks only
//! that rule, and programs built through the model that break none load
//! and run.

use super::realm;
use crate::bytecode::Operand::{Arg, Atom as A, ClosureVar, Const, Immediate, Local, Target};
use crate::bytecode::{
    Atom, ClosureType, ClosureVar as Cv, Export, Function, FunctionKind, Import, Instruction,
    JsString, Module, Operand, Program, RegExpCompiler, Rule, Value, VarDef, verify, write_program,
};
use crate::bytecode_abi::{js_mode, op};
use crate::{Context, EvalOptions, EvalSource};

/// A built-in atom to name variables with.
fn named() -> Atom {
    Atom::Builtin(crate::bytecode_abi::builtin_string_atom("value").unwrap())
}

fn i(opcode: u8, operands: &[Operand]) -> Instruction {
    Instruction::new(opcode, operands).unwrap()
}

fn s(opcode: u8) -> Instruction {
    Instruction::simple(opcode).unwrap()
}

fn function(kind: FunctionKind, code: Vec<Instruction>) -> Function {
    let mut function = Function::new(kind);
    function.instructions = code;
    function
}

fn script(root: Function) -> Program {
    Program::new(Value::Function(Box::new(root)))
}

/// A module whose evaluated body is `body` (which ends the function), after
/// the compiler's linking prologue.
fn module(program_atoms: &mut Program, body: Vec<Instruction>, closure_vars: Vec<Cv>) -> Module {
    let name = program_atoms.intern_str("app:///built.js");
    let mut code = vec![
        s(op::PUSH_THIS),
        i(op::IF_FALSE8, &[Target(3)]),
        s(op::RETURN_UNDEF),
    ];
    for mut instruction in body {
        for operand in instruction.operands_mut() {
            if let Target(target) = operand {
                *target += 3;
            }
        }
        code.push(instruction);
    }
    let mut function = function(FunctionKind::Async, code);
    function.js_mode = js_mode::STRICT;
    function.closure_vars = closure_vars;
    Module {
        name,
        requests: Vec::new(),
        exports: Vec::new(),
        star_exports: Vec::new(),
        imports: Vec::new(),
        has_tla: false,
        function,
    }
}

fn module_program(build: impl FnOnce(&mut Program) -> Module) -> Program {
    let mut program = Program::new(Value::Undefined);
    let module = build(&mut program);
    program.root = Value::Module(Box::new(module));
    program
}

fn verdict(program: &Program, realm: &Context) -> Result<(), Rule> {
    verify(program, realm as &dyn RegExpCompiler).map_err(|error| error.rule)
}

fn refused(program: &Program, rule: Rule) {
    let (_runtime, realm) = realm();
    let error =
        verify(program, &realm as &dyn RegExpCompiler).expect_err("the program breaks a rule");
    assert_eq!(error.rule, rule, "{error}");
    // Nothing refused reaches the engine through the checked entries.
    let bytes = write_program(program).unwrap();
    assert!(realm.verify_bytecode(&bytes).is_err());
}

fn returns_undefined() -> Vec<Instruction> {
    vec![s(op::UNDEFINED), s(op::RETURN)]
}

fn with_tail(mut code: Vec<Instruction>) -> Vec<Instruction> {
    code.extend(returns_undefined());
    code
}

// ---------------------------------------------------------------------------
// Programs that break no rule.

#[test]
fn programs_built_through_the_model_load_and_run() {
    let (mut runtime, mut realm) = realm();

    // `globalThis.built = 40 + 2`, as a script.
    let mut program = script(Function::new(FunctionKind::Normal));
    let global = program.intern_str("globalThis");
    let built = program.intern_str("built");
    let Value::Function(root) = &mut program.root else {
        unreachable!()
    };
    root.closure_vars = vec![Cv::new(global, ClosureType::Global, 0)];
    root.instructions = vec![
        i(op::GET_VAR, &[ClosureVar(0)]),
        i(op::PUSH_I8, &[Immediate(40)]),
        s(op::PUSH_2),
        s(op::ADD),
        i(op::PUT_FIELD, &[A(built)]),
        s(op::UNDEFINED),
        s(op::RETURN),
    ];
    assert_eq!(verdict(&program, &realm), Ok(()));
    let bytes = write_program(&program).unwrap();
    realm
        .evaluate_script_bytecode(&realm.verify_bytecode(&bytes).unwrap())
        .unwrap();
    let value = realm
        .evaluate(EvalSource::new("globalThis.built"), EvalOptions::default())
        .unwrap();
    assert_eq!(value.as_number(), Some(42.0));

    // `export const answer = /b+/.exec('abbc')[0].length`, as a module with
    // a regular expression literal.
    let regexp = realm
        .compile_regexp_bytecode(&"b+".encode_utf16().collect::<Vec<_>>(), "")
        .unwrap();
    let program = module_program(|program| {
        let answer = program.intern_str("answer");
        let exec = program.intern_str("exec");
        let mut module = module(
            program,
            vec![
                i(op::PUSH_CONST8, &[Const(0)]),
                i(op::PUSH_CONST8, &[Const(1)]),
                s(op::REGEXP),
                i(op::GET_FIELD2, &[A(exec)]),
                i(op::PUSH_CONST8, &[Const(2)]),
                i(op::CALL_METHOD, &[Immediate(1)]),
                i(op::GET_ARRAY_EL, &[]),
                s(op::GET_LENGTH),
                i(op::PUT_VAR_REF0, &[ClosureVar(0)]),
                s(op::UNDEFINED),
                s(op::RETURN_ASYNC),
            ]
            .into_iter()
            .enumerate()
            .flat_map(|(index, instruction)| {
                // `[0]`: push the index before `get_array_el`.
                if index == 6 {
                    vec![s(op::PUSH_0), instruction]
                } else {
                    vec![instruction]
                }
            })
            .collect(),
            vec![Cv::new(answer, ClosureType::ModuleDecl, 0)],
        );
        module.function.cpool = vec![
            Value::String(JsString::from_str("b+")),
            Value::String(JsString::Latin1(regexp)),
            Value::String(JsString::from_str("abbc")),
        ];
        module.exports = vec![Export::Local {
            var_idx: 0,
            name: answer,
        }];
        module
    });
    assert_eq!(verdict(&program, &realm), Ok(()));
    let name = "app:///built.js";
    let module = crate::bytecode::Verifier::new().module(&program).unwrap();
    assert_eq!(module.bytes(), write_program(&program).unwrap().as_slice());
    realm.complete_module_bytecode(name, name, &module).unwrap();
    let promise = realm.load_module(name).unwrap();
    runtime.drain_pending_jobs(&realm).unwrap();
    realm.settled_promise_result(&promise).unwrap().unwrap();
    let namespace = realm.module_namespace(name).unwrap();
    let answer = realm.property(&namespace, "answer").unwrap();
    assert_eq!(answer.as_number(), Some(2.0));
}

// ---------------------------------------------------------------------------
// One rule each.

#[test]
fn value_placement() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.cpool = vec![Value::Array(vec![Value::Function(Box::new(function(
        FunctionKind::Normal,
        returns_undefined(),
    )))])];
    refused(&script(root), Rule::ValuePlacement);
}

#[test]
fn atom_range() {
    let root = function(
        FunctionKind::Normal,
        with_tail(vec![
            s(op::OBJECT),
            i(op::GET_FIELD, &[A(Atom::File(5))]),
            s(op::DROP),
        ]),
    );
    let program = script(root);
    let (_runtime, realm) = realm();
    assert_eq!(verdict(&program, &realm), Err(Rule::AtomRange));
}

#[test]
fn null_atom() {
    let root = function(
        FunctionKind::Normal,
        with_tail(vec![
            s(op::OBJECT),
            i(op::GET_FIELD, &[A(Atom::NULL)]),
            s(op::DROP),
        ]),
    );
    refused(&script(root), Rule::NullAtom);
}

#[test]
fn private_symbol_atom() {
    let root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::PRIVATE_SYMBOL, &[A(Atom::Int(3))]), s(op::DROP)]),
    );
    refused(&script(root), Rule::PrivateSymbolAtom);
}

#[test]
fn vardef_count() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.var_count = 1;
    root.vardefs = vec![VarDef::new(Atom::NULL), VarDef::new(Atom::NULL)];
    let (_runtime, realm) = realm();
    // The writer refuses to write it, so only the model can hold it.
    assert_eq!(verdict(&script(root), &realm), Err(Rule::VardefCount));
}

#[test]
fn defined_arg_count() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.defined_arg_count = 1;
    refused(&script(root), Rule::DefinedArgCount);
}

#[test]
fn js_mode_async_bit() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.js_mode = js_mode::ASYNC;
    refused(&script(root), Rule::JsMode);
}

#[test]
fn var_kind() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.var_count = 1;
    let mut vardef = VarDef::new(Atom::NULL);
    vardef.var_kind = 11;
    root.vardefs = vec![vardef];
    refused(&script(root), Rule::VarKind);
}

#[test]
fn var_ref_index() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.var_count = 1;
    let mut vardef = VarDef::new(Atom::NULL);
    vardef.is_captured = true;
    root.vardefs = vec![vardef];
    refused(&script(root), Rule::VarRefIndex);
}

#[test]
fn closure_type() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.closure_vars = vec![Cv::new(named(), ClosureType::Local, 0)];
    refused(&script(root), Rule::ClosureType);
}

#[test]
fn closure_parent() {
    let mut child = function(FunctionKind::Normal, returns_undefined());
    child.closure_vars = vec![Cv::new(named(), ClosureType::Local, 0)];
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::FCLOSURE8, &[Const(0)]), s(op::DROP)]),
    );
    root.var_count = 1;
    root.vardefs = vec![VarDef::new(Atom::NULL)];
    root.cpool = vec![Value::Function(Box::new(child))];
    refused(&script(root), Rule::ClosureParent);
}

#[test]
fn module_record() {
    let program = module_program(|program| {
        let x = program.intern_str("x");
        module(
            program,
            vec![s(op::UNDEFINED), s(op::RETURN_ASYNC)],
            vec![Cv::new(x, ClosureType::ModuleImport, 0)],
        )
    });
    refused(&program, Rule::ModuleRecord);
}

#[test]
fn module_function() {
    let program = module_program(|program| {
        let mut module = module(
            program,
            vec![s(op::UNDEFINED), s(op::RETURN_ASYNC)],
            Vec::new(),
        );
        module.function.flags.kind = FunctionKind::Normal;
        module
    });
    refused(&program, Rule::ModuleFunction);
}

#[test]
fn class_constructor() {
    let constructor = function(
        FunctionKind::Generator,
        vec![s(op::INITIAL_YIELD), s(op::UNDEFINED), s(op::RETURN_ASYNC)],
    );
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![
            s(op::UNDEFINED),
            i(op::PUSH_CONST8, &[Const(0)]),
            i(op::DEFINE_CLASS, &[A(Atom::NULL), Immediate(0)]),
            s(op::DROP),
            s(op::DROP),
        ]),
    );
    root.cpool = vec![Value::Function(Box::new(constructor))];
    refused(&script(root), Rule::ClassConstructor);
}

#[test]
fn operand_range() {
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::GET_LOC, &[Local(3)]), s(op::DROP)]),
    );
    root.var_count = 1;
    refused(&script(root), Rule::OperandRange);
    let root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::GET_ARG, &[Arg(0)]), s(op::DROP)]),
    );
    refused(&script(root), Rule::OperandRange);
}

#[test]
fn enum_operand() {
    let root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::SPECIAL_OBJECT, &[Immediate(9)]), s(op::DROP)]),
    );
    refused(&script(root), Rule::EnumOperand);
}

#[test]
fn closure_constant() {
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::FCLOSURE8, &[Const(0)]), s(op::DROP)]),
    );
    root.cpool = vec![Value::Int32(1)];
    refused(&script(root), Rule::ClosureConstant);
}

fn regexp_root(realm: &Context, code: Vec<Instruction>) -> Function {
    let bytes = realm
        .compile_regexp_bytecode(&"a+".encode_utf16().collect::<Vec<_>>(), "g")
        .unwrap();
    let mut root = function(FunctionKind::Normal, code);
    root.cpool = vec![
        Value::String(JsString::from_str("a+")),
        Value::String(JsString::Latin1(bytes)),
    ];
    root
}

#[test]
fn regexp() {
    let (_runtime, realm) = realm();
    let literal = || {
        vec![
            i(op::PUSH_CONST8, &[Const(0)]),
            i(op::PUSH_CONST8, &[Const(1)]),
            s(op::REGEXP),
            s(op::RETURN),
        ]
    };
    assert_eq!(
        verdict(&script(regexp_root(&realm, literal())), &realm),
        Ok(())
    );

    // Bytecode that is not the engine's for the pattern and flags.
    let mut root = regexp_root(&realm, literal());
    let Value::String(JsString::Latin1(bytes)) = &mut root.cpool[1] else {
        unreachable!()
    };
    *bytes.last_mut().unwrap() ^= 1;
    refused(&script(root), Rule::RegExp);

    // Flags the header claims and the bytecode does not carry.
    let mut root = regexp_root(&realm, literal());
    let Value::String(JsString::Latin1(bytes)) = &mut root.cpool[1] else {
        unreachable!()
    };
    bytes[0] ^= 2;
    refused(&script(root), Rule::RegExp);

    // Not directly after two pushed strings.
    let mut code = literal();
    code[0] = s(op::PUSH_1);
    refused(&script(regexp_root(&realm, code)), Rule::RegExp);

    // A jump onto the second constant.
    let mut code = literal();
    code.push(i(op::GOTO8, &[Target(1)]));
    refused(&script(regexp_root(&realm, code)), Rule::RegExp);
}

#[test]
fn var_ref_capture() {
    let mut program = script(Function::new(FunctionKind::Normal));
    let x = program.intern_str("x");
    let Value::Function(root) = &mut program.root else {
        unreachable!()
    };
    root.var_count = 1;
    root.vardefs = vec![VarDef::new(x)];
    root.instructions = with_tail(vec![
        i(op::MAKE_LOC_REF, &[A(x), Local(0)]),
        s(op::DROP),
        s(op::DROP),
    ]);
    refused(&program, Rule::VarRefCapture);
}

#[test]
fn uninitialized() {
    let mut root = function(
        FunctionKind::Normal,
        vec![
            i(op::SET_LOC_UNINITIALIZED, &[Local(0)]),
            i(op::GET_LOC, &[Local(0)]),
            s(op::RETURN),
        ],
    );
    root.var_count = 1;
    refused(&script(root), Rule::Uninitialized);

    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![
            i(op::SET_LOC_UNINITIALIZED, &[Local(0)]),
            i(op::INC_LOC, &[Local(0)]),
        ]),
    );
    root.var_count = 1;
    refused(&script(root), Rule::Uninitialized);

    // A child reading the local without a check.
    let mut child = function(
        FunctionKind::Normal,
        vec![s(op::GET_VAR_REF0), s(op::RETURN)],
    );
    child.closure_vars = vec![Cv::new(named(), ClosureType::Local, 0)];
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![
            i(op::SET_LOC_UNINITIALIZED, &[Local(0)]),
            i(op::FCLOSURE8, &[Const(0)]),
            s(op::DROP),
        ]),
    );
    root.var_count = 1;
    root.var_ref_count = 1;
    let mut vardef = VarDef::new(Atom::NULL);
    vardef.is_captured = true;
    vardef.is_lexical = true;
    root.vardefs = vec![vardef];
    root.cpool = vec![Value::Function(Box::new(child))];
    refused(&script(root), Rule::Uninitialized);
}

#[test]
fn eval_scope() {
    let mut root = function(
        FunctionKind::Normal,
        vec![
            s(op::UNDEFINED),
            s(op::UNDEFINED),
            // Scope operand 2 is local 0 (`ARG_SCOPE_END` is -2).
            i(op::EVAL, &[Immediate(1), Immediate(2)]),
            s(op::RETURN),
        ],
    );
    root.var_count = 1;
    root.var_ref_count = 1;
    let mut vardef = VarDef::new(Atom::NULL);
    vardef.is_captured = true;
    vardef.has_scope = true;
    // The scope chain loops on itself.
    vardef.scope_next = 0;
    root.vardefs = vec![vardef];
    refused(&script(root), Rule::EvalScope);
}

#[test]
fn module_this_func() {
    let program = module_program(|program| {
        module(
            program,
            vec![
                i(op::SPECIAL_OBJECT, &[Immediate(2)]),
                s(op::DROP),
                s(op::UNDEFINED),
                s(op::RETURN_ASYNC),
            ],
            Vec::new(),
        )
    });
    refused(&program, Rule::ModuleThisFunc);
}

#[test]
fn stack_depth() {
    refused(
        &script(function(FunctionKind::Normal, with_tail(vec![s(op::DROP)]))),
        Rule::StackDepth,
    );
    // `for_of_next` addresses a triple below what the stack holds.
    refused(
        &script(function(
            FunctionKind::Normal,
            with_tail(vec![
                s(op::PUSH_0),
                s(op::PUSH_0),
                s(op::PUSH_0),
                i(op::FOR_OF_NEXT, &[Immediate(3)]),
            ]),
        )),
        Rule::StackDepth,
    );
}

#[test]
fn merge() {
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                s(op::PUSH_1),
                i(op::IF_FALSE8, &[Target(3)]),
                s(op::PUSH_1),
                s(op::UNDEFINED),
                s(op::RETURN),
            ],
        )),
        Rule::Merge,
    );
}

#[test]
fn fork_walk() {
    // A marker moved above the slot `for_of_start` pushed it to: the fork's
    // walk computes `nip_catch`'s depth from where it was pushed.
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                s(op::PUSH_0),
                s(op::PUSH_0),
                s(op::OBJECT),
                s(op::FOR_OF_START),
                s(op::PUSH_0),
                s(op::SWAP),
                s(op::PUSH_1),
                s(op::NIP_CATCH),
                s(op::RETURN),
            ],
        )),
        Rule::ForkWalk,
    );
}

#[test]
fn special_slot() {
    // A catch offset copied.
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                i(op::CATCH, &[Target(3)]),
                s(op::DUP),
                s(op::RETURN),
                s(op::RETURN),
            ],
        )),
        Rule::SpecialSlot,
    );
    // A return address returned as a value.
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                i(op::GOSUB, &[Target(3)]),
                s(op::UNDEFINED),
                s(op::RETURN),
                s(op::RETURN),
            ],
        )),
        Rule::SpecialSlot,
    );
    // Function bytecode pushed as a value.
    let mut root = function(
        FunctionKind::Normal,
        vec![i(op::PUSH_CONST8, &[Const(0)]), s(op::RETURN)],
    );
    root.cpool = vec![Value::Function(Box::new(function(
        FunctionKind::Normal,
        returns_undefined(),
    )))];
    refused(&script(root), Rule::SpecialSlot);
}

#[test]
fn iterator_slots() {
    refused(
        &script(function(
            FunctionKind::Normal,
            with_tail(vec![
                s(op::PUSH_0),
                s(op::PUSH_0),
                s(op::PUSH_0),
                i(op::FOR_OF_NEXT, &[Immediate(0)]),
            ]),
        )),
        Rule::IteratorSlots,
    );
}

/// Found by execution: `js_for_await_of_next` overwrites the slot it
/// clears without freeing it, so an object there leaked and the runtime
/// aborted when it was freed (`JS_FreeRuntime`'s leak assertion).
#[test]
fn for_await_of_next_over_a_value_regression() {
    refused(
        &script(function(
            FunctionKind::Normal,
            with_tail(vec![
                s(op::OBJECT),
                s(op::OBJECT),
                s(op::OBJECT),
                s(op::FOR_AWAIT_OF_NEXT),
                s(op::DROP),
                s(op::DROP),
                s(op::DROP),
                s(op::DROP),
            ]),
        )),
        Rule::IteratorSlots,
    );
}

#[test]
fn nip_catch() {
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                s(op::PUSH_1),
                s(op::PUSH_1),
                s(op::NIP_CATCH),
                s(op::RETURN),
            ],
        )),
        Rule::NipCatch,
    );
}

#[test]
fn ret() {
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![s(op::PUSH_1), s(op::RET)],
        )),
        Rule::Ret,
    );
    // The return address below the top. (A stack that differs below the
    // address cannot be built: nothing may reach under it.)
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                i(op::GOSUB, &[Target(3)]),
                s(op::UNDEFINED),
                s(op::RETURN),
                s(op::PUSH_1),
                s(op::RET),
            ],
        )),
        Rule::Ret,
    );
}

#[test]
fn catch_target() {
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                i(op::CATCH, &[Target(0)]),
                s(op::DROP),
                s(op::UNDEFINED),
                s(op::RETURN),
            ],
        )),
        Rule::BackwardTarget,
    );
}

/// Found by execution: a handler before its `catch` makes a loop that
/// throws and catches without passing a `goto`, so the execution limit
/// never fires.
#[test]
fn backward_target_regression() {
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                s(op::PUSH_0),
                s(op::DROP),
                i(op::CATCH, &[Target(1)]),
                i(op::THROW_ERROR, &[A(Atom::NULL), Immediate(0)]),
            ],
        )),
        Rule::BackwardTarget,
    );
    // A `gosub` back to a block that drops the address and runs it again.
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![
                i(op::GOTO8, &[Target(2)]),
                s(op::DROP),
                i(op::GOSUB, &[Target(1)]),
            ],
        )),
        Rule::BackwardTarget,
    );
}

/// Found by execution: `define_class` without its heritage flag
/// overwrites `sp[-2]` without freeing it, so a parent class left there
/// leaked and the runtime aborted when it was freed.
#[test]
fn define_class_without_heritage_over_a_value_regression() {
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![
            s(op::OBJECT),
            i(op::PUSH_CONST8, &[Const(0)]),
            i(op::DEFINE_CLASS, &[A(Atom::NULL), Immediate(0)]),
            s(op::DROP),
            s(op::DROP),
        ]),
    );
    root.cpool = vec![Value::Function(Box::new(function(
        FunctionKind::Normal,
        returns_undefined(),
    )))];
    refused(&script(root.clone()), Rule::SpecialSlot);
    // With the flag the parent is taken and freed.
    let Value::Function(_) = &root.cpool[0] else {
        unreachable!()
    };
    root.instructions[2] = i(op::DEFINE_CLASS, &[A(Atom::NULL), Immediate(1)]);
    let (_runtime, realm) = realm();
    assert_eq!(verdict(&script(root), &realm), Ok(()));
}

/// Found by execution: a global declaration named `JS_ATOM_NULL` became a
/// property of the global object that its shape reads as deleted, so the
/// variable reference it held was never marked or freed.
#[test]
fn null_atom_as_a_property_key_regression() {
    let mut root = function(FunctionKind::Normal, returns_undefined());
    root.closure_vars = vec![Cv::new(Atom::NULL, ClosureType::GlobalDecl, 0)];
    refused(&script(root), Rule::NullAtom);
    let mut root = function(
        FunctionKind::Normal,
        vec![i(op::PUSH_CONST8, &[Const(0)]), s(op::RETURN)],
    );
    root.cpool = vec![Value::Object(vec![(Atom::NULL, Value::Int32(1))])];
    refused(&script(root), Rule::NullAtom);
}

/// Found by execution: a value stored straight into an undeclared
/// global's variable reference leaked when a declaration adopted it.
#[test]
fn global_binding_regression() {
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![
            s(op::OBJECT),
            i(op::PUT_VAR_REF_CHECK_INIT, &[ClosureVar(0)]),
        ]),
    );
    root.closure_vars = vec![Cv::new(named(), ClosureType::Global, 0)];
    refused(&script(root), Rule::GlobalBinding);
    // Through a child's `GLOBAL_REF`.
    let mut child = function(
        FunctionKind::Normal,
        vec![s(op::GET_VAR_REF0), s(op::RETURN)],
    );
    child.closure_vars = vec![Cv::new(named(), ClosureType::GlobalRef, 0)];
    let mut root = function(
        FunctionKind::Normal,
        with_tail(vec![i(op::FCLOSURE8, &[Const(0)]), s(op::DROP)]),
    );
    root.closure_vars = vec![Cv::new(named(), ClosureType::Global, 0)];
    root.cpool = vec![Value::Function(Box::new(child))];
    refused(&script(root), Rule::GlobalBinding);
}

#[test]
fn frame_kind() {
    // `await` in a normal frame.
    refused(
        &script(function(
            FunctionKind::Normal,
            vec![s(op::PUSH_1), s(op::AWAIT), s(op::RETURN)],
        )),
        Rule::FrameKind,
    );
    let generator = |code: Vec<Instruction>| {
        let mut root = function(
            FunctionKind::Normal,
            with_tail(vec![i(op::FCLOSURE8, &[Const(0)]), s(op::DROP)]),
        );
        root.cpool = vec![Value::Function(Box::new(function(
            FunctionKind::Generator,
            code,
        )))];
        script(root)
    };
    // `yield` before `initial_yield`.
    refused(
        &generator(vec![s(op::PUSH_1), s(op::YIELD), s(op::RETURN_ASYNC)]),
        Rule::FrameKind,
    );
    // `initial_yield` twice.
    refused(
        &generator(vec![
            s(op::INITIAL_YIELD),
            s(op::INITIAL_YIELD),
            s(op::UNDEFINED),
            s(op::RETURN_ASYNC),
        ]),
        Rule::FrameKind,
    );
    // `return` from a generator frame.
    refused(
        &generator(vec![s(op::INITIAL_YIELD), s(op::UNDEFINED), s(op::RETURN)]),
        Rule::FrameKind,
    );
    // `return_async` before `initial_yield`.
    refused(
        &generator(vec![s(op::UNDEFINED), s(op::RETURN_ASYNC)]),
        Rule::FrameKind,
    );
    // A module function that throws while it links.
    let program = module_program(|program| {
        let mut module = module(
            program,
            vec![s(op::UNDEFINED), s(op::RETURN_ASYNC)],
            Vec::new(),
        );
        module.function.instructions.insert(2, s(op::PUSH_1));
        module.function.instructions.insert(3, s(op::THROW));
        module.function.instructions[1] = i(op::IF_FALSE8, &[Target(5)]);
        module
    });
    refused(&program, Rule::FrameKind);
}

/// Found by execution: a module function's linking runs in a normal frame
/// and leaves through the generator exit on an exception, so a value it
/// left in a local leaked and the runtime aborted when it was freed.
#[test]
fn module_linking_stores_a_reference_regression() {
    let program = module_program(|program| {
        let mut module = module(
            program,
            vec![s(op::UNDEFINED), s(op::RETURN_ASYNC)],
            Vec::new(),
        );
        module.function.var_count = 1;
        module.function.instructions.insert(0, s(op::OBJECT));
        module.function.instructions.insert(1, s(op::PUT_LOC0));
        module.function.instructions[3] = i(op::IF_FALSE8, &[Target(5)]);
        module
    });
    refused(&program, Rule::FrameKind);
}

#[test]
fn falls_off_end() {
    refused(
        &script(function(FunctionKind::Normal, vec![s(op::PUSH_1)])),
        Rule::FallsOffEnd,
    );
}

#[test]
fn module_imports_name_matching_variables() {
    let program = module_program(|program| {
        let x = program.intern_str("x");
        let spec = program.intern_str("./dep.js");
        let mut module = module(
            program,
            vec![s(op::UNDEFINED), s(op::RETURN_ASYNC)],
            vec![Cv::new(x, ClosureType::ModuleDecl, 0)],
        );
        module.requests = vec![crate::bytecode::ModuleRequest {
            specifier: spec,
            attributes: Value::Undefined,
        }];
        // A named import must fill a MODULE_IMPORT variable.
        module.imports = vec![Import {
            var_idx: 0,
            is_star: false,
            name: x,
            request: 0,
        }];
        module
    });
    refused(&program, Rule::ModuleRecord);
}
