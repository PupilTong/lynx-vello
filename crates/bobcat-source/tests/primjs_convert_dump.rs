//! Disassembly for people working on the converter: the `PrimJS` bytecode of
//! a bundle or corpus program, the fork's compilation of the same source,
//! and the converter's output. Ignored; run with
//! `PRIMJS_DUMP=<corpus program name or bundle path> cargo test -p
//! bobcat-source --test primjs_convert_dump -- --ignored --nocapture`.
//! `PRIMJS_DUMP_SOURCE=<file>` adds the fork's compilation of that file.

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;

use bobcat_source::native::primjs;
use quickjs_rust_bridge::{Runtime, bytecode as fork};

#[test]
#[ignore = "prints disassemblies for debugging: PRIMJS_DUMP=<program or bundle path>"]
fn dump() {
    let runtime = Runtime::new().unwrap();
    let realm = runtime.create_context().unwrap();
    let verifier = fork::Verifier::new();
    let target = std::env::var("PRIMJS_DUMP").unwrap_or_else(|_| "arith-basic".to_owned());
    let variant = std::env::var("PRIMJS_DUMP_VARIANT").unwrap_or_else(|_| "sdk-2.14".to_owned());
    let (bundle, source): (Vec<u8>, Option<String>) = match bytecode::PROGRAMS
        .iter()
        .find(|program| program.name == target)
    {
        Some(program) => {
            let variant = bytecode::Variant::ALL
                .into_iter()
                .find(|v| v.name() == variant)
                .unwrap();
            (
                program.bundle(variant).to_vec(),
                Some(program.source.to_owned()),
            )
        }
        None if std::path::Path::new(&target)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bin")) =>
        {
            let (_, program) = primjs::read_payload(&std::fs::read(&target).unwrap()).unwrap();
            println!("{}", primjs::disassemble(&program));
            let mut stack = vec![&program.root];
            while let Some(function) = stack.pop() {
                println!("{:?} {:?}", function.name, function.debug);
                for value in &function.cpool {
                    if let primjs::Value::Function(child) = value {
                        stack.push(child);
                    }
                }
            }
            let options = primjs::ConvertOptions::script("app:///dump.js");
            if let Ok(module) = primjs::convert_script(&program, &options, &verifier) {
                println!(
                    "{}",
                    fork::disassemble(&fork::read_program(module.bytes()).unwrap())
                );
            }
            return;
        }
        None => (
            std::fs::read(&target).unwrap(),
            std::env::var("PRIMJS_DUMP_SOURCE")
                .ok()
                .map(|path| std::fs::read_to_string(path).unwrap()),
        ),
    };
    let only_root = std::env::var("PRIMJS_DUMP_BTS").is_err();
    for section in primjs::bundle_bytecode_sections(&bundle).unwrap() {
        if only_root == (section.kind != primjs::SectionKind::RootLepus) {
            continue;
        }
        let (_, program) = primjs::read_payload(section.bytes).unwrap();
        println!("==== PrimJS {:?} {}", section.kind, section.name);
        println!("{}", primjs::disassemble(&program));
        let options = primjs::ConvertOptions::script("app:///dump.js");
        match primjs::convert_script(&program, &options, &verifier) {
            Ok(module) => {
                let converted = fork::read_program(module.bytes()).unwrap();
                println!("==== converted");
                println!("{}", fork::disassemble(&converted));
            }
            Err(error) => println!("==== conversion failed: {error}"),
        }
    }
    if let Some(source) = source {
        let module = std::env::var("PRIMJS_DUMP_MODULE").is_ok();
        let bytes = if module {
            realm
                .compile_module_bytecode("app:///m.js", &source)
                .unwrap()
        } else {
            realm.compile_script_bytecode("s.js", &source).unwrap()
        };
        println!("==== fork {}", if module { "module" } else { "script" });
        println!(
            "{}",
            fork::disassemble(&fork::read_program(&bytes).unwrap())
        );
    }
}
