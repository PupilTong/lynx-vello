//! Every bytecode section of both pages of every native `ReactLynx` fixture
//! converts, with the names of the realm's preamble as imports and names
//! bound to `undefined`, and the output passes the bridge's checks. The
//! sections are not executed here: that needs the engine's realms.
//!
//! The development builds' main-thread scripts contain `ReactLynx`'s
//! snapshot-patch `evaluate`, which calls `eval` directly (`special_object
//! 5; ...; eval 0`); direct `eval` is refused by ruling, so those sections
//! are expected to fail, and only with that refusal.

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;

use bobcat_core::{BTS_CHUNK_IMPORTS, BTS_CHUNK_UNDEFINED, MTS_CHUNK_IMPORTS};
use bobcat_source::native::primjs::{self, Completion, ConvertOptions, SectionKind};
use quickjs_rust_bridge::Runtime;
use quickjs_rust_bridge::bytecode::{self as fork, RegExpCompiler, Root};

#[test]
fn fixture_sections_convert_and_pass_the_checks() {
    let runtime = Runtime::new().unwrap();
    let realm = runtime.create_context().unwrap();
    let regexps: &dyn RegExpCompiler = &realm;
    let mut converted = 0usize;
    let mut failures = Vec::new();
    let mut refused_eval = Vec::new();
    for fixture in bytecode::FIXTURES {
        for (page, bundle) in [
            ("page", fixture.page),
            ("js-bytecode", fixture.js_bytecode_page),
        ] {
            for section in primjs::bundle_bytecode_sections(bundle).unwrap() {
                let label = format!(
                    "{} {page} {:?} {}",
                    fixture.name, section.kind, section.name
                );
                let (_, program) = primjs::read_payload(section.bytes).unwrap();
                let (imports, undefined_names, completion) = match section.kind {
                    SectionKind::JsBytecode => (
                        BTS_CHUNK_IMPORTS,
                        BTS_CHUNK_UNDEFINED,
                        Completion::DefaultExport,
                    ),
                    SectionKind::RootLepus => (MTS_CHUNK_IMPORTS, &[][..], Completion::Discard),
                    SectionKind::LepusChunk | SectionKind::CustomSection => {
                        (MTS_CHUNK_IMPORTS, &[][..], Completion::DefaultExport)
                    }
                };
                let options = ConvertOptions {
                    module_name: "app:///fixture.js",
                    imports,
                    undefined_names,
                    completion,
                };
                match primjs::convert_script(&program, &options, regexps) {
                    Ok(bytes) => {
                        if let Err(error) = fork::check(&bytes, regexps, Root::Module) {
                            failures.push(format!("{label}: check: {error}"));
                        } else {
                            converted += 1;
                        }
                    }
                    Err(primjs::ConvertError::Unsupported { opcode, .. })
                        if section.kind == SectionKind::RootLepus
                            && fixture.name.ends_with("-development")
                            && matches!(opcode, "special_object 5" | "eval") =>
                    {
                        refused_eval.push(format!("{} {page}", fixture.name));
                    }
                    Err(error) => failures.push(format!("{label}: {error}")),
                }
            }
        }
    }
    eprintln!(
        "{converted} fixture sections convert; {} refused for direct eval ({}); {} fail",
        refused_eval.len(),
        refused_eval.join(", "),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let development = bytecode::FIXTURES
        .iter()
        .filter(|fixture| fixture.name.ends_with("-development"))
        .count();
    assert_eq!(refused_eval.len(), development * 2);
}
