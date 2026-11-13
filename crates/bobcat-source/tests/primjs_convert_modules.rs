//! Converted scripts as modules: the completion value as the default
//! export, imports and names bound to `undefined`, global redeclaration
//! at link time, `yield*` in an async generator, and debug information.
//! The inputs are `PrimJS` bytecode in `tests/fixtures/primjs/`, encoded by
//! `@lynx-js/tasm` 0.0.53 from the `.js` file of the same stem: `.mts.v1`
//! with inline debug info (target SDK 4.1, `debugInfoOutside: false`),
//! `.mts.v9` with external debug info (target SDK 2.14).

use std::time::Duration;

use bobcat_source::native::primjs::{self, Completion, ConvertOptions};
use quickjs_rust_bridge::{
    Context, EvalOptions, EvalSource, Runtime, RuntimeOptions, Value, ValueKind,
};

const PROVIDER: &str = "app:///provider.js";
const CONVERTED: &str = "app:///converted.js";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/primjs/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn source(stem: &str) -> String {
    String::from_utf8(fixture(&format!("{stem}.js"))).unwrap()
}

fn new_realm() -> (Runtime, Context) {
    let runtime = Runtime::with_options(RuntimeOptions {
        execution_timeout: Some(Duration::from_secs(10)),
        ..RuntimeOptions::default()
    })
    .unwrap();
    let realm = runtime.create_context().unwrap();
    (runtime, realm)
}

fn string(_realm: &Context, value: &Value) -> String {
    assert_eq!(value.kind(), ValueKind::String, "a string");
    String::from_utf16_lossy(&value.to_utf16().unwrap())
}

fn global(realm: &mut Context, name: &str) -> Value {
    let global = realm.global_object().unwrap();
    realm.property(&global, name).unwrap()
}

/// Converts `bytes`, registers and loads the module, drains jobs, and
/// answers the load's settled result.
fn load(
    runtime: &mut Runtime,
    realm: &Context,
    bytes: &[u8],
    options: &ConvertOptions<'_>,
) -> Result<(), quickjs_rust_bridge::Error> {
    let (_, program) = primjs::read_payload(bytes).unwrap();
    let converted = primjs::convert_script(&program, options, realm).unwrap();
    realm.complete_module_bytecode(CONVERTED, CONVERTED, &converted)?;
    let promise = realm.load_module(CONVERTED)?;
    runtime.drain_pending_jobs(realm)?;
    realm.settled_promise_result(&promise)?.expect("settled");
    Ok(())
}

#[test]
fn imports_undefined_names_and_the_default_export() {
    let stem = "convert_imports";
    // The fork's run of the source with the same bindings, as the realm's
    // preamble would give them, is the expected log.
    let provider = "export const lynx = { tag: 'L' }; export let counter = 0; \
                    export function bump() { counter++; } export const shadowed = 'import';";
    let expected_log =
        r#"["object","L",0,1,1,"TypeError","TypeError","script","undefined","TypeError"]"#;
    for variant in ["mts.v1", "mts.v9"] {
        let (mut runtime, mut realm) = new_realm();
        realm
            .complete_module(PROVIDER, Ok((PROVIDER, provider)))
            .unwrap();
        let imports: &[(&str, &[&str])] = &[(PROVIDER, &["lynx", "counter", "bump", "shadowed"])];
        let options = ConvertOptions {
            module_name: CONVERTED,
            imports,
            undefined_names: &["window"],
            completion: Completion::DefaultExport,
        };
        load(
            &mut runtime,
            &realm,
            &fixture(&format!("{stem}.{variant}.bin")),
            &options,
        )
        .unwrap_or_else(|error| panic!("{variant}: {error}"));
        let namespace = realm.module_namespace(CONVERTED).unwrap();
        let default = realm.property(&namespace, "default").unwrap();
        assert_eq!(string(&realm, &default), "completion:10", "{variant}");
        let json = realm
            .evaluate(
                EvalSource::new("JSON.stringify(globalThis.__log)"),
                EvalOptions::default(),
            )
            .unwrap();
        assert_eq!(string(&realm, &json), expected_log, "{variant}");
        // The script's own `var shadowed` is a global property, not the
        // import of the same name.
        let shadowed = global(&mut realm, "shadowed");
        assert_eq!(string(&realm, &shadowed), "script", "{variant}");
    }

    // The same names in the fork's own module of the source, with the
    // bindings declared as the preamble declares them.
    let (mut runtime, realm) = new_realm();
    realm
        .complete_module(PROVIDER, Ok((PROVIDER, provider)))
        .unwrap();
    let module = format!(
        "import {{ lynx, counter, bump }} from '{PROVIDER}'; const window = undefined; {}",
        source(stem).replace("var shadowed", "globalThis.shadowed")
    );
    realm
        .complete_module(CONVERTED, Ok((CONVERTED, &module)))
        .unwrap();
    let promise = realm.load_module(CONVERTED).unwrap();
    runtime.drain_pending_jobs(&realm).unwrap();
    realm.settled_promise_result(&promise).unwrap();
    let json = realm
        .evaluate(
            EvalSource::new("JSON.stringify(globalThis.__log)"),
            EvalOptions::default(),
        )
        .unwrap();
    assert_eq!(string(&realm, &json), expected_log);
}

/// A live binding: a converted function reads the import's current value
/// each time it runs.
#[test]
fn imports_are_live_bindings() {
    let provider = "export let counter = 0; export function bump() { counter++; } \
                    export const lynx = {}; export const shadowed = 0;";
    let (mut runtime, realm) = new_realm();
    realm
        .complete_module(PROVIDER, Ok((PROVIDER, provider)))
        .unwrap();
    let imports: &[(&str, &[&str])] = &[(PROVIDER, &["lynx", "counter", "bump", "shadowed"])];
    let options = ConvertOptions {
        module_name: CONVERTED,
        imports,
        undefined_names: &["window"],
        completion: Completion::Discard,
    };
    load(
        &mut runtime,
        &realm,
        &fixture("convert_imports.mts.v9.bin"),
        &options,
    )
    .unwrap();
    // `readCount` is a global function of the script that reads the
    // import; bump the provider's binding from outside and read again.
    for expected in [1.0, 2.0, 3.0] {
        let value = realm
            .evaluate(EvalSource::new("readCount()"), EvalOptions::default())
            .unwrap();
        assert_eq!(value.as_number(), Some(expected));
        realm
            .evaluate(
                EvalSource::new("import('app:///provider.js').then(m => m.bump())"),
                EvalOptions::default(),
            )
            .unwrap();
        runtime.drain_pending_jobs(&realm).unwrap();
    }
}

/// `PrimJS` checks global declarations at script start
/// (`check_define_var`); the converted module declares them as closure
/// variables, which the fork checks when the module links: redeclaring a
/// global lexical binding is a `SyntaxError` and the module does not run.
#[test]
fn redeclaring_a_global_lexical_binding_fails_the_load() {
    for variant in ["mts.v1", "mts.v9"] {
        let (mut runtime, mut realm) = new_realm();
        realm
            .evaluate(
                EvalSource::new("let existing = 'realm';"),
                EvalOptions::default(),
            )
            .unwrap();
        let error = load(
            &mut runtime,
            &realm,
            &fixture(&format!("convert_redeclare.{variant}.bin")),
            &ConvertOptions::script(CONVERTED),
        )
        .expect_err("the redeclaration is refused");
        assert_eq!(
            error.name.as_deref(),
            Some("SyntaxError"),
            "{variant}: {error}"
        );
        assert!(global(&mut realm, "__result").kind() == ValueKind::Undefined);

        // Without the conflicting binding the same bytes run.
        let (mut runtime, mut realm) = new_realm();
        load(
            &mut runtime,
            &realm,
            &fixture(&format!("convert_redeclare.{variant}.bin")),
            &ConvertOptions::script(CONVERTED),
        )
        .unwrap();
        let result = global(&mut realm, "__result");
        assert_eq!(string(&realm, &result), "script");
    }
}

/// `yield*` in an async generator over an iterator whose second `next()`
/// rejects: the rejection reaches the caller and `return()` is not
/// called, as in `PrimJS` (whose `iterator_get_value_done` leaves the
/// slot `yield*` cleared to `undefined`) and in the fork's own run of the
/// source.
#[test]
fn async_generator_yield_star_does_not_close_on_rejection() {
    let expected = r#"[1,"boom"]"#;
    let (mut runtime, mut realm) = new_realm();
    realm
        .evaluate(
            EvalSource::new(&source("convert_async_yield_star")),
            EvalOptions::default(),
        )
        .unwrap();
    runtime.drain_pending_jobs(&realm).unwrap();
    let result = global(&mut realm, "__result");
    assert_eq!(string(&realm, &result), expected);
    for variant in ["mts.v1", "mts.v9"] {
        let (mut runtime, mut realm) = new_realm();
        load(
            &mut runtime,
            &realm,
            &fixture(&format!("convert_async_yield_star.{variant}.bin")),
            &ConvertOptions::script(CONVERTED),
        )
        .unwrap();
        let result = global(&mut realm, "__result");
        assert_eq!(string(&realm, &result), expected, "{variant}");
    }
}

/// A thrown error's stack. With inline debug info the frames carry the
/// lines of the source; with external debug info they name the function
/// and the module, with no line.
#[test]
fn stack_traces_keep_function_names_and_inline_lines() {
    let stack = |variant: &str| {
        let (mut runtime, mut realm) = new_realm();
        load(
            &mut runtime,
            &realm,
            &fixture(&format!("convert_stack.{variant}.bin")),
            &ConvertOptions::script(CONVERTED),
        )
        .unwrap();
        let value = global(&mut realm, "__result");
        string(&realm, &value)
    };
    let inline = stack("mts.v1");
    let external = stack("mts.v9");
    eprintln!("inline:\n{inline}\nexternal:\n{external}");
    // The fork's own run of the source names the same lines; the columns
    // are where each compiler records the call (the fork at the opening
    // parenthesis, PrimJS at the end of the statement), so only lines
    // are compared.
    let (_runtime, mut realm) = new_realm();
    realm
        .evaluate(
            EvalSource {
                text: &source("convert_stack"),
                name: Some(CONVERTED),
                line_offset: 0,
            },
            EvalOptions::default(),
        )
        .unwrap();
    let reference = global(&mut realm, "__result");
    let reference = string(&realm, &reference);
    eprintln!("source:\n{reference}");
    let lines = |stack: &str| -> Vec<String> {
        stack
            .lines()
            .take(2)
            .map(|frame| {
                frame
                    .rsplit_once(':')
                    .map_or(frame, |(head, _)| head)
                    .to_owned()
            })
            .collect()
    };
    assert_eq!(lines(&inline), lines(&reference));
    assert!(lines(&inline)[0].ends_with(":3"), "{inline}");
    let thrower = external
        .lines()
        .find(|line| line.contains("thrower"))
        .unwrap();
    assert!(thrower.contains(CONVERTED), "{external}");
}
