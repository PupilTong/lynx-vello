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

use std::fmt::Write as _;

use bobcat_core::{BTS_CHUNK_PREAMBLE, MTS_CHUNK_PREAMBLE};
use bobcat_source::native::primjs::{self, Completion, ConvertOptions, SectionKind};
use quickjs_rust_bridge::Runtime;
use quickjs_rust_bridge::bytecode::{self as fork, RegExpCompiler, Root};

/// The bindings a preamble declares: `import { a, b } from "spec";`
/// statements and one `const a = undefined, b = undefined;` statement.
#[derive(Debug, Default, PartialEq, Eq)]
struct Preamble {
    imports: Vec<(String, Vec<String>)>,
    undefined: Vec<String>,
}

impl Preamble {
    /// Parses a preamble of exactly those two statement forms; anything
    /// else is an error, so a change of the constants is noticed.
    fn parse(text: &str) -> Result<Self, String> {
        let mut preamble = Self::default();
        for statement in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(rest) = statement.strip_prefix("import {") {
                let (names, from) = rest
                    .split_once("} from ")
                    .ok_or_else(|| format!("unexpected import: {statement}"))?;
                let specifier = from
                    .trim()
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .ok_or_else(|| format!("unquoted specifier: {statement}"))?;
                let names = names
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| {
                        if name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
                        {
                            Ok(name.to_owned())
                        } else {
                            Err(format!("not a plain import: {name}"))
                        }
                    })
                    .collect::<Result<_, _>>()?;
                preamble.imports.push((specifier.to_owned(), names));
            } else if let Some(rest) = statement.strip_prefix("const ") {
                for binding in rest.split(',') {
                    let name = binding
                        .trim()
                        .strip_suffix("= undefined")
                        .ok_or_else(|| format!("not bound to undefined: {binding}"))?;
                    preamble.undefined.push(name.trim().to_owned());
                }
            } else {
                return Err(format!("unexpected statement: {statement}"));
            }
        }
        Ok(preamble)
    }

    /// The preamble text this parse stands for, in the constants' layout.
    fn render(&self) -> String {
        let mut out = String::new();
        for (specifier, names) in &self.imports {
            let _ = write!(
                out,
                "import {{ {} }} from \"{specifier}\"; ",
                names.join(", ")
            );
        }
        if !self.undefined.is_empty() {
            let bindings: Vec<String> = self
                .undefined
                .iter()
                .map(|name| format!("{name} = undefined"))
                .collect();
            let _ = write!(out, "const {}; ", bindings.join(", "));
        }
        out
    }
}

#[test]
fn preambles_parse_into_their_bindings() {
    for text in [MTS_CHUNK_PREAMBLE, BTS_CHUNK_PREAMBLE] {
        let preamble = Preamble::parse(text).unwrap();
        assert!(!preamble.imports.is_empty());
        assert_eq!(
            preamble.render(),
            text,
            "the parse accounts for every binding"
        );
    }
    assert!(
        Preamble::parse(BTS_CHUNK_PREAMBLE)
            .unwrap()
            .undefined
            .contains(&"window".to_owned())
    );
    assert!(Preamble::parse("let x = 1;").is_err());
    assert!(Preamble::parse("import x from \"y\";").is_err());
}

#[test]
fn fixture_sections_convert_and_pass_the_checks() {
    let runtime = Runtime::new().unwrap();
    let realm = runtime.create_context().unwrap();
    let regexps: &dyn RegExpCompiler = &realm;
    let mts = Preamble::parse(MTS_CHUNK_PREAMBLE).unwrap();
    let bts = Preamble::parse(BTS_CHUNK_PREAMBLE).unwrap();
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
                let (preamble, completion) = match section.kind {
                    SectionKind::JsBytecode => (&bts, Completion::DefaultExport),
                    SectionKind::RootLepus => (&mts, Completion::Discard),
                    SectionKind::LepusChunk | SectionKind::CustomSection => {
                        (&mts, Completion::DefaultExport)
                    }
                };
                let names: Vec<(&str, Vec<&str>)> = preamble
                    .imports
                    .iter()
                    .map(|(spec, names)| {
                        (spec.as_str(), names.iter().map(String::as_str).collect())
                    })
                    .collect();
                let imports: Vec<(&str, &[&str])> = names
                    .iter()
                    .map(|(spec, names)| (*spec, names.as_slice()))
                    .collect();
                let undefined: Vec<&str> = preamble.undefined.iter().map(String::as_str).collect();
                let options = ConvertOptions {
                    module_name: "app:///fixture.js",
                    imports: &imports,
                    undefined_names: &undefined,
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
