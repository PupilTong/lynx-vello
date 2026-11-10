//! What the fork compiles reads, writes back to the same bytes, passes the
//! verifier, and runs the same after the checks as before.

use std::path::{Path, PathBuf};

use super::{assert_round_trip, check, realm};
use crate::bytecode::{Root, read_program};
use crate::bytecode_tests::{MODULES, SCRIPTS};
use crate::{Context, EvalOptions, EvalSource, Runtime};

/// The scripts of `packages/reactlynx-test-fixtures/bytecode-corpus`, read
/// from the directory at test time (its size changes as scripts are
/// added).
pub(in crate::bytecode) fn corpus() -> Vec<(String, String)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/reactlynx-test-fixtures/bytecode-corpus");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "js"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let source = std::fs::read_to_string(&path).unwrap();
            (name, source)
        })
        .collect()
}

/// Every program of the test sources: scripts, and modules where the source
/// is valid module code. `(name, bytes)`.
pub(in crate::bytecode) fn compiled_programs(
    realm_for: &dyn Fn() -> Context,
) -> Vec<(String, Vec<u8>)> {
    let mut programs = Vec::new();
    let mut sources: Vec<(String, String)> = SCRIPTS
        .iter()
        .enumerate()
        .map(|(index, source)| (format!("SCRIPTS[{index}]"), (*source).to_owned()))
        .collect();
    sources.extend(corpus());
    for (name, source) in &sources {
        let compiler = realm_for();
        let script = compiler
            .compile_script_bytecode(name, source)
            .unwrap_or_else(|error| panic!("{name} compiles as a script: {error}"));
        programs.push((format!("{name} (script)"), script));
        // A realm compiles each module name once.
        let compiler = realm_for();
        if let Ok(module) = compiler.compile_module_bytecode(&format!("app:///{name}"), source) {
            programs.push((format!("{name} (module)"), module));
        }
    }
    for (index, source) in MODULES.iter().enumerate() {
        let compiler = realm_for();
        let module = compiler
            .compile_module_bytecode(&format!("app:///module{index}.js"), source)
            .unwrap();
        programs.push((format!("MODULES[{index}] (module)"), module));
    }
    programs
}

fn new_realm() -> Context {
    // The runtime must outlive the realm; leak one per realm in tests.
    let runtime = Box::leak(Box::new(Runtime::new().unwrap()));
    runtime.create_context().unwrap()
}

#[test]
fn what_the_fork_writes_round_trips_and_verifies() {
    let programs = compiled_programs(&new_realm);
    assert!(programs.len() > 400, "{} programs", programs.len());
    let (_runtime, checker) = realm();
    for (name, bytes) in &programs {
        assert_round_trip(bytes, name);
        let program = read_program(bytes).unwrap();
        check(&program, &checker).unwrap_or_else(|error| panic!("{name} refused: {error}"));
    }
}

/// The checked bytes are what the engine reads, and they run as the
/// compiler's own bytes do (read with the verifying read alone).
#[test]
fn checked_scripts_run_as_compiled() {
    let run = |bytes: &[u8]| {
        let (mut runtime, mut runner) = realm();
        let result = runner.evaluate_script_bytecode(bytes).map(|_| ());
        let _ = runtime.drain_pending_jobs(&runner);
        let global = runner.global_object().unwrap();
        let value = runner
            .property(&global, "__result")
            .ok()
            .and_then(|value| value.to_utf16().ok())
            .map(|units| String::from_utf16_lossy(&units));
        (result.is_ok(), value)
    };
    for (name, source) in corpus() {
        let (_compiler_runtime, compiler) = realm();
        let bytes = compiler.compile_script_bytecode(&name, &source).unwrap();
        let (_runtime, realm_for_checks) = realm();
        let checked = crate::bytecode::check(&bytes, &realm_for_checks, Root::Any)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            checked, bytes,
            "{name}: the checked bytes are the compiled bytes"
        );
        assert_eq!(run(&checked), run(&bytes), "{name}");
        // The source run the usual way sets a result too.
        let (_runtime, direct) = realm();
        let _ = direct.evaluate(EvalSource::new(&source), EvalOptions::default());
    }
}
