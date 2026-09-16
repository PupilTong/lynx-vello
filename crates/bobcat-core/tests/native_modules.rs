//! The public `NativeModule` seam: an embedder's module, injected at view
//! construction, answering a call the BTS realm made — over a real group, a
//! real Worker and a real fetcher.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest,
};
use bobcat_core::{
    EngineError, EngineEvent, LynxGroup, LynxViewError, ModuleCall, ModuleCallback, NativeModule,
    NoWakeup, StyleThreads, ViewSources,
};

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";

/// A minimal main-thread entry: a card with one element, so boot finishes.
const MAIN_ENTRY: &str = r"
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// Serves the two entries this test has out of memory.
struct Entries;

impl bobcat_core::FrameImages for Entries {
    fn read(
        &self,
        _source: &str,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::vello::peniko::ImageData> {
        None
    }

    fn retain(&self, _frame: &[Arc<str>]) {}
}

impl Entries {
    fn source(specifier: &str) -> Option<&'static str> {
        match specifier {
            MAIN_URL => Some(MAIN_ENTRY),
            BACKGROUND_URL => Some(BACKGROUND_ENTRY),
            _ => None,
        }
    }
}

impl ResourceFetcher for Entries {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        // Every request this host can be asked is named by one string, and it
        // is the same string in every case: this page has no stylesheets and
        // no fonts, so a specifier it does not know is refused below.
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
                    message: "this host serves two entries".into(),
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

/// The background entry: one call, whose callback prints what came back.
///
/// It imports its own bindings, as a fetched BTS entry does — only the
/// built-in bootstrap carries a preamble.
const BACKGROUND_ENTRY: &str = r"
import { console, lynx } from 'bobcat:bts-runtime';
const modules = lynx.getApp().NativeModules;
if (modules.Absent !== undefined) throw Error('an absent module is undefined');
if (modules.Echo.nope !== undefined) throw Error('an undeclared method is undefined');
modules.Echo.echo({ note: 'hello' }, function (method, echoed) {
  console.log('echoed ' + method + ' ' + JSON.stringify(echoed));
});
";

/// An embedder's module: it hands the call's own arguments back through the
/// callback, on the thread `pump` gave it.
struct Echo {
    /// The `NativeModules` key, held rather than spelled in `name` so the
    /// answer is this module's own rather than a `'static` literal.
    name: &'static str,
    /// Which thread each call arrived on, for the assertion that it is the
    /// embedder's own.
    calls: RefCell<Vec<std::thread::ThreadId>>,
}

impl NativeModule for Echo {
    fn name(&self) -> &str {
        self.name
    }

    fn methods(&self) -> Vec<String> {
        vec!["echo".to_owned()]
    }

    fn invoke(&self, call: ModuleCall) {
        self.calls.borrow_mut().push(std::thread::current().id());
        let ModuleCall {
            method,
            arguments,
            callbacks,
        } = call;
        let Some(callback) = callbacks.into_iter().next() else {
            return;
        };
        assert_eq!(callback.argument_index(), 1);
        // The method name, then whatever the realm sent — the argument list
        // is JSON array text, so it is spliced rather than parsed.
        callback.invoke(format!(
            "[{},{arguments}]",
            serde_json::to_string(&method).expect("a string is JSON")
        ));
    }
}

/// The handle the test keeps while the view holds its own: a module is the
/// embedder's, and an embedder that wants to read what its module did keeps a
/// share of it.
struct Shared(Rc<Echo>);

impl NativeModule for Shared {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn methods(&self) -> Vec<String> {
        self.0.methods()
    }

    fn invoke(&self, call: ModuleCall) {
        self.0.invoke(call);
    }
}

/// A module that declares a name somebody else already has.
struct Twin(&'static str);

impl NativeModule for Twin {
    fn name(&self) -> &str {
        self.0
    }

    fn methods(&self) -> Vec<String> {
        Vec::new()
    }

    fn invoke(&self, _call: ModuleCall) {
        unreachable!("a duplicate name never builds a view");
    }
}

fn sources() -> ViewSources {
    let mut sources = ViewSources::new(MAIN_URL);
    sources.background_entry = Some(BACKGROUND_URL.to_owned());
    sources
}

#[tokio::test]
async fn an_injected_module_answers_a_background_call_on_the_embedders_own_thread() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let echo = Rc::new(Echo {
        name: "Echo",
        calls: RefCell::default(),
    });
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Entries,
            vec![Box::new(Shared(Rc::clone(&echo))) as Box<dyn NativeModule>],
            sources(),
        )
        .expect("the view is built");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut echoed = None;
    while echoed.is_none() {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } => echoed = Some(message),
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::WorkerFailed(error) | EngineEvent::ScriptRunError(error) => {
                    panic!("the realm failed: {}", error.message)
                }
                _ => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "the module's callback reached the background realm"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        echoed.expect("the callback printed"),
        r#"echoed echo [{"note":"hello"},null]"#,
        "the function argument is null in the JSON and the callback carries it"
    );
    assert_eq!(
        echo.calls.borrow().as_slice(),
        [std::thread::current().id()],
        "a module is invoked on the thread that pumps its view"
    );
}

#[tokio::test]
async fn two_modules_of_one_name_refuse_to_build_a_view() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let refused = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Entries,
            vec![Box::new(Twin("Echo")), Box::new(Twin("Echo"))],
            sources(),
        )
        .map(|_: bobcat_core::LynxView<Entries>| ())
        .expect_err("one name, one module");
    assert!(
        matches!(
            &refused,
            LynxViewError::Engine(EngineError::DuplicateNativeModule(name)) if name == "Echo"
        ),
        "unexpected refusal: {refused}"
    );
}

/// Nothing here reads a callback, but the type has to be nameable by an
/// embedder that stores one.
const _: fn(ModuleCallback) -> usize = |callback| callback.argument_index();
