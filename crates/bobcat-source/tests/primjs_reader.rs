//! `PrimJS` bytecode reader: frozen tables, the three empty-program
//! fingerprints `native::decode` accepts, real encoder output, and bundle
//! section extraction.
//!
//! The binary fixtures in `tests/fixtures/primjs/` were written by the native
//! encoder `@lynx-js/tasm@0.0.53` (the version `reactlynx-test-fixtures`
//! pins). Each `NAME.js` was encoded as a bundle's main-thread root
//! (`lepusCode.root`) and as its background `/app-service.js` with
//! `experimental_encodeQuickjsBytecode`, `useLepusNG` and
//! `enableFiberArch` on; the files are the payloads cut out of those bundles:
//!
//! | File | Section | `targetSdkVersion` | `debugInfoOutside` |
//! | --- | --- | --- | --- |
//! | `NAME.mts.v1.bin` | `ROOT_LEPUS` | 2.13 | false |
//! | `NAME.mts.v1-outside.bin` | `ROOT_LEPUS` | 2.13 | true |
//! | `NAME.mts.v9.bin` | `ROOT_LEPUS` | 3.2 | true |
//! | `NAME.mts.v9-strip.bin` | `ROOT_LEPUS` | 4.1.0 | true |
//! | `NAME.bts.v1.bin` | `JS_BYTECODE` `/app-service.js` (packed) | 2.13 | true |
//! | `NAME.bts.v9.bin` | `JS_BYTECODE` `/app-service.js` (packed) | 3.2 | true |
//!
//! `sections.lynx.bundle` is a whole bundle (target 3.2) with a root, one
//! Lepus chunk, one `JS_BYTECODE` file, one `JsBytecode` custom section and
//! one source custom section.

use std::path::{Path, PathBuf};

use bobcat_source::native::primjs::{
    self, ATOM_END, Atom, BUILTIN_ATOM_COUNT, BUILTIN_ATOMS, DebugInfo, Format, Function,
    FunctionKind, Header, HeaderFlags, NON_SHIPPABLE_INC_COVERAGE, OPCODE_COUNT, OPCODE_INFO,
    Opcode, Operand, Program, SectionKind, Value,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/primjs")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn read(name: &str) -> Program {
    let bytes = fixture(name);
    let (_, program) =
        primjs::read_payload(&bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
    program
}

const PROGRAMS: &[&str] = &[
    "async_function",
    "class",
    "closures",
    "direct_eval",
    "generator",
    "globals",
    "regexp",
    "template_literal",
    "try_finally",
    "values",
];
const VARIANTS: &[&str] = &[
    "mts.v1",
    "mts.v1-outside",
    "mts.v9",
    "mts.v9-strip",
    "bts.v1",
    "bts.v9",
];

/// Every function of the program, depth first, parents before children.
fn functions(program: &Program) -> Vec<&Function> {
    fn walk<'a>(function: &'a Function, out: &mut Vec<&'a Function>) {
        out.push(function);
        for value in &function.cpool {
            if let Value::Function(child) = value {
                walk(child, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&program.root, &mut out);
    out
}

fn has_opcode(program: &Program, opcode: Opcode) -> bool {
    functions(program).iter().any(|function| {
        function
            .instructions
            .iter()
            .any(|insn| insn.opcode == opcode)
    })
}

fn named<'a>(program: &'a Program, name: &str) -> &'a Function {
    functions(program)
        .into_iter()
        .find(|function| program.atom_text(function.name).as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no function named {name}"))
}

#[test]
fn tables_are_the_frozen_primjs_tables() {
    assert_eq!(OPCODE_COUNT, 242);
    assert_eq!(OPCODE_INFO.len(), 242);
    assert_eq!(primjs::ALL_OPCODES.len(), 242);
    for (number, opcode) in primjs::ALL_OPCODES.iter().enumerate() {
        assert_eq!(usize::from(opcode.number()), number);
        assert_eq!(Opcode::from_u8(opcode.number()), Some(*opcode));
    }
    assert_eq!(NON_SHIPPABLE_INC_COVERAGE, 242);
    assert_eq!(Opcode::from_u8(242), None);
    assert_eq!(Opcode::from_u8(255), None);
    assert_eq!(Opcode::from_u8(0), Some(Opcode::Invalid));
    // Spot checks against quickjs-opcode.h.
    assert_eq!(Opcode::PushI32.number(), 1);
    assert_eq!(Opcode::Nop.name(), "nop");
    assert_eq!(Opcode::PushMinus1.number(), Opcode::Nop.number() + 1);
    assert_eq!(Opcode::GetLoc0.number(), 194);
    assert_eq!(Opcode::Return.number(), 40);
    assert_eq!(Opcode::IsFunction.number(), 241);
    let with = Opcode::WithGetVar.info();
    assert_eq!(
        (with.size, with.pops, with.pushes, with.format),
        (10, 1, 0, Format::AtomLabelU8)
    );
    let call = Opcode::CallMethod.info();
    assert_eq!(
        (call.size, call.pops, call.pushes, call.format),
        (3, 2, 1, Format::Npop)
    );
    // Every encoded size agrees with its format.
    for info in &OPCODE_INFO {
        let expected = match info.format {
            Format::None
            | Format::NoneInt
            | Format::NoneLoc
            | Format::NoneArg
            | Format::NoneVarRef
            | Format::Npopx => 1,
            Format::U8 | Format::I8 | Format::Loc8 | Format::Const8 | Format::Label8 => 2,
            Format::U16
            | Format::I16
            | Format::Label16
            | Format::Npop
            | Format::Loc
            | Format::Arg
            | Format::VarRef => 3,
            Format::I32 | Format::Const | Format::Label | Format::Atom => 5,
            Format::AtomU8 => 6,
            Format::AtomU16 => 7,
            Format::AtomLabelU8 => 10,
        };
        assert_eq!(info.size, expected, "{}", info.name);
    }

    assert_eq!(ATOM_END, 203);
    assert_eq!(BUILTIN_ATOM_COUNT, 202);
    assert_eq!(BUILTIN_ATOMS.len(), 203);
    assert_eq!(BUILTIN_ATOMS[1], "null");
    assert_eq!(BUILTIN_ATOMS[47], "");
    assert_eq!(BUILTIN_ATOMS[79], "<eval>");
    assert_eq!(BUILTIN_ATOMS[80], "<ret>");
    assert_eq!(BUILTIN_ATOMS[202], "Symbol.asyncIterator");
    assert!(!BUILTIN_ATOMS.contains(&"not-equal"));
    assert!(!BUILTIN_ATOMS.contains(&"timed-out"));
    assert_eq!(primjs::builtin_atom(203), None);

    assert_eq!(primjs::VERSION_WORD, 0xCAB0_01CC);
    assert_eq!(primjs::tag::BIG_FLOAT, 11);
    assert_eq!(primjs::tag::FUNCTION_BYTECODE, 13);
    assert_eq!(primjs::tag::MODULE, 14);
}

// Copies of the three fingerprints in `src/native/decode.rs`.
const EMPTY_ROOT_LEPUS_INLINE_DEBUG: &[u8] = &[
    1, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 1, 160, 1, 0, 0, 0, 194, 40, 94, 1, 12, 0, 0,
    128, 128, 128, 144, 128, 128, 128, 128, 128, 1,
];
const EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG: &[u8] = &[
    9, 204, 1, 176, 202, 1, 0, 0, 0, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 1, 160, 1, 0, 0,
    0, 194, 40, 94, 1, 0,
];
const EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG_TASM_0_0_53: &[u8] = &[
    9, 204, 1, 176, 202, 3, 0, 0, 0, 0, 13, 0, 6, 0, 158, 1, 0, 1, 0, 1, 0, 0, 2, 0, 194, 40, 94,
    1, 0,
];

fn assert_empty_program(program: &Program) {
    assert!(program.atoms.is_empty());
    let root = &program.root;
    assert_eq!(
        root.offset,
        if program.header == Header::Legacy {
            2
        } else {
            10
        }
    );
    assert_eq!(root.name, Atom::Builtin(79));
    assert_eq!(program.atom_text(root.name).as_deref(), Some("<eval>"));
    assert!(root.flags.arguments_allowed && root.flags.has_debug);
    assert!(!root.flags.has_prototype && !root.flags.new_target_allowed);
    assert_eq!(root.flags.kind, FunctionKind::Normal);
    assert_eq!(root.js_mode.0, 0);
    assert_eq!(
        (
            root.arg_count,
            root.var_count,
            root.defined_arg_count,
            root.stack_size
        ),
        (0, 1, 0, 1)
    );
    assert!(root.closure_vars.is_empty() && root.cpool.is_empty());
    assert_eq!(root.bytecode_len, 2);
    let code: Vec<_> = root
        .instructions
        .iter()
        .map(|insn| (insn.pc, insn.opcode, insn.operands().to_vec()))
        .collect();
    assert_eq!(
        code,
        [
            (0, Opcode::GetLoc0, vec![Operand::Local(0)]),
            (1, Opcode::Return, vec![]),
        ]
    );
}

#[test]
fn empty_program_with_inline_debug() {
    let program = primjs::read_program(EMPTY_ROOT_LEPUS_INLINE_DEBUG).unwrap();
    assert_eq!(program.header, Header::Legacy);
    assert_empty_program(&program);
    assert_eq!(program.root.vardefs.len(), 1);
    assert_eq!(program.root.vardefs[0].name, Atom::Builtin(80));
    assert_eq!(program.root.vardefs[0].scope_next, -1);
    let Some(DebugInfo::Inline {
        filename,
        line_num,
        pc2line,
    }) = &program.root.debug
    else {
        panic!("expected inline debug info");
    };
    assert_eq!(
        (*filename, *line_num, pc2line.len()),
        (Atom::Builtin(47), 1, 12)
    );
    assert_eq!(
        primjs::disassemble(&program),
        "header: version 1
atoms: 0
function @2 \"<eval>\"
  kind normal, js_mode 0x00, flags [arguments_allowed has_debug]
  args 0, vars 1, defined_args 0, stack 1, closure_vars 0, cpool 0, bytecode 2 bytes @21
  vardefs:
    0: \"<ret>\" scope_level 0 scope_next -1 kind 0
  code:
        0  get_loc0 loc:0
        1  return
  debug: inline, filename \"\", line 1, pc2line 12 bytes
"
    );
}

#[test]
fn empty_program_with_external_debug() {
    let program = primjs::read_program(EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG).unwrap();
    assert_eq!(
        program.header,
        Header::Versioned(HeaderFlags {
            external_debug_info: true,
            vardefs_stripped: false,
        })
    );
    assert_empty_program(&program);
    assert_eq!(program.root.vardefs.len(), 1);
    assert_eq!(
        program.root.debug,
        Some(DebugInfo::External {
            filename: Atom::Builtin(47),
            function_id: 1,
        })
    );
    assert_eq!(
        primjs::disassemble(&program),
        "header: version 9, external debug info
atoms: 0
function @10 \"<eval>\"
  kind normal, js_mode 0x00, flags [arguments_allowed has_debug]
  args 0, vars 1, defined_args 0, stack 1, closure_vars 0, cpool 0, bytecode 2 bytes @29
  vardefs:
    0: \"<ret>\" scope_level 0 scope_next -1 kind 0
  code:
        0  get_loc0 loc:0
        1  return
  debug: external, filename \"\", function_id 1
"
    );
}

#[test]
fn empty_program_with_stripped_vardefs() {
    let program = primjs::read_program(EMPTY_ROOT_LEPUS_EXTERNAL_DEBUG_TASM_0_0_53).unwrap();
    assert_eq!(
        program.header,
        Header::Versioned(HeaderFlags {
            external_debug_info: true,
            vardefs_stripped: true,
        })
    );
    assert_empty_program(&program);
    assert!(program.root.vardefs.is_empty());
    assert_eq!(
        primjs::disassemble(&program),
        "header: version 9, external debug info, vardefs stripped
atoms: 0
function @10 \"<eval>\"
  kind normal, js_mode 0x00, flags [arguments_allowed has_debug]
  args 0, vars 1, defined_args 0, stack 1, closure_vars 0, cpool 0, bytecode 2 bytes @24
  code:
        0  get_loc0 loc:0
        1  return
  debug: external, filename \"\", function_id 1
"
    );
}

#[test]
fn every_fixture_reads_and_disassembles_deterministically() {
    for program in PROGRAMS {
        for variant in VARIANTS {
            let name = format!("{program}.{variant}.bin");
            let decoded = read(&name);
            let text = primjs::disassemble(&decoded);
            assert_eq!(text, primjs::disassemble(&read(&name)), "{name}");
            assert!(text.contains("\n  code:\n"), "{name}");
            // Every instruction's operands respect the function's counts and
            // every jump lands on an instruction.
            for function in functions(&decoded) {
                for insn in &function.instructions {
                    for operand in insn.operands() {
                        match *operand {
                            Operand::Local(index) => assert!(index < function.var_count),
                            Operand::Arg(index) => assert!(index < function.arg_count),
                            Operand::ClosureVar(index) => {
                                assert!(u32::from(index) < function.closure_var_count());
                            }
                            Operand::Const(index) => {
                                assert!((index as usize) < function.cpool.len());
                            }
                            Operand::Target(index) => {
                                assert!((index as usize) < function.instructions.len());
                            }
                            Operand::Atom(atom) => assert!(decoded.atom_text(atom).is_some()),
                            Operand::Immediate(_) => {}
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn header_forms_follow_the_encoder_options() {
    for program in PROGRAMS {
        let v1 = read(&format!("{program}.mts.v1.bin"));
        assert_eq!(v1.header, Header::Legacy);
        assert!(
            matches!(v1.root.debug, Some(DebugInfo::Inline { .. })),
            "{program}"
        );
        assert!(!v1.root.vardefs.is_empty(), "{program}");

        let outside = read(&format!("{program}.mts.v1-outside.bin"));
        assert_eq!(outside.header, Header::Legacy);
        let Some(DebugInfo::External { filename, .. }) = outside.root.debug else {
            panic!("{program}: legacy external debug info expected");
        };
        assert_eq!(
            outside.atom_text(filename).as_deref(),
            Some("__lepusNG_function_id__")
        );

        let v9 = read(&format!("{program}.mts.v9.bin"));
        assert_eq!(
            v9.header,
            Header::Versioned(HeaderFlags {
                external_debug_info: true,
                vardefs_stripped: false,
            })
        );
        assert!(matches!(v9.root.debug, Some(DebugInfo::External { .. })));
        assert!(!v9.root.vardefs.is_empty(), "{program}");

        let stripped = read(&format!("{program}.mts.v9-strip.bin"));
        assert_eq!(
            stripped.header,
            Header::Versioned(HeaderFlags {
                external_debug_info: true,
                vardefs_stripped: true,
            })
        );
        // Stripping keeps the vardefs of a function with a direct eval.
        for function in functions(&stripped) {
            let has_eval = function
                .instructions
                .iter()
                .any(|insn| insn.opcode == Opcode::Eval);
            assert_eq!(!function.vardefs.is_empty(), has_eval, "{program}");
        }

        for (variant, major, minor) in [("bts.v1", 2, 13), ("bts.v9", 3, 2)] {
            let bytes = fixture(&format!("{program}.{variant}.bin"));
            assert!(primjs::is_packed(&bytes));
            let (packed, program_bytes) = primjs::unwrap_packed(&bytes).unwrap().unwrap();
            assert_eq!(packed.bytecode_offset, 24);
            assert_eq!(
                (packed.target_sdk_major, packed.target_sdk_minor),
                (major, minor)
            );
            let background = primjs::read_program(program_bytes).unwrap();
            if variant == "bts.v1" {
                assert_eq!(background.header, Header::Legacy);
                // Compiled with debug info stripped below 2.14.
                assert!(functions(&background).iter().all(|f| f.debug.is_none()));
            } else {
                assert!(background.header.flags().external_debug_info);
                assert!(!background.header.flags().vardefs_stripped);
            }
        }
    }
}

#[test]
#[expect(clippy::too_many_lines, reason = "one block per fixture program")]
fn programs_decode_to_the_expected_shapes() {
    let closures = read("closures.mts.v9-strip.bin");
    let counter = named(&closures, "counter");
    assert!(
        counter
            .instructions
            .iter()
            .any(|insn| matches!(insn.opcode, Opcode::Fclosure | Opcode::Fclosure8))
    );
    // `next` and `reset` capture `count` (a local of `counter`); `reset`
    // also captures the argument `start`.
    let capturing: Vec<_> = counter
        .cpool
        .iter()
        .filter_map(|value| match value {
            Value::Function(child) => Some(child),
            _ => None,
        })
        .collect();
    assert_eq!(capturing.len(), 2);
    for child in capturing {
        let count = child
            .closure_vars
            .iter()
            .find(|var| closures.atom_text(var.name).as_deref() == Some("count"))
            .unwrap();
        assert!(count.is_local && !count.is_arg);
        assert!(count.var_idx < u32::from(counter.var_count));
    }

    let class = read("class.mts.v9.bin");
    assert!(has_opcode(&class, Opcode::DefineClass));
    assert!(has_opcode(&class, Opcode::GetSuper) || has_opcode(&class, Opcode::GetSuperCtor));
    assert!(
        functions(&class)
            .iter()
            .any(|f| f.flags.is_derived_class_constructor)
    );

    let try_finally = read("try_finally.mts.v1.bin");
    for opcode in [Opcode::Catch, Opcode::Gosub, Opcode::Ret, Opcode::Throw] {
        assert!(has_opcode(&try_finally, opcode), "{opcode:?}");
    }
    let guarded = named(&try_finally, "guarded");
    let gosub = guarded
        .instructions
        .iter()
        .find(|insn| insn.opcode == Opcode::Gosub)
        .unwrap();
    let target = &guarded.instructions[gosub.target().unwrap() as usize];
    assert!(target.pc > gosub.pc);

    let generator = read("generator.mts.v9.bin");
    let numbers = named(&generator, "numbers");
    assert_eq!(numbers.flags.kind, FunctionKind::Generator);
    for opcode in [Opcode::InitialYield, Opcode::Yield, Opcode::YieldStar] {
        assert!(
            numbers
                .instructions
                .iter()
                .any(|insn| insn.opcode == opcode),
            "{opcode:?}"
        );
    }

    let asynchronous = read("async_function.mts.v9.bin");
    let load = named(&asynchronous, "load");
    assert_eq!(load.flags.kind, FunctionKind::Async);
    for opcode in [Opcode::Await, Opcode::ForAwaitOfStart, Opcode::ReturnAsync] {
        assert!(
            load.instructions.iter().any(|insn| insn.opcode == opcode),
            "{opcode:?}"
        );
    }

    // A RegExp literal: two pool strings, then `regexp`.
    let regexp = read("regexp.mts.v9.bin");
    let root = &regexp.root;
    let mut literals = 0;
    for (index, insn) in root.instructions.iter().enumerate() {
        if insn.opcode != Opcode::Regexp {
            continue;
        }
        literals += 1;
        for previous in &root.instructions[index - 2..index] {
            let constant = match previous.operands() {
                [Operand::Const(constant)] => *constant,
                other => panic!("regexp operand {other:?}"),
            };
            assert!(matches!(root.cpool[constant as usize], Value::String(_)));
        }
    }
    assert_eq!(literals, 2);

    let template = read("template_literal.mts.v9.bin");
    assert!(
        template
            .root
            .cpool
            .iter()
            .any(|value| matches!(value, Value::TemplateObject { .. }))
    );

    let globals = read("globals.mts.v9-strip.bin");
    for opcode in [
        Opcode::CheckDefineVar,
        Opcode::DefineVar,
        Opcode::DefineFunc,
        Opcode::PutVarInit,
    ] {
        assert!(has_opcode(&globals, opcode), "{opcode:?}");
    }

    let values = read("values.mts.v9.bin");
    let pool = &values.root.cpool;
    assert!(
        pool.iter()
            .any(|value| matches!(value, Value::BigInt(bytes) if bytes.len() > 8))
    );
    assert!(pool.contains(&Value::Float64(1.5)));
    // String literals become atoms; the non-Latin-1 one is stored wide.
    assert!(values.atoms.iter().any(|atom| {
        matches!(atom, primjs::JsString::Utf16(_))
            && atom.to_string_lossy() == "caf\u{e9} \u{4e16}\u{754c}"
    }));
    assert!(
        values
            .atoms
            .iter()
            .any(|atom| atom == &primjs::JsString::Latin1(b"nested".to_vec()))
    );

    let eval = read("direct_eval.mts.v9-strip.bin");
    let inspect = named(&eval, "inspect");
    assert!(
        inspect
            .instructions
            .iter()
            .any(|insn| insn.opcode == Opcode::Eval)
    );
    assert_eq!(
        inspect.vardefs.len(),
        usize::from(inspect.arg_count) + usize::from(inspect.var_count)
    );
}

#[test]
fn bundle_sections_are_extracted() {
    let bundle = fixture("sections.lynx.bundle");
    let sections = primjs::bundle_bytecode_sections(&bundle).unwrap();
    let summary: Vec<_> = sections
        .iter()
        .map(|section| (section.kind, section.name.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (SectionKind::RootLepus, "ROOT_LEPUS"),
            (SectionKind::LepusChunk, "chunk"),
            (SectionKind::JsBytecode, "/app-service.js"),
            (SectionKind::CustomSection, "compiled"),
        ]
    );
    for section in &sections {
        assert_eq!(
            &bundle[section.offset..section.offset + section.bytes.len()],
            section.bytes
        );
        let (packed, program) = primjs::read_payload(section.bytes)
            .unwrap_or_else(|error| panic!("{}: {error}", section.name));
        assert_eq!(
            packed.is_some(),
            section.kind == SectionKind::JsBytecode,
            "{}",
            section.name
        );
        let atoms: Vec<_> = program
            .atoms
            .iter()
            .map(|atom| atom.to_string_lossy().into_owned())
            .collect();
        let expected = match section.kind {
            SectionKind::RootLepus => "rootValue",
            SectionKind::LepusChunk => "chunkValue",
            SectionKind::JsBytecode => "background",
            SectionKind::CustomSection => "custom",
        };
        assert!(atoms.iter().any(|atom| atom == expected), "{atoms:?}");
    }

    // `decode` still rejects the same bundle.
    let error = bobcat_source::native::decode(&bundle).unwrap_err();
    assert!(matches!(
        error,
        bobcat_source::native::ConvertError::CodeCacheBundle { .. }
    ));
}

#[test]
fn real_program_disassembly_is_pinned() {
    // The disassembly of a whole real program: global `var`, `let`, `const`
    // and function declarations in the script prologue, and a nested function
    // reading them through `get_var`.
    let program = read("globals.mts.v9-strip.bin");
    let expected = String::from_utf8(fixture("globals.mts.v9-strip.txt")).unwrap();
    assert_eq!(primjs::disassemble(&program), expected);
}
