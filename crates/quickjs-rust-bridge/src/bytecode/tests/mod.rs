//! Tests of the bytecode model, reader, writer and verifier.

mod corpus;
mod fuzz;
mod robustness;
mod rules;
mod verifier;
mod wide;

use super::{Program, RegExpCompiler, read_program, verify, write_program};
use crate::{Context, Runtime};

/// A runtime and a realm for compiling and checking.
pub(super) fn realm() -> (Runtime, Context) {
    let runtime = Runtime::new().expect("runtime should initialize");
    let realm = runtime.create_context().expect("realm should initialize");
    (runtime, realm)
}

/// Verifies `program`, compiling regular expressions in `realm`.
pub(super) fn check(program: &Program, realm: &Context) -> Result<(), super::VerifyError> {
    verify(program, realm as &dyn RegExpCompiler)
}

/// Read, write: the same bytes.
pub(super) fn assert_round_trip(bytes: &[u8], what: &str) {
    let program = read_program(bytes).unwrap_or_else(|error| panic!("{what} reads: {error}"));
    let written = write_program(&program).unwrap_or_else(|error| panic!("{what} writes: {error}"));
    assert!(written == bytes, "{what}: written bytes differ");
}

#[test]
#[ignore = "prints a disassembly for debugging: BYTECODE_SOURCE=..."]
fn print_disassembly() {
    let source = std::env::var("BYTECODE_SOURCE").unwrap_or_else(|_| "1".to_owned());
    let module = std::env::var("BYTECODE_MODULE").is_ok();
    let (_runtime, realm) = realm();
    let bytes = if module {
        realm
            .compile_module_bytecode("app:///m.js", &source)
            .unwrap()
    } else {
        realm.compile_script_bytecode("s.js", &source).unwrap()
    };
    let program = read_program(&bytes).unwrap();
    println!("{}", super::disassemble(&program));
    println!("{:?}", check(&program, &realm));
}
