//! `bobcat:module` end to end: a real group, a real view, a real BTS Worker
//! and a real fetcher, with both realms loading `CommonJS`, JSON and ES
//! modules synchronously while the entry that asked for them is still
//! evaluating.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest,
};
use bobcat_core::{
    DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads, ViewSources,
};

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";
const MAIN_MODULE_URL: &str = "app:///module-main.js";
const BACKGROUND_MODULE_URL: &str = "app:///module-background.js";

/// The card: two `require`s before the render hook exists, so the page cannot
/// be built at all unless both answered.
const MAIN_ENTRY: &str = r"
import { createRequire } from 'bobcat:module';
const require = createRequire(import.meta.url);
const { answer } = require('./lib/answer.cjs');
const config = require('./config.json');
console.log('main ' + answer + ' ' + config.name + ' ' + require.resolve('./config.json'));
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// The background entry, which imports its own bindings as a BTS entry that
/// is not a card's body does.
const BACKGROUND_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
import { createRequire } from 'bobcat:module';
const shared = createRequire(import.meta.url)('./bts/shared.cjs');
console.log('background ' + shared.tag + ' ' + shared.dir);
";

/// The card of the second test: every shape of ES module `require` has to
/// answer for, taken while the entry itself is still evaluating.
const MAIN_MODULE_ENTRY: &str = r"
import { createRequire } from 'bobcat:module';
const require = createRequire(import.meta.url);
const dep = require('./lib/dep.mjs');
const detected = require('./lib/detected.js');
const dual = require('./lib/dual.mjs');
const again = require('./lib/dep.mjs');
let refused = 'none';
try {
  require('./lib/awaits.mjs');
} catch (error) {
  refused = error.message.includes('top-level await') ? 'refused' : error.message;
}
console.log([
  'main',
  dep.answer,
  detected.tag,
  dual(),
  again === dep,
  refused,
].join(' '));
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// The same of a worker realm, whose entry is fetched as this one is.
const BACKGROUND_MODULE_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
import { createRequire } from 'bobcat:module';
const module = createRequire(import.meta.url)('./bts/module.mjs');
console.log(['background', module.tag, module.fromDep, module.url].join(' '));
";

/// Every source a page is made of, answered from memory.
struct Files {
    source: fn(&str) -> Option<&'static str>,
}

impl bobcat_core::FrameImages for Files {
    fn read(
        &self,
        _source: &str,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        None
    }

    fn retain(&self, _frame: &[Arc<str>]) {}
}

impl Files {
    fn wrappers(specifier: &str) -> Option<&'static str> {
        match specifier {
            MAIN_URL => Some(MAIN_ENTRY),
            BACKGROUND_URL => Some(BACKGROUND_ENTRY),
            "app:///lib/answer.cjs" => Some("exports.answer = require('./deep.cjs').answer + 1;"),
            "app:///lib/deep.cjs" => Some("exports.answer = 41;"),
            "app:///config.json" => Some(r#"{"name": "card"}"#),
            "app:///bts/shared.cjs" => Some("exports.tag = 'shared'; exports.dir = __dirname;"),
            _ => None,
        }
    }

    /// `.mjs` is a module by its extension; `detected.js` has neither
    /// extension nor a `package.json` to say, so its own text does.
    fn modules(specifier: &str) -> Option<&'static str> {
        match specifier {
            MAIN_MODULE_URL => Some(MAIN_MODULE_ENTRY),
            BACKGROUND_MODULE_URL => Some(BACKGROUND_MODULE_ENTRY),
            "app:///lib/dep.mjs" => {
                Some("import { base } from './base.mjs';\nexport const answer = base + 1;")
            }
            "app:///lib/base.mjs" => Some("export const base = 41;"),
            "app:///lib/detected.js" => Some("export const tag = 'detected';"),
            "app:///lib/dual.mjs" => {
                Some("const exported = () => 'dual';\nexport { exported as 'module.exports' };")
            }
            "app:///lib/awaits.mjs" => Some("await Promise.resolve();\nexport const late = true;"),
            "app:///bts/module.mjs" => Some(
                "import { dep } from './dep.mjs';\n\
                 export const tag = 'module';\n\
                 export const fromDep = dep;\n\
                 export const url = import.meta.url;",
            ),
            "app:///bts/dep.mjs" => Some("export const dep = 'dep';"),
            _ => None,
        }
    }
}

impl ResourceFetcher for Files {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let specifier = match &request {
            SourceRequest::Module(url)
            | SourceRequest::StyleSheet(url)
            | SourceRequest::Font { url }
            | SourceRequest::Fetch { url } => url.clone(),
        };
        completion.complete((self.source)(&specifier).map_or_else(
            || {
                Err(ResourceError {
                    kind: ResourceErrorKind::NotFound,
                    phase: ResourceErrorPhase::Resolve,
                    locator: Some(Arc::from(specifier.as_str())),
                    message: "this page has no such file".into(),
                    retry: RetryAdvice::Never,
                }
                .into())
            },
            |source| {
                // A main-thread entry is a card's MTS body, served as
                // `bobcat-source` registers a card's root; every other file
                // is served as it stands.
                let source = if [MAIN_URL, MAIN_MODULE_URL].contains(&specifier.as_str()) {
                    format!("{}{source}", bobcat_core::MTS_CHUNK_PREAMBLE)
                } else {
                    source.to_owned()
                };
                Ok(LoadedSource::Module {
                    source,
                    url: specifier.clone(),
                })
            },
        ));
    }
}

/// Boots one view and collects what its two realms logged.
async fn logs_of(
    main: &'static str,
    background: &'static str,
    source: fn(&str) -> Option<&'static str>,
) -> Vec<String> {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut sources = ViewSources::new("app:///", main, SCREEN);
    sources.background_entry = Some(background.to_owned());
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            move |_reports| Files { source },
            Vec::new(),
            sources,
        )
        .expect("the view is built");
    // Boot's first flush waits for a painter to bind the view, and this
    // collects what both realms logged past it.
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut logged = Vec::new();
    let mut booted = false;
    while !booted || logged.len() < 2 {
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
        assert!(Instant::now() < deadline, "both realms finished requiring");
        std::thread::sleep(Duration::from_millis(1));
    }
    logged.sort();
    logged
}

#[tokio::test]
async fn both_realms_require_commonjs_and_json_while_their_entry_evaluates() {
    assert_eq!(
        logs_of(MAIN_URL, BACKGROUND_URL, Files::wrappers).await,
        [
            "background shared app:///bts/",
            "main 42 card app:///config.json",
        ],
        "each realm required against its own entry URL"
    );
}

/// The same of ES modules: the extension or the text names the shape, an
/// import inside a required module is loaded inline against the response
/// URL, a `module.exports` export is what `require` answers, one URL is one
/// module however often it is required, and a graph that awaits at its top
/// level is refused rather than waited for.
#[tokio::test]
async fn both_realms_require_es_modules_while_their_entry_evaluates() {
    assert_eq!(
        logs_of(MAIN_MODULE_URL, BACKGROUND_MODULE_URL, Files::modules).await,
        [
            "background module dep app:///bts/module.mjs",
            "main 42 detected dual true refused",
        ],
        "each realm linked and evaluated its modules synchronously"
    );
}
