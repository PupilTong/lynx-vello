//! Module bytecode end to end: a fetcher answering
//! [`LoadedSource::ModuleBytecode`] with bytecode the engine's own compiler
//! wrote (`Context::compile_module_bytecode`), on a real view's main-thread
//! realm and its background Worker realm. Each realm boots from a bytecode
//! entry, `import()`s a bytecode module, `require`s one from its running
//! entry body, is refused bytes that are not a module on both paths, and the
//! main thread runs a chunk answered as bytecode through `__LoadLepusChunk`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest,
};
use bobcat_core::{
    DrawTarget, EngineEvent, LynxGroup, MTS_CHUNK_IMPORTS, MTS_CHUNK_PREAMBLE, NoWakeup, Painter,
    StyleThreads, ViewSources,
};

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";

/// The card's entry, a card body as `bobcat-source` writes one (the MTS
/// preamble in front), compiled to bytecode.
const MAIN_BODY: &str = r"
import { createRequire } from 'bobcat:module';
const require = createRequire(import.meta.url);
const dep = require('./dep.js');
let refused;
try {
  require('./refused.js');
  refused = 'loaded';
} catch (error) {
  refused = error instanceof SyntaxError
    && error.message.includes(`bytecode rejected for 'app:///refused.js'`);
}
console.log(['main', dep.value, refused].join(' '));
import('./lazy.js').then(
  (module) => console.log('main import ' + module.value),
  (error) => console.log('main import failed ' + error.message));
import('./refused-async.js').then(
  () => console.log('main refused-async loaded'),
  (error) => console.log('main refused-async '
    + error.message.includes(`bytecode rejected for 'app:///refused-async.js'`)));
__LoadLepusChunk('chunk', {});
__LoadLepusChunk('chunk', {});
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// A chunk as the converter writes one: a module whose default export is the
/// chunk's script, run with `this` = `globalThis` on every call.
const CHUNK_BODY: &str =
    "export default function () { console.log('chunk ' + (this === globalThis)); }";

/// The background entry, which imports its own bindings as a BTS entry that
/// is not a card's body does.
const BACKGROUND_BODY: &str = r"
import { console } from 'bobcat:bts-runtime';
import { createRequire } from 'bobcat:module';
const require = createRequire(import.meta.url);
const dep = require('./bts/dep.js');
let refused;
try {
  require('./bts/refused.js');
  refused = 'loaded';
} catch (error) {
  refused = error instanceof SyntaxError
    && error.message.includes(`bytecode rejected for 'app:///bts/refused.js'`);
}
console.log(['background', dep.value, refused].join(' '));
import('./bts/lazy.js').then(
  (module) => console.log('background import ' + module.value),
  (error) => console.log('background import failed ' + error.message));
import('./bts/refused-async.js').then(
  () => console.log('background refused-async loaded'),
  (error) => console.log('background refused-async '
    + error.message.includes(`bytecode rejected for 'app:///bts/refused-async.js'`)));
";

/// What a URL answers with.
#[derive(Clone)]
enum Answer {
    Text(String),
    Bytecode(Vec<u8>),
}

struct Served {
    answers: Arc<HashMap<String, Answer>>,
}

impl bobcat_core::FrameImages for Served {
    fn read(
        &self,
        _source: &str,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::peniko::ImageData> {
        None
    }

    fn retain(&self, _frame: &[Arc<str>]) {}
}

impl ResourceFetcher for Served {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let url = match &request {
            SourceRequest::Module(url)
            | SourceRequest::StyleSheet(url)
            | SourceRequest::Font { url }
            | SourceRequest::Fetch { url, .. } => url.clone(),
        };
        let answer = match (&request, self.answers.get(&url)) {
            (SourceRequest::Module(_), Some(Answer::Bytecode(bytes))) => {
                Ok(LoadedSource::ModuleBytecode {
                    bytes: bytes::Bytes::from(bytes.clone()),
                    url: url.clone(),
                })
            }
            (SourceRequest::Module(_), Some(Answer::Text(text))) => Ok(LoadedSource::Module {
                source: text.clone(),
                url: url.clone(),
            }),
            _ => Err(ResourceError {
                kind: ResourceErrorKind::NotFound,
                phase: ResourceErrorPhase::Resolve,
                locator: Some(Arc::from(url.as_str())),
                message: "this page has no such file".into(),
                retry: RetryAdvice::Never,
            }
            .into()),
        };
        completion.complete(answer);
    }
}

/// The engine's own module bytecode for `source`, named `url`.
fn compiled(url: &str, source: &str) -> Answer {
    let runtime = quickjs_rust_bridge::Runtime::new().unwrap();
    let compiler = runtime.create_context().unwrap();
    Answer::Bytecode(compiler.compile_module_bytecode(url, source).unwrap())
}

/// Script bytecode: well-formed, but not a module, so every load refuses it.
fn script_bytecode() -> Answer {
    let runtime = quickjs_rust_bridge::Runtime::new().unwrap();
    let compiler = runtime.create_context().unwrap();
    Answer::Bytecode(compiler.compile_script_bytecode("script.js", "1").unwrap())
}

/// Boots one view over `answers` and collects what its realms logged, until
/// `expected` lines have arrived.
async fn logs_of(
    main: &str,
    background: Option<&str>,
    answers: HashMap<String, Answer>,
    expected: usize,
) -> Vec<String> {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut sources = ViewSources::new("app:///", main, SCREEN, "en-US");
    sources.background_entry = background.map(str::to_owned);
    let answers = Arc::new(answers);
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            move |_reports| Served { answers },
            Vec::new(),
            sources,
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut logged = Vec::new();
    let mut booted = false;
    while !booted || logged.len() < expected {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } => logged.push(message),
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::WorkerThrew { error, .. }
                | EngineEvent::WorkerEnded { error, .. }
                | EngineEvent::ScriptRunError(error)
                | EngineEvent::Panicked(error) => {
                    panic!("the realm failed: {}", error.message)
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "the realms logged everything: {logged:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    logged.sort();
    logged
}

#[tokio::test]
async fn both_realms_load_module_bytecode_on_every_path() {
    let mut answers = HashMap::new();
    answers.insert(
        MAIN_URL.to_owned(),
        compiled(MAIN_URL, &format!("{MTS_CHUNK_PREAMBLE}{MAIN_BODY}")),
    );
    answers.insert(
        "app:///dep.js".to_owned(),
        compiled("app:///dep.js", "export const value = 41 + 1;"),
    );
    answers.insert(
        "app:///lazy.js".to_owned(),
        compiled("app:///lazy.js", "export const value = 7;"),
    );
    answers.insert("app:///refused.js".to_owned(), script_bytecode());
    answers.insert("app:///refused-async.js".to_owned(), script_bytecode());
    // `__LoadLepusChunk` loads a chunk at its section URL under the card's.
    answers.insert(
        "app:///main.js/chunk.js".to_owned(),
        compiled(
            "app:///main.js/chunk.js",
            &format!("{MTS_CHUNK_PREAMBLE}{CHUNK_BODY}"),
        ),
    );
    answers.insert(
        BACKGROUND_URL.to_owned(),
        compiled(BACKGROUND_URL, BACKGROUND_BODY),
    );
    answers.insert(
        "app:///bts/dep.js".to_owned(),
        compiled("app:///bts/dep.js", "export const value = 43;"),
    );
    answers.insert(
        "app:///bts/lazy.js".to_owned(),
        compiled("app:///bts/lazy.js", "export const value = 8;"),
    );
    answers.insert("app:///bts/refused.js".to_owned(), script_bytecode());
    answers.insert("app:///bts/refused-async.js".to_owned(), script_bytecode());

    assert_eq!(
        logs_of(MAIN_URL, Some(BACKGROUND_URL), answers, 8).await,
        [
            "background 43 true",
            "background import 8",
            "background refused-async true",
            "chunk true",
            "chunk true",
            "main 42 true",
            "main import 7",
            "main refused-async true",
        ]
    );
}

/// An entry answered with bytes that are not a module boots no further than
/// a text entry that does not parse: boot's import of it rejects with the
/// reason, which is the app's failure, and the view goes on.
#[tokio::test]
async fn a_refused_bytecode_entry_rejects_boots_import_with_the_reason() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut answers = HashMap::new();
    answers.insert(MAIN_URL.to_owned(), script_bytecode());
    let answers = Arc::new(answers);
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            move |_reports| Served { answers },
            Vec::new(),
            ViewSources::new("app:///", MAIN_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut reason = None;
    while reason.is_none() {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptRunError(error) => reason = Some(error.message.to_string()),
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::Panicked(error) => panic!("panicked: {}", error.message),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the entry's import was refused");
        std::thread::sleep(Duration::from_millis(1));
    }
    let reason = reason.unwrap();
    assert!(
        reason.contains("bytecode rejected for 'app:///main.js'"),
        "{reason}"
    );
}

/// `__LoadLepusChunk` passes a source chunk every name in `CHUNK_BINDINGS`
/// (`packages/bobcat-element/src/main-thread-runtime.ts`) as a parameter,
/// where a card body, and a chunk written as module bytecode, has the names
/// `MTS_CHUNK_PREAMBLE` imports. The bindings are every export of the real
/// `bobcat:element` and thirteen runtime names; this compares both halves
/// with the preamble's two import lists. The element half differs today:
/// `CSSStyleValue`, `Document` and the element module's internal exports
/// (`__Bobcat*`) are parameters of a source chunk and not imports of a card
/// body, so this pins that difference rather than equality.
#[tokio::test]
async fn chunk_bindings_and_the_mts_preamble_name_the_same_bindings() {
    const ENTRY: &str = r"
import * as element from 'bobcat:element';
console.log('element ' + JSON.stringify(Object.keys(element)));
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";
    let mut answers = HashMap::new();
    answers.insert(
        MAIN_URL.to_owned(),
        Answer::Text(format!("{MTS_CHUNK_PREAMBLE}{ENTRY}")),
    );
    let logged = logs_of(MAIN_URL, None, answers, 1).await;
    let exports: Vec<String> =
        serde_json::from_str(logged[0].strip_prefix("element ").unwrap()).unwrap();
    let imports = |specifier: &str| -> Vec<&str> {
        MTS_CHUNK_IMPORTS
            .iter()
            .find(|(name, _)| *name == specifier)
            .map(|(_, names)| names.to_vec())
            .unwrap()
    };
    // The element half: every name the preamble imports is a binding...
    let element = imports("bobcat:element");
    let missing: Vec<&&str> = element
        .iter()
        .filter(|name| !exports.iter().any(|export| export == **name))
        .collect();
    assert!(
        missing.is_empty(),
        "imported but not a chunk binding: {missing:?}"
    );
    // ...and these are the bindings it does not import: a source chunk has
    // them as parameters, a card body and a bytecode chunk only as globals
    // (which none of them is).
    let mut extra: Vec<&str> = exports
        .iter()
        .map(String::as_str)
        .filter(|export| !element.contains(export))
        .collect();
    extra.sort_unstable();
    assert_eq!(
        extra,
        [
            "CSSStyleValue",
            "Document",
            "__BobcatAfterExposureRecords",
            "__BobcatComputedStyleMap",
            "__BobcatDispatchEvent",
            "__BobcatHandleOf",
            "__BobcatQueryNodes",
            "__BobcatSwitchExposure",
            "__BobcatUsePageRoot",
        ],
        "chunk bindings the preamble does not import"
    );

    // The runtime half: the thirteen names `CHUNK_BINDINGS` lists after the
    // element exports, read from the source.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packages/bobcat-element/src/main-thread-runtime.ts"
    ))
    .unwrap();
    let start = source.find("const CHUNK_BINDINGS").unwrap();
    let end = start + source[start..].find("];").unwrap();
    let runtime_names: Vec<&str> = source[start..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("[\""))
        .filter_map(|line| line.split('"').next())
        .collect();
    assert_eq!(runtime_names, imports("bobcat:runtime"));
}
