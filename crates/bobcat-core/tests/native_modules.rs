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
    DrawTarget, EngineError, EngineEvent, LynxGroup, LynxView, LynxViewError, ModuleCall,
    ModuleCallback, NativeModule, NoWakeup, Painter, StyleThreads, ViewSources,
};

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const MAIN_URL: &str = "app:///main.js";
const BACKGROUND_URL: &str = "app:///background.js";
const UNDECLARED_URL: &str = "app:///undeclared.js";

/// A minimal main-thread entry: a card with one element, so boot finishes.
const MAIN_ENTRY: &str = r"
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// Serves the entries these tests have out of memory.
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
            UNDECLARED_URL => Some(UNDECLARED_ENTRY),
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
            | SourceRequest::Font { url }
            | SourceRequest::Fetch { url } => url.clone(),
            SourceRequest::Worker { specifier, .. } => specifier.clone(),
        };
        completion.complete(Self::source(&specifier).map_or_else(
            || {
                Err(ResourceError {
                    kind: ResourceErrorKind::NotFound,
                    phase: ResourceErrorPhase::Resolve,
                    locator: Some(Arc::from(specifier.as_str())),
                    message: "this host serves three entries".into(),
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

/// A background entry that first calls a method `Echo` never declared. Only
/// the host member reaches it, because `NativeModules.Echo.nope` is
/// `undefined`; the callback prints if anything ever answers it. The declared
/// call after it is what the test waits for, and its callback also prints
/// whether the realm still holds the undeclared call's function.
///
/// That function is an arrow function created inside a function that has
/// returned, so the realm's table of outstanding calls holds its only strong
/// reference. An arrow function has no `prototype` object pointing back at
/// it, so dropping that reference frees it at once, and `QuickJS` answers a
/// `WeakRef` from the reference count: `deref()` is `undefined` from then on,
/// with no collection needed.
const UNDECLARED_ENTRY: &str = r"
import { console, lynx } from 'bobcat:bts-runtime';
import { callNativeModule } from 'bobcat:worker';
let undeclared;
(() => {
  const callback = () => console.log('the undeclared call was answered');
  undeclared = new WeakRef(callback);
  callNativeModule('Echo', 'nope', [callback]);
})();
lynx.getApp().NativeModules.Echo.echo({ note: 'hello' }, function (method, echoed) {
  console.log('echoed ' + method + ' ' + JSON.stringify(echoed)
    + ', released ' + (undeclared.deref() === undefined));
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
    /// Which method each call named, for the assertion that only a declared
    /// one arrives.
    named: RefCell<Vec<String>>,
}

impl Echo {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: RefCell::default(),
            named: RefCell::default(),
        }
    }
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
        self.named.borrow_mut().push(call.method.clone());
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
struct Shared<M>(Rc<M>);

impl<M: NativeModule> NativeModule for Shared<M> {
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

/// A module that reads the painter of its own view inside `invoke` — an
/// embedder whose module drives its display synchronously — and then answers
/// the way [`Echo`] does.
struct Driver {
    echo: Echo,
    /// The painter attached to the view this module serves, `None` until the
    /// test has attached it.
    painter: Rc<RefCell<Option<Painter>>>,
    /// What the painter said, once per call.
    animating: RefCell<Vec<bool>>,
}

impl NativeModule for Driver {
    fn name(&self) -> &str {
        self.echo.name()
    }

    fn methods(&self) -> Vec<String> {
        self.echo.methods()
    }

    fn invoke(&self, call: ModuleCall) {
        let painter = self.painter.borrow();
        let painter = painter
            .as_ref()
            .expect("the painter is attached before the view is pumped");
        // Reads the view's frame demand, the cell `pump` also borrows.
        self.animating.borrow_mut().push(painter.is_animating());
        self.echo.invoke(call);
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

fn sources(background: &str) -> ViewSources {
    let mut sources = ViewSources::new(MAIN_URL, SCREEN);
    sources.background_entry = Some(background.to_owned());
    sources
}

/// Pumps `view` until the background realm prints, and answers with what it
/// printed first. Every event of the batch that message arrived in is still
/// read, so a failure of a realm in that batch or an earlier one fails the
/// test.
fn first_console_message(view: &mut LynxView<Entries>) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut first = None;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage { message, .. } => {
                    first.get_or_insert(message);
                }
                EngineEvent::StartupFailed(error) => panic!("boot failed: {error}"),
                EngineEvent::WorkerFailed(error) | EngineEvent::ScriptRunError(error) => {
                    panic!("the realm failed: {}", error.message)
                }
                _ => {}
            }
        }
        if let Some(message) = first {
            return message;
        }
        assert!(
            Instant::now() < deadline,
            "the module's callback reached the background realm"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[tokio::test]
async fn an_injected_module_answers_a_background_call_on_the_embedders_own_thread() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let echo = Rc::new(Echo::new("Echo"));
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Entries,
            vec![Box::new(Shared(Rc::clone(&echo))) as Box<dyn NativeModule>],
            sources(BACKGROUND_URL),
        )
        .expect("the view is built");
    // The BTS `console` this waits for is forwarded through MTS, whose boot
    // flush waits for a painter to bind the view.
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    assert_eq!(
        first_console_message(&mut view),
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
            sources(BACKGROUND_URL),
        )
        .map(|_: LynxView<Entries>| ())
        .expect_err("one name, one module");
    assert!(
        matches!(
            &refused,
            LynxViewError::Engine(EngineError::DuplicateNativeModule(name)) if name == "Echo"
        ),
        "unexpected refusal: {refused}"
    );
}

/// A call naming a method its module never declared is dropped by `pump`
/// rather than handed to the module, and dropping it releases its function in
/// the realm uninvoked. The release reaches the realm ahead of the declared
/// call's answer, so the first thing printed is that answer, and by then the
/// realm no longer holds the released function.
#[tokio::test]
async fn an_undeclared_method_never_reaches_its_module() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let echo = Rc::new(Echo::new("Echo"));
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Entries,
            vec![Box::new(Shared(Rc::clone(&echo))) as Box<dyn NativeModule>],
            sources(UNDECLARED_URL),
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    assert_eq!(
        first_console_message(&mut view),
        r#"echoed echo [{"note":"hello"},null], released true"#,
        "the undeclared call's function was released, not answered"
    );
    assert_eq!(
        echo.named.borrow().as_slice(),
        ["echo"],
        "only the declared method reached the module"
    );
}

/// A module may read the painter attached to its own view while it serves a
/// call: `pump` holds no borrow of the view's frame demand across `invoke`,
/// and the painter borrows that same cell.
#[tokio::test]
async fn a_module_may_drive_its_painter_inside_invoke() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let painter = Rc::new(RefCell::new(None));
    let driver = Rc::new(Driver {
        echo: Echo::new("Echo"),
        painter: Rc::clone(&painter),
        animating: RefCell::default(),
    });
    let mut view = group
        .create_lynx_view(
            32.0,
            24.0,
            1.0,
            |_reports| Entries,
            vec![Box::new(Shared(Rc::clone(&driver))) as Box<dyn NativeModule>],
            sources(BACKGROUND_URL),
        )
        .expect("the view is built");
    let mut attached = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    attached
        .attach(&view)
        .expect("a fresh view takes a painter");
    *painter.borrow_mut() = Some(attached);

    assert_eq!(
        first_console_message(&mut view),
        r#"echoed echo [{"note":"hello"},null]"#,
        "the module answered after reading its painter"
    );
    assert_eq!(
        driver.animating.borrow().len(),
        1,
        "the module read its painter inside the one call"
    );
}

/// Nothing here reads a callback, but the type has to be nameable by an
/// embedder that stores one.
const _: fn(ModuleCallback) -> usize = |callback| callback.argument_index();
