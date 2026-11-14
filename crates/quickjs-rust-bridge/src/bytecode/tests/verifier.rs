//! [`Verifier`] and [`VerifiedModule`]: when the engine is made, how often a
//! regular expression is compiled, what a refused literal answers, and one
//! module completing in realms of different runtimes and threads.

use super::realm;
use crate::bytecode::{
    CheckError, JsString, Program, RegExpError, Rule, Value, VerifiedModule, Verifier, read_program,
};
use crate::{Context, Runtime};

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Module bytecode the fork compiles for `source` under `name`.
fn compiled_module(name: &str, source: &str) -> Vec<u8> {
    let (_runtime, compiler) = realm();
    compiler.compile_module_bytecode(name, source).unwrap()
}

/// `result` of the module `name`, completed from `module` in `realm` and
/// evaluated.
fn run(runtime: &mut Runtime, realm: &mut Context, name: &str, module: &VerifiedModule) -> String {
    realm.complete_module_bytecode(name, name, module).unwrap();
    let promise = realm.load_module(name).unwrap();
    runtime.drain_pending_jobs(realm).unwrap();
    realm.settled_promise_result(&promise).unwrap().unwrap();
    let namespace = realm.module_namespace(name).unwrap();
    let result = realm.property(&namespace, "result").unwrap();
    String::from_utf16(&result.to_utf16().unwrap()).unwrap()
}

/// Every string constant of `program`'s functions equal to `from`, replaced
/// by `to`.
fn replace_string(program: &mut Program, from: &str, to: &str) {
    fn walk(value: &mut Value, from: &JsString, to: &JsString) {
        match value {
            Value::String(text) if text == from => *text = to.clone(),
            Value::Function(function) => {
                for constant in &mut function.cpool {
                    walk(constant, from, to);
                }
            }
            Value::Module(module) => {
                for constant in &mut module.function.cpool {
                    walk(constant, from, to);
                }
            }
            _ => {}
        }
    }
    walk(
        &mut program.root,
        &JsString::from_str(from),
        &JsString::from_str(to),
    );
}

#[test]
fn a_program_without_literals_makes_no_engine() {
    let verifier = Verifier::new();
    assert!(!verifier.has_engine());
    let bytes = compiled_module("app:///plain.js", "export const result = String(6 * 7);");
    let module = verifier.module_from_bytes(&bytes).unwrap();
    assert_eq!(module.bytes(), bytes.as_slice());
    assert!(!verifier.has_engine());
    assert_eq!(verifier.compiled_regexps(), 0);
}

#[test]
fn each_pattern_and_flags_compile_once() {
    let source = "const a = /a+/g; const b = () => /a+/g; const c = /a+/;\n\
                  function d() { return [/b(?=c)/u, /a+/g]; }\n\
                  export const result = [a.test('aa'), b().source, c.flags, d().length].join();";
    let bytes = compiled_module("app:///literals.js", source);
    let verifier = Verifier::new();

    // The converter asks for a literal's bytecode first; the rule that
    // compares it in the program costs nothing more.
    let (_runtime, realm) = realm();
    let asked = verifier.compile_regexp(&utf16("a+"), "g").unwrap();
    assert_eq!(
        asked,
        realm.compile_regexp_bytecode(&utf16("a+"), "g").unwrap()
    );
    assert!(verifier.has_engine());
    assert_eq!(verifier.compiled_regexps(), 1);
    assert_eq!(verifier.compile_regexp(&utf16("a+"), "g").unwrap(), asked);
    assert_eq!(verifier.compiled_regexps(), 1);

    // `/a+/g` three times, `/a+/` and `/b(?=c)/u`: three compilations, the
    // first already made.
    verifier.module_from_bytes(&bytes).unwrap();
    assert_eq!(verifier.compiled_regexps(), 3);
    // A second program with the same literals compiles nothing.
    verifier.module_from_bytes(&bytes).unwrap();
    assert_eq!(verifier.compiled_regexps(), 3);
}

#[test]
fn the_bare_engine_compiles_what_a_full_realm_compiles() {
    let verifier = Verifier::new();
    let (_runtime, realm) = realm();
    for (pattern, flags) in [
        ("a+b", "gi"),
        ("(\\d+)-(?<word>[a-z]+)", "y"),
        ("[\\u{1F600}-\\u{1F64F}]", "u"),
        ("é+[à-ÿ]", "i"),
        ("\\p{L}+\\P{Script=Greek}", "u"),
        ("[\\p{RGI_Emoji}--\\q{x}]", "v"),
        ("ſ|K", "iu"),
        ("x", "dgmsy"),
        ("", ""),
    ] {
        assert_eq!(
            verifier.compile_regexp(&utf16(pattern), flags),
            Ok(realm
                .compile_regexp_bytecode(&utf16(pattern), flags)
                .unwrap()),
            "/{pattern}/{flags}"
        );
    }
    assert_eq!(
        verifier.compile_regexp(&[0xd800], ""),
        Ok(realm.compile_regexp_bytecode(&[0xd800], "").unwrap())
    );
    for (pattern, flags) in [
        ("(", ""),
        ("a", "gg"),
        ("a", "uv"),
        ("a", "q"),
        ("[b-a]", ""),
    ] {
        assert!(
            realm
                .compile_regexp_bytecode(&utf16(pattern), flags)
                .is_err()
        );
        assert_eq!(
            verifier.compile_regexp(&utf16(pattern), flags),
            Err(RegExpError::Refused),
            "/{pattern}/{flags}"
        );
        // A refusal is kept too, and leaves the engine usable.
        assert_eq!(
            verifier.compile_regexp(&utf16(pattern), flags),
            Err(RegExpError::Refused)
        );
    }
    assert!(verifier.compile_regexp(&utf16("ok"), "").is_ok());
}

#[test]
fn a_literal_the_engine_refuses_is_refused_by_the_verifier() {
    let bytes = compiled_module("app:///refused.js", "export const result = /a+/g.source;");
    let mut program = read_program(&bytes).unwrap();
    replace_string(&mut program, "a+", "(");
    let error = Verifier::new()
        .module(&program)
        .expect_err("the pattern does not compile");
    let CheckError::Verify(error) = error else {
        panic!("refused by a rule: {error:?}");
    };
    assert_eq!(error.rule, Rule::RegExp, "{error}");
    assert_eq!(error.detail, "the engine refuses the regexp's pattern");
}

#[test]
fn a_verifier_without_an_engine_says_so() {
    let verifier = Verifier::without_engine("no memory");
    assert_eq!(
        verifier.compile_regexp(&utf16("a+"), "g"),
        Err(RegExpError::NoEngine("no memory".to_owned()))
    );
    let bytes = compiled_module("app:///literal.js", "export const result = /a+/g.source;");
    let Err(CheckError::Verify(error)) = verifier.module_from_bytes(&bytes) else {
        panic!("a literal needs the engine");
    };
    assert_eq!(error.rule, Rule::RegExp, "{error}");
    assert_eq!(
        error.detail,
        "no engine to compile regular expressions with: no memory"
    );
    // A program without literals does not need it.
    let plain = compiled_module("app:///plain.js", "export const result = 'plain';");
    assert!(verifier.module_from_bytes(&plain).is_ok());
}

#[test]
fn one_verified_module_completes_in_realms_of_two_runtimes_and_threads() {
    let name = "app:///shared.js";
    let bytes = compiled_module(
        name,
        "export const result = ['x1', 'y22'].map((s) => /\\d+/.exec(s)[0]).join();",
    );
    let module = Verifier::new().module_from_bytes(&bytes).unwrap();

    let (mut runtime, mut realm) = realm();
    assert_eq!(run(&mut runtime, &mut realm, name, &module), "1,22");

    let elsewhere = module.clone();
    let other = std::thread::spawn(move || {
        let mut runtime = Runtime::new().unwrap();
        let mut realm = runtime.create_context().unwrap();
        run(&mut runtime, &mut realm, name, &elsewhere)
    })
    .join()
    .unwrap();
    assert_eq!(other, "1,22");
}
