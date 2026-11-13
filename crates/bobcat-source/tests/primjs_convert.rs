//! The `PrimJS` converter by differential execution: every program of the
//! bytecode corpus of `reactlynx-test-fixtures`, in every encoder variant
//! and both sections, converted and run as a module of the fork, must
//! leave the same `globalThis.__result` as the fork's own evaluation of the
//! program's source, except the programs [`KNOWN_DIFFERENCES`] lists.

#[path = "../../../packages/reactlynx-test-fixtures/bytecode.rs"]
#[allow(
    clippy::doc_markdown,
    reason = "the registry is the fixture package's file"
)]
mod bytecode;

use std::time::Duration;

use bobcat_source::native::primjs::{self, ConvertOptions, SectionKind};
use quickjs_rust_bridge::{Context, EvalOptions, EvalSource, Runtime, RuntimeOptions, ValueKind};

/// What a run left behind: `globalThis.__result`, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// `__result` is this string.
    Result(String),
    /// `__result` is not a string: its kind.
    NotString(String),
    /// The script (or module) threw: the error's name and message.
    Threw(String),
    /// The section did not convert.
    Unconverted(String),
    /// The converted bytes were refused or did not load.
    Refused(String),
}

fn realm() -> (Runtime, Context) {
    let runtime = Runtime::with_options(RuntimeOptions {
        execution_timeout: Some(Duration::from_secs(10)),
        max_stack_size: Some(16 << 20),
        ..RuntimeOptions::default()
    })
    .expect("runtime");
    let realm = runtime.create_context().expect("realm");
    (runtime, realm)
}

fn read_result(realm: &mut Context) -> Outcome {
    let global = realm.global_object().expect("global object");
    let value = realm.property(&global, "__result").expect("__result");
    if value.kind() == ValueKind::String {
        Outcome::Result(String::from_utf16_lossy(&value.to_utf16().expect("string")))
    } else {
        Outcome::NotString(format!("{:?}", value.kind()))
    }
}

fn describe(error: &quickjs_rust_bridge::Error) -> String {
    format!(
        "{}: {}",
        error.name.as_deref().unwrap_or("?"),
        error.message
    )
}

/// The fork evaluates `source` as a script.
fn reference(source: &str) -> Outcome {
    let (mut runtime, mut realm) = realm();
    if let Err(error) = realm.evaluate(EvalSource::new(source), EvalOptions::default()) {
        return Outcome::Threw(describe(&error));
    }
    if let Err(error) = runtime.drain_pending_jobs(&realm) {
        return Outcome::Threw(describe(&error));
    }
    read_result(&mut realm)
}

/// The section converted, loaded as a module and evaluated.
fn subject(section: &[u8]) -> Outcome {
    let (mut runtime, mut realm) = realm();
    let (_, program) = primjs::read_payload(section).expect("the reader accepts every section");
    let name = "app:///converted.js";
    let bytes = match primjs::convert_script(&program, &ConvertOptions::script(name), &realm) {
        Ok(bytes) => bytes,
        Err(error) => return Outcome::Unconverted(error.to_string()),
    };
    if let Err(error) = realm.complete_module_bytecode(name, name, &bytes) {
        return Outcome::Refused(describe(&error));
    }
    let promise = match realm.load_module(name) {
        Ok(promise) => promise,
        Err(error) => return Outcome::Threw(describe(&error)),
    };
    if let Err(error) = runtime.drain_pending_jobs(&realm) {
        return Outcome::Threw(describe(&error));
    }
    match realm.settled_promise_result(&promise) {
        Ok(Some(_)) => {}
        Ok(None) => return Outcome::Threw("module evaluation still pending".to_owned()),
        Err(error) => return Outcome::Threw(describe(&error)),
    }
    read_result(&mut realm)
}

/// One program whose converted runs differ from the fork's evaluation of
/// its source, in every variant and both sections.
struct KnownDifference {
    program: &'static str,
    /// What every converted run leaves: a `__result` string, or (with
    /// `threw`) the error. Where a section lost names (stripped vardefs
    /// and closure-variable names), an error message can name the
    /// variable differently; `also` is the message then.
    outcome: &'static str,
    also: Option<&'static str>,
    threw: bool,
}

/// Programs that `PrimJS` compiled to different semantics than the fork
/// gives their source, or that the fork's own bytecode reader runs
/// differently from its source. The converter keeps `PrimJS`'s
/// instructions, so the converted run does what `PrimJS`'s bytecode says.
/// The evidence for each is the `PrimJS` disassembly
/// (`tests/primjs_convert_dump.rs` prints it).
const KNOWN_DIFFERENCES: &[KnownDifference] = &[
    // `null ?? 'a'` is `null; dup; is_undefined; if_false8 L; drop;
    // push_atom_value "a"; L:` (ROOT_LEPUS pc 46): the bytes test for
    // `undefined` only, so a null left side is kept. The fork's source
    // gives "a".
    KnownDifference {
        program: "nullish-coalescing",
        outcome: r#"[["null","b","0","","false","NaN","x","g","2"],[]]"#,
        also: None,
        threw: false,
    },
    // `(o?.m)()` is `get_var o; ...; get_field "m"; call0` (pc 262): the
    // parenthesised optional chain reads `m` with `get_field` and calls
    // it without `this`, so `this.v` is undefined. The fork's source keeps
    // `o` as `this` and gives "o".
    KnownDifference {
        program: "optional-chaining-call",
        outcome: r#"["o","undefined","undefined","undefined","o","o","undefined","TypeError"]"#,
        also: None,
        threw: false,
    },
    // `delete o?.a` is `get_var o; <null test>; get_field "a"; drop;
    // push_true` (pc 277..300): the property is read, not deleted, so
    // `'a' in o` stays true. The fork's source deletes it.
    KnownDifference {
        program: "optional-chaining-short-circuit",
        outcome: r#"["undefined","undefined","undefined",0,true,true,true]"#,
        also: None,
        threw: false,
    },
    // In `function f(a = x, b = () => a) { var x = 'body'; ... }` the
    // default of `a` is `get_loc2` (f pc 12), the body's `var x`, which
    // is still undefined, instead of the global `x`. The fork's source
    // reads the global ("outer").
    KnownDifference {
        program: "default-params-scope",
        outcome: r#"[["changed",null,"body"],["changed","given","body"],"ReferenceError"]"#,
        also: None,
        threw: false,
    },
    // `class S { static b = S.a + 1; ... }`: the static initializer reads
    // the class's inner binding with `get_var_ref_check` (initializer pc
    // 32), and the script stores that binding (`set_loc1`, pc 94) only
    // after calling the initializer (`call_method 0`, pc 90), so the read
    // raises the TDZ error. The fork's source initializes the binding
    // first.
    KnownDifference {
        program: "class-static-fields",
        outcome: "ReferenceError: S is not initialized",
        also: Some("ReferenceError: lexical variable is not initialized"),
        threw: true,
    },
    // The template object is a `BC_TAG_TEMPLATE_OBJECT` constant. The
    // fork's reader (`JS_ReadArray` in `vendor/quickjs/quickjs.c`) only
    // prevents extensions of it, where the fork's compiler and
    // `PrimJS`'s reader also make `length` read-only (`seal_template_obj`),
    // so `Object.isFrozen(strings)` is false. The fork's own bytecode of
    // the source gives the same result as the converted run, which
    // `template_objects_from_bytecode_are_not_frozen` checks.
    KnownDifference {
        program: "tagged-template",
        outcome: r#"[{"cooked":["a","b","\n",""],"raw":["a","b","\\n",""],"subs":[5,10,"s"],"frozen":false},true,"\\unicode",true,"a\\n1"]"#,
        also: None,
        threw: false,
    },
    KnownDifference {
        program: "tagged-template-nested",
        outcome: r#"[["a=| b=| c=| i=|\\t <1,2,10,0>","a=| b=| c=| i=|\\t <1,2,10,1>","arrow | <13>","outer | <nested | <10>>"],["a=| b=| c=| i=|\\t <2,3,20,0>","a=| b=| c=| i=|\\t <2,3,20,1>","arrow | <25>","outer | <nested | <20>>"],[true,true,false],10,"m:1"]"#,
        also: None,
        threw: false,
    },
];

impl KnownDifference {
    fn outcome(&self) -> Outcome {
        self.wrap(self.outcome)
    }

    fn wrap(&self, text: &str) -> Outcome {
        if self.threw {
            Outcome::Threw(text.to_owned())
        } else {
            Outcome::Result(text.to_owned())
        }
    }

    fn matches(&self, actual: &Outcome) -> bool {
        *actual == self.outcome() || self.also.is_some_and(|also| *actual == self.wrap(also))
    }
}

fn sections(bundle: &[u8]) -> Vec<(SectionKind, &[u8])> {
    primjs::bundle_bytecode_sections(bundle)
        .expect("sections")
        .into_iter()
        .map(|section| (section.kind, section.bytes))
        .collect()
}

/// Runs `body` on a thread with a stack deep enough for the corpus's
/// recursion in an unoptimized build.
fn with_large_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(body)
        .expect("thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

#[test]
fn corpus_runs_as_the_source_does() {
    with_large_stack(corpus_runs);
}

fn corpus_runs() {
    let filter = std::env::var("PRIMJS_CONVERT_FILTER").ok();
    let mut passed = 0usize;
    let mut total = 0usize;
    let mut failures = Vec::new();
    let mut known_seen = Vec::new();
    for program in bytecode::PROGRAMS {
        if filter
            .as_deref()
            .is_some_and(|filter| !program.name.contains(filter))
        {
            continue;
        }
        let expected = reference(program.source);
        assert!(
            matches!(expected, Outcome::Result(_)),
            "{}: the reference run gives {expected:?}",
            program.name
        );
        let known = KNOWN_DIFFERENCES
            .iter()
            .find(|known| known.program == program.name);
        for variant in bytecode::Variant::ALL {
            for (kind, bytes) in sections(program.bundle(variant)) {
                total += 1;
                let actual = subject(bytes);
                let label = format!("{} {} {kind:?}", program.name, variant.name());
                if let Some(known) = known {
                    if known.matches(&actual) && actual != expected {
                        known_seen.push(label);
                    } else {
                        failures.push(format!(
                            "{label}: listed as differing with {:?}, but gives {actual:?} (the \
                             source gives {expected:?})",
                            known.outcome()
                        ));
                    }
                } else if actual == expected {
                    passed += 1;
                } else {
                    failures.push(format!(
                        "{label}:\n  expected {expected:?}\n  actual   {actual:?}"
                    ));
                }
            }
        }
    }
    eprintln!(
        "{passed} of {total} runs match; {} listed differences; {} failures",
        known_seen.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    if filter.is_none() {
        assert_eq!(total, bytecode::PROGRAMS.len() * 4 * 2);
        assert_eq!(known_seen.len(), KNOWN_DIFFERENCES.len() * 4 * 2);
    }
}

/// The fork compiles `source` as a module, writes it as bytecode and runs
/// the bytecode through `complete_module_bytecode`.
fn fork_bytecode_run(source: &str) -> Outcome {
    let name = "app:///source.js";
    // Compiled in a realm of its own, so the realm that runs the bytes
    // has no compiled module of that name.
    let (_compiler_runtime, compiler) = realm();
    let bytes = compiler
        .compile_module_bytecode(name, source)
        .expect("the fork compiles the program");
    let (mut runtime, mut realm) = realm();
    realm
        .complete_module_bytecode(name, name, &bytes)
        .expect("the fork's bytecode passes");
    let promise = realm.load_module(name).expect("module loads");
    runtime.drain_pending_jobs(&realm).expect("jobs");
    if let Err(error) = realm.settled_promise_result(&promise) {
        return Outcome::Threw(describe(&error));
    }
    read_result(&mut realm)
}

/// The evidence for the two tagged-template entries of
/// [`KNOWN_DIFFERENCES`]: the fork's own bytecode of the source (compiled
/// as a module, which these programs allow) gives what the converted runs
/// give, so the difference is the fork's reader, not the conversion.
#[test]
fn template_objects_from_bytecode_are_not_frozen() {
    with_large_stack(|| {
        for name in ["tagged-template", "tagged-template-nested"] {
            let program = bytecode::PROGRAMS
                .iter()
                .find(|program| program.name == name)
                .expect("corpus program");
            let known = KNOWN_DIFFERENCES
                .iter()
                .find(|known| known.program == name)
                .expect("listed");
            assert_eq!(fork_bytecode_run(program.source), known.outcome(), "{name}");
            assert_ne!(reference(program.source), known.outcome(), "{name}");
        }
    });
}
