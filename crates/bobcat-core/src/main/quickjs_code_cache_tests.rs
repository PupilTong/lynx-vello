//! The code cache through real realms: what a realm's compile leaves in the
//! memory tier, and that a realm on another runtime reading it back runs the
//! same page — the same exports, its own `import.meta.url`, the same stack
//! frames and the same function text.
//!
//! Every test runs over a tier of its own, so what it asserts about the
//! tier is exact.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use bytes::Bytes;
use rustc_hash::FxHashMap;
use vant::artifact::Artifacts;

use super::*;
use crate::code_cache::script_engine_id;

/// Where the tests' own host members live: `report` and `load`.
const PROBE: &str = "bobcat-internal:code-cache-test";

/// Comment lines that take a text over the cache's threshold without
/// changing what it does.
fn padding() -> String {
    let line = format!("// padding {}\n", "x".repeat(64));
    line.repeat(16)
}

/// An ES module over the threshold that exports what the probes read.
fn module_text() -> String {
    format!(
        "export const answer = 42;\n\
         export const url = import.meta.url;\n\
         export function where() {{\n\
         \x20 return new Error('here').stack;\n\
         }}\n\
         export function shape(a, b) {{\n\
         \x20 return a + b; // kept as written\n\
         }}\n{}",
        padding()
    )
}

/// A `CommonJS` file over the threshold with the same three exports.
fn commonjs_text() -> String {
    format!(
        "exports.answer = 42;\n\
         exports.where = function () {{\n\
         \x20 return new Error('here').stack;\n\
         }};\n\
         exports.shape = function shape(a, b) {{\n\
         \x20 return a + b; // kept as written\n\
         }};\n{}",
        padding()
    )
}

/// What a synchronous load of a URL answers in these tests.
type Sources = Rc<RefCell<FxHashMap<String, quickjs::RequiredSource>>>;

/// One realm on a runtime of its own, over `artifacts`, with the probe
/// members registered.
struct Realm {
    runtime: ScriptRuntime,
    engine: ScriptEngine,
    reports: Rc<RefCell<Vec<String>>>,
    required: Sources,
    /// The modules this realm's imports are answered from: name to response
    /// URL and text.
    modules: FxHashMap<String, (String, String)>,
    /// Numbers the probe modules, each a module of its own.
    probes: usize,
}

impl Realm {
    fn new(artifacts: &Arc<Artifacts>) -> Self {
        let runtime =
            ScriptRuntime::with_artifacts(Arc::clone(artifacts)).expect("a QuickJS runtime");
        let mut engine = runtime.create_realm().expect("a QuickJS realm");
        engine.enable_module_loading();
        let reports = Rc::<RefCell<Vec<String>>>::default();
        let sink = Rc::clone(&reports);
        engine
            .register_host_module_function(PROBE, "report", 1, move |arguments| {
                match arguments.first() {
                    Some(&quickjs::HostArgument::String(text)) => {
                        sink.borrow_mut().push(text.to_owned());
                        Ok(quickjs::HostValue::Undefined)
                    }
                    _ => Err("report expects a string".to_owned()),
                }
            })
            .expect("register report");
        let required = Sources::default();
        let table = Rc::clone(&required);
        engine
            .register_synchronous_loader(PROBE, "load", move |url| {
                table
                    .borrow()
                    .get(url)
                    .cloned()
                    .ok_or_else(|| format!("nothing at {url}"))
            })
            .expect("register load");
        Self {
            runtime,
            engine,
            reports,
            required,
            modules: FxHashMap::default(),
            probes: 0,
        }
    }

    fn serve(&mut self, name: &str, url: &str, text: &str) {
        self.modules
            .insert(name.to_owned(), (url.to_owned(), text.to_owned()));
    }

    fn serve_required(&self, asked: &str, url: &str, text: &str, kind: quickjs::RequiredKind) {
        self.required.borrow_mut().insert(
            asked.to_owned(),
            quickjs::RequiredSource {
                url: url.to_owned(),
                text: text.to_owned(),
                kind,
            },
        );
    }

    /// Answers every module request from what [`Self::serve`] set up, until
    /// none is left.
    fn answer_requests(&mut self) {
        while let Some(name) = self.engine.take_module_request() {
            let (url, text) = self
                .modules
                .get(&name)
                .cloned()
                .unwrap_or_else(|| panic!("no module is served as {name}"));
            self.engine
                .complete_module(&mut self.runtime, &name, Ok((&url, &text)))
                .expect("the module completes");
        }
    }

    /// Imports `name` dynamically and answers the whole graph.
    fn import(&mut self, name: &str) {
        self.engine
            .execute_script(
                &mut self.runtime,
                &format!(
                    "import('{name}').then(() => {{}}, error => {{ globalThis.failed = String(error); }});"
                ),
                "app:///main.js",
            )
            .expect("the import is asked for");
        self.answer_requests();
    }

    /// Runs `body` in a module that imports [`PROBE`]'s members, and answers
    /// with the one string it reported.
    fn probe(&mut self, body: &str) -> String {
        self.probes += 1;
        self.engine
            .execute_module(
                &mut self.runtime,
                &format!(
                    "import {{ report, load }} from '{PROBE}';\n\
                     if (globalThis.failed) throw new Error(globalThis.failed);\n{body}"
                ),
                &format!("app:///probe-{}.js", self.probes),
            )
            .expect("the probe runs");
        self.reports.borrow_mut().pop().expect("the probe reported")
    }

    /// What a page sees of the module `name`, whose exports are
    /// [`module_text`]'s.
    fn module_view(&mut self, name: &str) -> String {
        self.probe(&format!(
            "import * as loaded from '{name}';\n\
             report(JSON.stringify([loaded.answer, loaded.url, loaded.where(), String(loaded.shape)]));"
        ))
    }

    /// What a page sees of a `CommonJS` file required as `asked` with
    /// `parameters`, whose exports are [`commonjs_text`]'s.
    fn commonjs_view(&mut self, asked: &str, parameters: &str) -> String {
        self.probe(&format!(
            "const loaded = load('{asked}', '{parameters}');\n\
             if (loaded.kind !== 'commonjs') throw new Error('kind ' + loaded.kind);\n\
             const module = {{ exports: {{}} }};\n\
             loaded.value(module.exports, module);\n\
             const ex = module.exports;\n\
             report(JSON.stringify([loaded.url, ex.answer, ex.where(), String(ex.shape)]));"
        ))
    }

    fn code(&self) -> &RealmCodeCache {
        &self.engine.code
    }
}

#[test]
fn a_second_realm_reads_the_module_bytecode_the_first_one_compiled() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let text = module_text();

    let mut first = Realm::new(&artifacts);
    first.serve("app:///lib.js", "https://a.test/lib.js", &text);
    first.import("app:///lib.js");
    let first_view = first.module_view("app:///lib.js");
    let key = first.code().module_key_of("app:///lib.js", &text);
    assert_eq!(artifacts.len(), 1, "one artifact for the one module");
    assert!(artifacts.contains(script_engine_id(), key));
    assert_eq!(artifacts.take_pending().len(), 1);

    let mut second = Realm::new(&artifacts);
    second.serve("app:///lib.js", "https://b.test/lib.js", &text);
    second.import("app:///lib.js");
    let second_view = second.module_view("app:///lib.js");
    assert!(
        artifacts.take_pending().is_empty(),
        "the second realm read the bytecode and compiled nothing"
    );
    assert_eq!(artifacts.len(), 1);

    // The same exports, stack and function text; each realm's own
    // `import.meta.url`.
    assert_eq!(
        first_view.replace("https://a.test/lib.js", "<url>"),
        second_view.replace("https://b.test/lib.js", "<url>"),
    );
    assert!(first_view.contains("\"https://a.test/lib.js\""));
    assert!(second_view.contains("\"https://b.test/lib.js\""));
    assert!(second_view.contains("app:///lib.js:4"));
    assert!(second_view.contains("kept as written"));
}

#[test]
fn the_same_text_under_another_name_is_another_artifact() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let text = module_text();
    let mut realm = Realm::new(&artifacts);
    realm.serve("app:///one.js", "https://a.test/one.js", &text);
    realm.serve("app:///two.js", "https://a.test/two.js", &text);
    realm.import("app:///one.js");
    realm.import("app:///two.js");
    let one = realm.module_view("app:///one.js");
    let two = realm.module_view("app:///two.js");
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts.contains(
        script_engine_id(),
        realm.code().module_key_of("app:///one.js", &text)
    ));
    assert!(artifacts.contains(
        script_engine_id(),
        realm.code().module_key_of("app:///two.js", &text)
    ));
    assert!(one.contains("app:///one.js:4"));
    assert!(two.contains("app:///two.js:4"));
}

#[test]
fn a_text_under_the_threshold_is_never_cached() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let text = "export const answer = 42;\nexport const url = import.meta.url;\n\
                export function where() { return new Error('here').stack; }\n\
                export function shape(a, b) { return a + b; }\n";
    assert!(text.len() < crate::code_cache::MIN_CACHED_SOURCE_BYTES);
    let mut realm = Realm::new(&artifacts);
    realm.serve("app:///small.js", "https://a.test/small.js", text);
    realm.import("app:///small.js");
    let view = realm.module_view("app:///small.js");
    assert!(view.starts_with("[42,"));
    assert!(artifacts.is_empty());
    assert!(!artifacts.has_pending());
    assert_eq!(
        realm.code().handed_over_len(),
        0,
        "the text was never hashed"
    );
}

#[test]
fn a_tier_with_no_budget_caches_nothing() {
    let artifacts = Arc::new(Artifacts::new(0));
    let mut realm = Realm::new(&artifacts);
    realm.serve("app:///lib.js", "https://a.test/lib.js", &module_text());
    realm.serve_required(
        "app:///lib.cjs",
        "https://a.test/lib.cjs",
        &commonjs_text(),
        quickjs::RequiredKind::CommonJs,
    );
    realm.import("app:///lib.js");
    let view = realm.module_view("app:///lib.js");
    let required = realm.commonjs_view("app:///lib.cjs", "exports, module");
    assert!(view.starts_with("[42,"));
    assert!(required.contains(",42,"));
    assert!(artifacts.is_empty());
    assert!(!artifacts.has_pending());
    assert_eq!(realm.code().handed_over_len(), 0, "nothing was hashed");
    assert!(
        realm.engine.realm.take_compiled().is_none(),
        "nothing was kept"
    );
}

#[test]
fn a_required_commonjs_file_is_read_back_by_a_second_realm_under_the_same_parameters() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let text = commonjs_text();
    let serve = |realm: &Realm| {
        realm.serve_required(
            "app:///lib.cjs",
            "https://a.test/lib.cjs",
            &text,
            quickjs::RequiredKind::CommonJs,
        );
    };

    let mut first = Realm::new(&artifacts);
    serve(&first);
    let first_view = first.commonjs_view("app:///lib.cjs", "exports, module");
    let key = first
        .code()
        .commonjs_key_of("https://a.test/lib.cjs", "exports, module", &text);
    assert_eq!(artifacts.len(), 1);
    assert!(artifacts.contains(script_engine_id(), key));
    assert_eq!(artifacts.take_pending().len(), 1);

    let mut second = Realm::new(&artifacts);
    serve(&second);
    let second_view = second.commonjs_view("app:///lib.cjs", "exports, module");
    assert_eq!(first_view, second_view);
    assert!(second_view.contains("https://a.test/lib.cjs:3"));
    assert!(second_view.contains("kept as written"));
    assert!(artifacts.take_pending().is_empty(), "the wrapper was read");

    // Another parameter list is another wrapper: a miss, compiled and kept.
    let mut third = Realm::new(&artifacts);
    serve(&third);
    let third_view = third.commonjs_view("app:///lib.cjs", "exports, module, require");
    assert_eq!(first_view, third_view);
    let pending = artifacts.take_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].1,
        third
            .code()
            .commonjs_key_of("https://a.test/lib.cjs", "exports, module, require", &text)
    );
    assert_eq!(artifacts.len(), 2);
}

#[test]
fn a_required_file_detected_as_an_es_module_is_cached_as_a_module() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let text = module_text();
    let required = |realm: &mut Realm| {
        realm.serve_required(
            "app:///either.js",
            "https://a.test/either.js",
            &text,
            quickjs::RequiredKind::Detect,
        );
        realm.probe(
            "const loaded = load('app:///either.js', 'exports, module');\n\
             if (loaded.kind !== 'module') throw new Error('kind ' + loaded.kind);\n\
             const ns = loaded.value;\n\
             report(JSON.stringify([ns.answer, ns.url, ns.where(), String(ns.shape)]));",
        )
    };

    let mut first = Realm::new(&artifacts);
    let first_view = required(&mut first);
    assert_eq!(artifacts.len(), 1);
    assert!(artifacts.contains(
        script_engine_id(),
        first.code().module_key_of("app:///either.js", &text)
    ));
    assert_eq!(artifacts.take_pending().len(), 1);

    let mut second = Realm::new(&artifacts);
    let second_view = required(&mut second);
    assert_eq!(first_view, second_view);
    assert!(second_view.contains("app:///either.js:4"));
    assert!(artifacts.take_pending().is_empty(), "the module was read");
}

#[test]
fn json_is_never_hashed_or_cached() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let mut realm = Realm::new(&artifacts);
    let json = format!("{{\"answer\": 42, \"padding\": \"{}\"}}", "x".repeat(2048));
    realm.serve_required(
        "app:///config.json",
        "https://a.test/config.json",
        &json,
        quickjs::RequiredKind::Json,
    );
    let view = realm.probe(
        "const loaded = load('app:///config.json', 'unread');\n\
         report(String(loaded.value.answer));",
    );
    assert_eq!(view, "42");
    assert!(artifacts.is_empty());
    assert_eq!(realm.code().handed_over_len(), 0);
}

#[test]
fn a_corrupt_artifact_falls_back_to_the_text_and_is_replaced_by_a_working_one() {
    let text = module_text();
    let baseline = {
        let artifacts = Arc::new(Artifacts::new(0));
        let mut realm = Realm::new(&artifacts);
        realm.serve("app:///lib.js", "https://a.test/lib.js", &text);
        realm.import("app:///lib.js");
        realm.module_view("app:///lib.js")
    };

    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let mut realm = Realm::new(&artifacts);
    let key = realm.code().module_key_of("app:///lib.js", &text);
    let garbage = Bytes::from_static(b"not bytecode at all, and certainly not this module's");
    artifacts.insert(script_engine_id(), key, garbage.clone());
    let _ = artifacts.take_pending();

    realm.serve("app:///lib.js", "https://a.test/lib.js", &text);
    realm.import("app:///lib.js");
    assert_eq!(realm.module_view("app:///lib.js"), baseline);
    let fresh = artifacts
        .get(script_engine_id(), key)
        .expect("the text's compile is resident");
    assert_ne!(fresh, garbage);
    assert_eq!(artifacts.take_pending().len(), 1);

    let mut next = Realm::new(&artifacts);
    next.serve("app:///lib.js", "https://a.test/lib.js", &text);
    next.import("app:///lib.js");
    assert_eq!(next.module_view("app:///lib.js"), baseline);
    assert!(
        artifacts.take_pending().is_empty(),
        "the fresh artifact was read"
    );
}

#[test]
fn every_module_of_an_import_graph_is_cached_on_its_own() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let dependency = module_text();
    let entry = format!(
        "import * as dependency from 'app:///dependency.js';\n\
         export const answer = dependency.answer + 1;\n\
         export const url = import.meta.url;\n\
         export const where = dependency.where;\n\
         export function shape(a, b) {{ return dependency.shape(a, b) * 2; }}\n{}",
        padding()
    );
    let run = |artifacts: &Arc<Artifacts>| {
        let mut realm = Realm::new(artifacts);
        realm.serve("app:///entry.js", "https://a.test/entry.js", &entry);
        realm.serve(
            "app:///dependency.js",
            "https://a.test/dependency.js",
            &dependency,
        );
        realm.import("app:///entry.js");
        let view = realm.module_view("app:///entry.js");
        (realm, view)
    };

    let (first, first_view) = run(&artifacts);
    assert_eq!(artifacts.len(), 2);
    for (name, text) in [
        ("app:///entry.js", &entry),
        ("app:///dependency.js", &dependency),
    ] {
        assert!(
            artifacts.contains(script_engine_id(), first.code().module_key_of(name, text)),
            "{name} is cached"
        );
    }
    assert_eq!(artifacts.take_pending().len(), 2);

    let (_second, second_view) = run(&artifacts);
    assert_eq!(first_view, second_view);
    assert!(second_view.starts_with("[43,"));
    assert!(
        artifacts.take_pending().is_empty(),
        "both modules were read"
    );
}

/// A worker realm's root is loaded the way an `import()` of its URL is
/// ([`ScriptEngine::load_root_module`]), and completed as any module.
#[test]
fn a_worker_root_module_is_cached_as_a_module() {
    let artifacts = Arc::new(Artifacts::new(Artifacts::DEFAULT_BUDGET_BYTES));
    let text = module_text();
    let boot = |artifacts: &Arc<Artifacts>| {
        let mut realm = Realm::new(artifacts);
        realm.serve(
            "https://a.test/worker.js",
            "https://a.test/worker.js",
            &text,
        );
        realm
            .engine
            .load_root_module(&mut realm.runtime, "https://a.test/worker.js")
            .expect("the root load starts");
        realm.answer_requests();
        assert!(
            realm.engine.module_finished().expect("the root settles"),
            "the root module finished"
        );
        let view = realm.module_view("https://a.test/worker.js");
        (realm, view)
    };

    let (first, first_view) = boot(&artifacts);
    assert!(
        artifacts.contains(
            script_engine_id(),
            first
                .code()
                .module_key_of("https://a.test/worker.js", &text)
        )
    );
    assert_eq!(artifacts.take_pending().len(), 1);

    let (_second, second_view) = boot(&artifacts);
    assert_eq!(first_view, second_view);
    assert!(artifacts.take_pending().is_empty(), "the root was read");
}
