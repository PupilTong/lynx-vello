//! The converter's opcode classification against both opcode tables, and
//! the opcodes the corpus conversion goes through.

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;

use std::collections::BTreeSet;

use bobcat_source::native::primjs::{
    self, ALL_OPCODES, ConvertOptions, Format, Function, OpcodeClass, REJECTED, SAME, TRANSLATED,
    Value, opcode_class,
};
use quickjs_rust_bridge::Runtime;
use quickjs_rust_bridge::bytecode_abi::{OPCODES, OperandFormat};

/// The fork's operand format that matches a `PrimJS` one by meaning.
fn same_format(primjs: Format, fork: OperandFormat) -> bool {
    matches!(
        (primjs, fork),
        (Format::None, OperandFormat::None)
            | (Format::NoneInt, OperandFormat::NoneInt)
            | (Format::NoneLoc, OperandFormat::NoneLoc)
            | (Format::NoneArg, OperandFormat::NoneArg)
            | (Format::NoneVarRef, OperandFormat::NoneVarRef)
            | (Format::U8, OperandFormat::U8)
            | (Format::I8, OperandFormat::I8)
            | (Format::Loc8, OperandFormat::Loc8)
            | (Format::Const8, OperandFormat::Const8)
            | (Format::Label8, OperandFormat::Label8)
            | (Format::U16, OperandFormat::U16)
            | (Format::I16, OperandFormat::I16)
            | (Format::Label16, OperandFormat::Label16)
            | (Format::Npop, OperandFormat::Npop)
            | (Format::Npopx, OperandFormat::Npopx)
            | (Format::Loc, OperandFormat::Loc)
            | (Format::Arg, OperandFormat::Arg)
            | (Format::VarRef, OperandFormat::VarRef)
            | (Format::I32, OperandFormat::I32)
            | (Format::Const, OperandFormat::Const)
            | (Format::Label, OperandFormat::Label)
            | (Format::Atom, OperandFormat::Atom)
            | (Format::AtomU8, OperandFormat::AtomU8)
            | (Format::AtomU16, OperandFormat::AtomU16)
            | (Format::AtomLabelU8, OperandFormat::AtomLabelU8)
    )
}

/// Every `PrimJS` opcode is in exactly one list; every copied opcode has a
/// fork opcode of the same name, stack effect and operand format (checked
/// here against the generated table, so a fork rebase that changes one
/// fails this test); and the converter's lookup agrees with the lists.
#[test]
fn opcode_classes() {
    for opcode in ALL_OPCODES {
        let lists = usize::from(SAME.contains(&opcode))
            + TRANSLATED.iter().filter(|(op, _)| *op == opcode).count()
            + REJECTED.iter().filter(|(op, _)| *op == opcode).count();
        assert_eq!(lists, 1, "{} is in {lists} lists", opcode.name());
        match opcode_class(opcode) {
            OpcodeClass::Same(fork) => {
                assert!(SAME.contains(&opcode));
                let info = &OPCODES[usize::from(fork)];
                assert_eq!(info.name, opcode.name());
                assert_eq!(
                    (info.pops, info.pushes),
                    (opcode.pops(), opcode.pushes()),
                    "{}: stack effect",
                    opcode.name()
                );
                assert!(
                    same_format(opcode.format(), info.format),
                    "{}: operand format {:?} vs {:?}",
                    opcode.name(),
                    opcode.format(),
                    info.format
                );
                assert_eq!(info.size, opcode.size(), "{}: size", opcode.name());
            }
            OpcodeClass::Translated(_) => {
                assert!(TRANSLATED.iter().any(|(op, _)| *op == opcode));
            }
            OpcodeClass::Rejected(_) => {
                assert!(REJECTED.iter().any(|(op, _)| *op == opcode));
            }
        }
    }
    assert_eq!(
        SAME.len() + TRANSLATED.len() + REJECTED.len(),
        ALL_OPCODES.len()
    );
}

fn walk(function: &Function, seen: &mut BTreeSet<u8>) {
    for instruction in &function.instructions {
        seen.insert(instruction.opcode.number());
    }
    for value in &function.cpool {
        if let Value::Function(child) = value {
            walk(child, seen);
        }
    }
}

/// Every opcode the converter accepts occurs in a corpus program that
/// converts (and so runs in `primjs_convert`); prints the list.
#[test]
fn every_accepted_opcode_is_exercised_by_the_corpus() {
    let runtime = Runtime::new().unwrap();
    let realm = runtime.create_context().unwrap();
    let mut seen = BTreeSet::new();
    for program in bytecode::PROGRAMS {
        for variant in bytecode::Variant::ALL {
            for section in primjs::bundle_bytecode_sections(program.bundle(variant)).unwrap() {
                let (_, parsed) = primjs::read_payload(section.bytes).unwrap();
                let options = ConvertOptions::script("app:///coverage.js");
                if primjs::convert_script(&parsed, &options, &realm).is_ok() {
                    walk(&parsed.root, &mut seen);
                }
            }
        }
    }
    let names: Vec<&str> = ALL_OPCODES
        .iter()
        .filter(|opcode| seen.contains(&opcode.number()))
        .map(|opcode| opcode.name())
        .collect();
    eprintln!(
        "the corpus conversion goes through {} PrimJS opcodes: {}",
        names.len(),
        names.join(" ")
    );
    let unexercised: Vec<&str> = ALL_OPCODES
        .iter()
        .filter(|opcode| {
            !matches!(opcode_class(**opcode), OpcodeClass::Rejected(_))
                && !seen.contains(&opcode.number())
        })
        .map(|opcode| opcode.name())
        .collect();
    assert!(
        unexercised.is_empty(),
        "accepted but never exercised: {}",
        unexercised.join(" ")
    );
}
