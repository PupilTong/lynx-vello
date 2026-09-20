//! `bobcat:module` end to end: a real group, a real view, a real BTS Worker
//! and a real fetcher, with both realms loading `CommonJS` and JSON
//! synchronously while the entry that asked for them is still evaluating.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest,
};
use bobcat_core::{EngineEvent, LynxGroup, NoWakeup, StyleThreads, ViewSources};

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";

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

/// The background entry, which imports its own bindings as a fetched BTS
/// entry does.
const BACKGROUND_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
import { createRequire } from 'bobcat:module';
const shared = createRequire(import.meta.url)('./bts/shared.cjs');
console.log('background ' + shared.tag + ' ' + shared.dir);
";

/// Every source this page is made of, answered from memory.
struct Files;

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
    fn source(specifier: &str) -> Option<&'static str> {
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
}

impl ResourceFetcher for Files {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let specifier = match &request {
            SourceRequest::Entry(url)
            | SourceRequest::Module(url)
            | SourceRequest::StyleSheet(url)
            | SourceRequest::Font { url } => url.clone(),
            SourceRequest::Worker { specifier, .. } => specifier.clone(),
        };
        completion.complete(Self::source(&specifier).map_or_else(
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
                Ok(LoadedSource::Entry {
                    source: source.to_owned(),
                    url: specifier.clone(),
                })
            },
        ));
    }
}

#[tokio::test]
async fn both_realms_require_commonjs_and_json_while_their_entry_evaluates() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut sources = ViewSources::new(MAIN_URL);
    sources.background_entry = Some(BACKGROUND_URL.to_owned());
    let mut view = group
        .create_lynx_view(32.0, 24.0, 1.0, |_reports| Files, Vec::new(), sources)
        .expect("the view is built");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut logged = Vec::new();
    let mut booted = false;
    while !booted || logged.len() < 2 {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } => logged.push(message),
                EngineEvent::ScriptFinished => booted = true,
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::WorkerFailed(error) | EngineEvent::ScriptRunError(error) => {
                    panic!("the realm failed: {}", error.message)
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "both realms finished requiring");
        std::thread::sleep(Duration::from_millis(1));
    }
    logged.sort();
    assert_eq!(
        logged,
        [
            "background shared app:///bts/",
            "main 42 card app:///config.json",
        ],
        "each realm required against its own entry URL"
    );
}
