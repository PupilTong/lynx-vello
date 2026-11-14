//! The reader, writer, verifier and disassembler answer for any input:
//! fixed-seed mutations of real bytecode never panic them.

use super::corpus::compiled_programs;
use super::fuzz::Rng;
use super::{check, realm};
use crate::bytecode::{Value, disassemble, read_program, write_program};
use crate::bytecode_abi::{BC_VERSION, OP_COUNT, op, tag};
use crate::bytecode_tests::{BRANCHY, compiled_script, first_function, instructions};
use crate::{Context, Runtime};

fn new_realm() -> Context {
    let runtime = Box::leak(Box::new(Runtime::new().unwrap()));
    runtime.create_context().unwrap()
}

#[test]
fn mutated_bytes_never_panic_the_reader() {
    let programs = compiled_programs(&new_realm);
    let (_runtime, checker) = realm();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut read, mut refused) = (0usize, 0usize);
    for round in 0..40_000 {
        let (_, original) = &programs[round % programs.len()];
        let mut bytes = original.clone();
        for _ in 0..=rng.below(6) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len());
            match rng.below(7) {
                0 => bytes[at] = rng.next().to_le_bytes()[0],
                1 => bytes[at] ^= 1 << rng.below(8),
                2 => bytes[at] = [0, 1, 0x7f, 0x80, 0xff][rng.below(5)],
                3 => bytes.truncate(at),
                4 => {
                    let byte = rng.next().to_le_bytes()[0];
                    bytes.insert(at, byte);
                }
                5 => {
                    bytes.remove(at);
                }
                _ => {
                    // A long run of 0xff: huge LEB128 counts.
                    let end = (at + 1 + rng.below(8)).min(bytes.len());
                    bytes[at..end].fill(0xff);
                }
            }
        }
        match read_program(&bytes) {
            Ok(program) => {
                read += 1;
                let _ = disassemble(&program);
                let _ = check(&program, &checker);
                // Writing renumbers file atoms in first-use order, so the
                // bytes it writes are a fixed point of read and write.
                let written = write_program(&program).expect("a read program writes");
                let again = read_program(&written).expect("written bytes read");
                assert!(write_program(&again).unwrap() == written, "round {round}");
            }
            Err(_) => refused += 1,
        }
    }
    assert!(read > 0 && refused > 0, "{read} read, {refused} refused");
}

#[test]
fn deep_nesting_is_refused_not_overflowed() {
    // An array nested far deeper than the limit, as bytes.
    let mut bytes = vec![crate::bytecode_abi::BC_VERSION, 0];
    for _ in 0..100_000 {
        bytes.extend_from_slice(&[crate::bytecode_abi::tag::ARRAY, 1]);
    }
    bytes.push(crate::bytecode_abi::tag::NULL);
    let error = read_program(&bytes).expect_err("too deep");
    assert!(error.message.contains("nested"), "{error}");
    // A count far beyond the input is refused before anything is allocated.
    let bytes = [
        crate::bytecode_abi::BC_VERSION,
        0xff,
        0xff,
        0xff,
        0xff,
        0x07,
    ];
    assert!(read_program(&bytes).is_err());
}

/// The malformed code the fork's reader steps through without a check —
/// an opcode outside its table, an instruction that runs past the end of
/// its function — is refused here.
#[test]
fn malformed_code_is_refused() {
    let original = compiled_script(BRANCHY);
    let layout = first_function(&original);
    for opcode in [op::INVALID, u8::try_from(OP_COUNT).unwrap(), u8::MAX] {
        let mut bytes = original.clone();
        bytes[layout.code.0] = opcode;
        let error = read_program(&bytes).expect_err("an invalid opcode");
        assert!(error.message.contains("invalid opcode"), "{error}");
    }
    let mut bytes = original.clone();
    let last = *instructions(&bytes, &layout).last().unwrap();
    assert_eq!(bytes[last], op::RETURN);
    // A one-byte instruction becomes a five-byte one.
    bytes[last] = op::PUSH_I32;
    let error = read_program(&bytes).expect_err("a truncated instruction");
    assert!(error.message.contains("truncated instruction"), "{error}");
}

/// Malformed records and values the fork's reader would take as they
/// are: a 16-bit field above 16 bits, vardefs that do not number the
/// arguments and variables, and a function or a module where compiled
/// code never puts one.
#[test]
fn malformed_records_are_refused() {
    // The top-level function of a script with a block-scoped local.
    let original = compiled_script("{ let y = globalThis.c ? 1 : 2; String(y) }");
    let program = read_program(&original).unwrap();
    let Value::Function(function) = &program.root else {
        panic!("a script")
    };
    assert!(function.var_count > 0 && !function.vardefs.is_empty());
    assert!(function.arg_count < 0x80 && function.var_count < 0x7f);
    let layout = first_function(&original);
    let (at, length) = layout.stack_size;

    // The stack size, 65536.
    let mut bytes = original.clone();
    bytes.splice(at..at + length, [0x80, 0x80, 0x04]);
    let error = read_program(&bytes).expect_err("a 16-bit field above 16 bits");
    assert!(error.message.contains("above 16 bits"), "{error}");

    // `var_count`, one byte two before the stack size, one more than the
    // vardefs.
    let mut bytes = original.clone();
    bytes[at - 2] += 1;
    let error = read_program(&bytes).expect_err("a vardef count");
    assert!(error.message.contains("vardefs for"), "{error}");

    // A function inside an array, a module inside an array, and a tag
    // compiled code does not contain.
    for (inner, what) in [
        (tag::FUNCTION_BYTECODE, "function"),
        (tag::MODULE, "module"),
        (0xfe, "unknown tag"),
    ] {
        let bytes = [BC_VERSION, 0, tag::ARRAY, 1, inner];
        let error = read_program(&bytes).expect_err(what);
        assert!(
            error.message.contains("not allowed here"),
            "{what}: {error}"
        );
    }
}
