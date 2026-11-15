//! The public `NativeModule` seam: an embedder's module, injected at view
//! construction, answering a call the BTS realm or the MTS realm made — over
//! a real group, a real Worker and a real fetcher.

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
    ModuleCallback, NativeModule, NativeModuleEsm, NoWakeup, Painter, ScriptSource, StyleThreads,
    ViewSources, compile_native_module_esm,
};

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

const MAIN_URL: &str = "app:///main.js";
const MAIN_CALLER_URL: &str = "app:///main-caller.js";
const BACKGROUND_URL: &str = "app:///background.js";
const UNDECLARED_URL: &str = "app:///undeclared.js";
const ON_EVENT_URL: &str = "app:///on-event.js";
const ESM_CALLER_URL: &str = "app:///esm-caller.js";
const ORDER_URL: &str = "app:///order.js";
const UNKNOWN_ESM_URL: &str = "app:///unknown-esm.js";
const UNREADABLE_ESM_URL: &str = "app:///unreadable-esm.js";
const MAIN_LYNX_URL: &str = "app:///main-lynx.js";

thread_local! {
    /// Every source the fetcher of this test's thread was asked for, in
    /// order: a test's views are pumped on its own thread, which is the
    /// only thread a fetcher is called on.
    static REQUESTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Whether the fetcher of this test's thread was ever asked for a `lynx:`
/// name.
fn requested_a_native_esm() -> bool {
    REQUESTED.with_borrow(|requested| requested.iter().any(|url| url.starts_with("lynx:")))
}

/// A minimal main-thread entry: a card with one element, so boot finishes.
const MAIN_ENTRY: &str = r"
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// A main-thread entry that calls `Echo` through the transport itself, as
/// its top level runs: the MTS realm's `NativeModules` is `undefined`, since
/// the view's table is only posted to the BTS, but `bobcat:native-modules`
/// links there and the view answers the call back to this realm. The card is
/// the one [`MAIN_ENTRY`] renders.
const MAIN_CALLER_ENTRY: &str = r"
import { callNativeModule } from 'bobcat:native-modules';
if (NativeModules !== undefined) throw Error('the MTS NativeModules is undefined');
callNativeModule('Echo', 'echo', [{ note: 'main' }, function (method, echoed) {
  console.log('echoed ' + method + ' ' + JSON.stringify(echoed));
}]);
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// Serves the entries these tests have out of memory.
struct Entries;

impl bobcat_core::FrameImages for Entries {
    type PlatformFrame = std::convert::Infallible;

    fn describe(
        &self,
        _picture: &bobcat_core::PictureKey,
        _hint: bobcat_core::ImageSizeHint,
    ) -> Option<bobcat_core::FrameImage> {
        None
    }

    /// Nothing is ever answered, so nothing is ever wanted.
    fn want(&self, _picture: &bobcat_core::PictureKey, _hint: bobcat_core::ImageSizeHint) {}

    fn retain(&self, _frame: &[bobcat_core::PictureKey]) {}
}

impl Entries {
    /// A main-thread entry is a card's MTS body, served as `bobcat-source`
    /// registers a card's root; a background entry is served as it stands.
    fn source(specifier: &str) -> Option<String> {
        let card = |body: &str| format!("{}{body}", bobcat_core::MTS_CHUNK_PREAMBLE);
        match specifier {
            MAIN_URL => Some(card(MAIN_ENTRY)),
            MAIN_CALLER_URL => Some(card(MAIN_CALLER_ENTRY)),
            BACKGROUND_URL => Some(BACKGROUND_ENTRY.to_owned()),
            UNDECLARED_URL => Some(UNDECLARED_ENTRY.to_owned()),
            ON_EVENT_URL => Some(ON_EVENT_ENTRY.to_owned()),
            ESM_CALLER_URL => Some(ESM_CALLER_ENTRY.to_owned()),
            ORDER_URL => Some(ORDER_ENTRY.to_owned()),
            UNKNOWN_ESM_URL => Some(UNKNOWN_ESM_ENTRY.to_owned()),
            UNREADABLE_ESM_URL => Some(UNREADABLE_ESM_ENTRY.to_owned()),
            MAIN_LYNX_URL => Some(card(MAIN_LYNX_ENTRY)),
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
            SourceRequest::Module(url)
            | SourceRequest::StyleSheet(url)
            | SourceRequest::Font { url }
            | SourceRequest::Fetch { url, .. } => url.clone(),
        };
        REQUESTED.with_borrow_mut(|requested| requested.push(specifier.clone()));
        completion.complete(Self::source(&specifier).map_or_else(
            || {
                Err(ResourceError {
                    kind: ResourceErrorKind::NotFound,
                    phase: ResourceErrorPhase::Resolve,
                    locator: Some(Arc::from(specifier.as_str())),
                    message: "this host serves five entries".into(),
                    retry: RetryAdvice::Never,
                }
                .into())
            },
            |source| {
                Ok(LoadedSource::Module {
                    source,
                    url: specifier.clone(),
                })
            },
        ));
    }
}

/// The background entry: one call, whose callback prints what came back.
///
/// It imports its own bindings, as a BTS entry that is not a card's body
/// does: `BTS_CHUNK_PREAMBLE` is what `bobcat-source` prepends to a card's
/// bodies, and the engine adds nothing to an entry.
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
import { callNativeModule } from 'bobcat:native-modules';
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

/// A background entry that makes its call only when the host sends it the
/// `call` global event, so the call comes after whatever the host did first.
const ON_EVENT_ENTRY: &str = r"
import { console, lynx } from 'bobcat:bts-runtime';
lynx.getJSModule('GlobalEventEmitter').addListener('call', () => {
  lynx.getApp().NativeModules.Echo.echo({ note: 'after' }, function (method, echoed) {
    console.log('echoed ' + method + ' ' + JSON.stringify(echoed));
  });
});
";

/// The ES module the `Echo` module supplies in the tests below: a wrapper
/// over its own `echo` method, written over `bobcat:native-modules`, and the
/// URL it knows itself by.
const ECHO_ESM: &str = r"
import { callNativeModule } from 'bobcat:native-modules';
export const url = import.meta.url;
export function echo(value, callback) {
  callNativeModule('Echo', 'echo', [value, callback]);
}
";

/// A background entry that imports `Echo`'s ES module statically and calls
/// the module through it.
const ESM_CALLER_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
import { echo, url } from 'lynx:Echo';
echo({ note: url }, function (method, echoed) {
  console.log('echoed ' + method + ' ' + JSON.stringify(echoed));
});
";

/// A background entry that prints, as its top level runs, which native
/// module ES modules have run so far.
const ORDER_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
console.log('order ' + JSON.stringify(globalThis.ran ?? []));
";

/// A native module ES module that records it ran, and what `NativeModules`
/// named when it did. `{name}` is the module's name.
fn recording_esm(name: &str) -> String {
    format!(
        r"
import {{ lynx }} from 'bobcat:bts-runtime';
globalThis.ran = (globalThis.ran ?? []).concat(
  '{name}:' + Object.keys(lynx.getApp().NativeModules).join('+'));
"
    )
}

/// A background entry that loads a `lynx:` name the view has no ES module
/// for, both ways a realm can: `require` and `import()`.
const UNKNOWN_ESM_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
import { createRequire } from 'bobcat:module';
let required;
try {
  createRequire(import.meta.url)('lynx:Missing');
  required = 'loaded';
} catch (error) {
  required = error.message;
}
import('lynx:Missing').then(
  () => console.log('loaded'),
  error => console.log('import: ' + error.message + ' | require: ' + required),
);
";

/// A background entry that imports `Echo`'s ES module and prints how that
/// went.
const UNREADABLE_ESM_ENTRY: &str = r"
import { console } from 'bobcat:bts-runtime';
import('lynx:Echo').then(
  () => console.log('loaded'),
  error => console.log(error.name + ': ' + error.message),
);
";

/// A main-thread entry that imports `Echo`'s ES module, which only a worker
/// realm loads, and prints how that went; the card is [`MAIN_ENTRY`]'s.
const MAIN_LYNX_ENTRY: &str = r"
import('lynx:Echo').then(
  () => console.log('loaded'),
  error => console.log(error.name + ': ' + error.message),
);
globalThis.renderPage = function () {
  __AppendElement(__CreatePage('card', 0), __CreateView(0));
};
";

/// A module that supplies an ES module beside [`Echo`]'s method: what
/// [`NativeModule::esm`] answers is fixed when it is built.
struct WithEsm {
    echo: Echo,
    bytecode: bytes::Bytes,
    import_before_entry: bool,
}

impl WithEsm {
    /// `name`'s module, its ES module compiled from `source`.
    fn new(name: &'static str, source: &str, import_before_entry: bool) -> Self {
        Self::with_bytecode(
            name,
            compile_native_module_esm(name, source).expect("the ES module compiles"),
            import_before_entry,
        )
    }

    fn with_bytecode(name: &'static str, bytecode: Vec<u8>, import_before_entry: bool) -> Self {
        Self {
            echo: Echo::new(name),
            bytecode: bytecode.into(),
            import_before_entry,
        }
    }
}

impl NativeModule for WithEsm {
    fn name(&self) -> &str {
        self.echo.name()
    }

    fn methods(&self) -> Vec<String> {
        self.echo.methods()
    }

    fn invoke(&self, call: ModuleCall) {
        self.echo.invoke(call);
    }

    fn esm(&self) -> Option<NativeModuleEsm> {
        Some(NativeModuleEsm {
            bytecode: self.bytecode.clone(),
            import_before_entry: self.import_before_entry,
        })
    }
}

/// Builds a view of `modules` whose BTS entry is `background`, and binds a
/// painter to it, which boot's flush waits for.
async fn esm_view(
    group: &LynxGroup,
    modules: Vec<Box<dyn NativeModule>>,
    sources: ViewSources,
) -> (LynxView<Entries>, Painter) {
    let view = group
        .create_lynx_view(32.0, 24.0, 1.0, |_reports| Entries, modules, sources)
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");
    (view, painter)
}

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

    fn esm(&self) -> Option<NativeModuleEsm> {
        self.0.esm()
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
    let mut sources = ViewSources::new("app:///", MAIN_URL, SCREEN, "en-US");
    sources.background_entry = Some(background.to_owned());
    sources
}

/// Pumps `view` until the background realm prints, and answers with what it
/// printed first.
///
/// The background realm is the worker whose URL is `bobcat:bts`, which is the
/// comparison an embedder makes. Its id is 1 here: each test's group has one
/// view, whose entry constructs no worker, so boot's BTS is the group's first.
fn first_console_message(view: &mut LynxView<Entries>) -> String {
    let (source, message) = first_console_output(view);
    let ScriptSource::Worker { id, url } = &source else {
        panic!("{source} printed first: {message}");
    };
    assert_eq!(&**url, "bobcat:bts", "{message}");
    assert_eq!(id.to_string(), "1", "{message}");
    message
}

/// Pumps `view` until a realm prints, and answers with which realm printed
/// first and what. Every event of the batch that message arrived in is still
/// read, so a failure of a realm in that batch or an earlier one fails the
/// test.
fn first_console_output(view: &mut LynxView<Entries>) -> (ScriptSource, String) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut first = None;
    loop {
        for event in view.pump() {
            match event {
                EngineEvent::ConsoleMessage {
                    source, message, ..
                } => {
                    first.get_or_insert((source, message));
                }
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
        if let Some(message) = first {
            return message;
        }
        assert!(
            Instant::now() < deadline,
            "the module's callback reached the calling realm"
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

/// A call the MTS realm makes through `bobcat:native-modules` reaches the
/// embedder's module like a BTS call does, and its answer goes back to the
/// MTS realm: `pump` answers a call with no worker through the view's own
/// command FIFO, and the callback runs in the realm that made the call.
#[tokio::test]
async fn a_main_thread_call_is_answered_back_to_the_main_thread() {
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
            ViewSources::new("app:///", MAIN_CALLER_URL, SCREEN, "en-US"),
        )
        .expect("the view is built");
    // The answer is applied by a job of the view, which runs once boot's
    // flush has found a painter bound.
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    assert_eq!(
        first_console_output(&mut view),
        (
            ScriptSource::Main,
            r#"echoed echo [{"note":"main"},null]"#.to_owned()
        ),
        "the MTS realm's own callback printed the answer"
    );
    assert_eq!(echo.named.borrow().as_slice(), ["echo"]);
}

/// A script failure after boot leaves the view running, and its native
/// modules with it: a page update the main-thread realm fails is reported as a
/// `ScriptRunError`, and a background call made after it is still answered.
///
/// A failure that ended the view would have cleared the table `pump` answers
/// a call through, so the callback would never have arrived.
#[tokio::test]
async fn a_script_failure_after_boot_leaves_native_module_calls_answered() {
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
            sources(ON_EVENT_URL),
        )
        .expect("the view is built");
    let mut painter = Painter::new(DrawTarget::Offscreen, 32.0, 24.0, 1.0)
        .await
        .expect("the painter is built");
    painter.attach(&view).expect("a fresh view takes a painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut failed = false;
    while !failed {
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => view
                    .update_data("not JSON".to_owned(), String::new())
                    .expect("a booted view accepts a data update"),
                EngineEvent::ScriptRunError(_) => failed = true,
                event if event.is_fatal() => panic!("the view failed: {event:?}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the update never failed");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(view.is_ready(), "the failure did not end the view");

    view.send_global_event("call", "[]".to_owned())
        .expect("the view still takes page updates");
    assert_eq!(
        first_console_message(&mut view),
        r#"echoed echo [{"note":"after"},null]"#,
        "the call made after the failure was answered"
    );
    assert_eq!(echo.named.borrow().as_slice(), ["echo"]);
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

/// A module's ES module is what a BTS entry's `import` of `lynx:<name>`
/// loads: its `import.meta.url` is that specifier, its own import of
/// `bobcat:native-modules` links, and a call through it reaches the module's
/// `invoke`. The fetcher is never asked for it.
#[tokio::test]
async fn a_background_entry_calls_its_module_through_the_modules_es_module() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let module = Rc::new(WithEsm::new("Echo", ECHO_ESM, false));
    let (mut view, _painter) = esm_view(
        &group,
        vec![Box::new(Shared(Rc::clone(&module))) as Box<dyn NativeModule>],
        sources(ESM_CALLER_URL),
    )
    .await;
    assert_eq!(
        first_console_message(&mut view),
        r#"echoed echo [{"note":"lynx:Echo"},null]"#,
        "the module knows itself as `lynx:Echo` and its call was answered"
    );
    assert_eq!(module.echo.named.borrow().as_slice(), ["echo"]);
    assert!(!requested_a_native_esm(), "a `lynx:` name is never fetched");
}

/// The modules flagged `import_before_entry` run before the BTS entry's top
/// level, one after another in the order the view was given them, each after
/// `initialize` has supplied `NativeModules`; a module not flagged does not
/// run when nothing imports it.
#[tokio::test]
async fn flagged_es_modules_run_in_order_before_the_entry_and_others_do_not_run() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let (mut view, _painter) = esm_view(
        &group,
        vec![
            Box::new(WithEsm::new("Second", &recording_esm("Second"), true)),
            Box::new(WithEsm::new("Lazy", &recording_esm("Lazy"), false)),
            Box::new(WithEsm::new("First", &recording_esm("First"), true)),
        ],
        sources(ORDER_URL),
    )
    .await;
    assert_eq!(
        first_console_message(&mut view),
        r#"order ["Second:Second+Lazy+First","First:Second+Lazy+First"]"#,
        "the flagged modules ran in the given order, before the entry, and `Lazy` did not"
    );
    assert!(!requested_a_native_esm());
}

/// A flagged module that throws is the entry's failure: reported as the
/// BTS's uncaught exception, and the entry is not imported.
#[tokio::test]
async fn a_flagged_es_module_that_throws_fails_the_entry() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let (mut view, _painter) = esm_view(
        &group,
        vec![Box::new(WithEsm::new(
            "Broken",
            "throw Error('the native module ES module failed');",
            true,
        ))],
        sources(ORDER_URL),
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    let error = 'reported: loop {
        for event in view.pump() {
            match event {
                EngineEvent::WorkerThrew { source, error } => {
                    let ScriptSource::Worker { url, .. } = &source else {
                        panic!("{source} threw: {}", error.message);
                    };
                    assert_eq!(&**url, "bobcat:bts");
                    break 'reported error;
                }
                EngineEvent::ConsoleMessage { message, .. } => {
                    panic!("the entry ran: {message}")
                }
                event if event.is_fatal() => panic!("the view failed: {event:?}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the failure was never reported");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(
        error.message.contains("the native module ES module failed"),
        "{error:?}"
    );
    // A turn later, the entry has still printed nothing.
    std::thread::sleep(Duration::from_millis(50));
    for event in view.pump() {
        assert!(
            !matches!(event, EngineEvent::ConsoleMessage { .. }),
            "the entry ran after its flagged module failed: {event:?}"
        );
    }
}

/// A `lynx:` name the view has no ES module for fails in the BTS realm,
/// naming the specifier — an `import` and a `require` alike — and is never
/// fetched.
#[tokio::test]
async fn an_unknown_native_es_module_fails_without_a_request() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let (mut view, _painter) = esm_view(
        &group,
        vec![Box::new(WithEsm::new("Echo", ECHO_ESM, false))],
        sources(UNKNOWN_ESM_URL),
    )
    .await;
    let message = first_console_message(&mut view);
    let (import, require) = message
        .strip_prefix("import: ")
        .and_then(|rest| rest.split_once(" | require: "))
        .unwrap_or_else(|| panic!("{message}"));
    assert!(
        import.contains("lynx:Missing") && import.contains("no native module `Missing`"),
        "{import}"
    );
    assert!(
        require.contains("lynx:Missing") && require.contains("cannot be loaded synchronously"),
        "{require}"
    );
    assert!(!requested_a_native_esm(), "{:?}", REQUESTED.take());
}

/// The MTS realm does not load a native module's ES module: its import fails
/// with a `ReferenceError` naming the specifier, and nothing is fetched.
#[tokio::test]
async fn the_main_thread_does_not_load_a_native_es_module() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let (mut view, _painter) = esm_view(
        &group,
        vec![Box::new(WithEsm::new("Echo", ECHO_ESM, false))],
        ViewSources::new("app:///", MAIN_LYNX_URL, SCREEN, "en-US"),
    )
    .await;
    let (source, message) = first_console_output(&mut view);
    assert_eq!(source, ScriptSource::Main, "{message}");
    assert!(
        message.starts_with("ReferenceError: ") && message.contains("lynx:Echo"),
        "{message}"
    );
    assert!(!requested_a_native_esm());
}

/// Bytecode whose frame is not this build's — another build's id, or cut
/// short — refuses the view where it is built, naming the module.
#[tokio::test]
async fn an_es_module_outside_this_builds_frame_refuses_the_view() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let good = compile_native_module_esm("Echo", ECHO_ESM).expect("the ES module compiles");
    let mut foreign = good.clone();
    foreign[0] ^= 1;
    let truncated = good[..good.len() - 1].to_vec();
    for (case, bytecode) in [("another build", foreign), ("truncated", truncated)] {
        let refused = group
            .create_lynx_view(
                32.0,
                24.0,
                1.0,
                |_reports| Entries,
                vec![Box::new(WithEsm::with_bytecode("Echo", bytecode, false))],
                sources(ESM_CALLER_URL),
            )
            .map(|_: LynxView<Entries>| ())
            .expect_err(case);
        assert!(
            matches!(
                &refused,
                LynxViewError::Engine(EngineError::InvalidNativeModuleEsm(name)) if name == "Echo"
            ),
            "{case}: unexpected refusal: {refused}"
        );
    }
    assert!(
        REQUESTED.with_borrow(Vec::is_empty),
        "a refused view requests nothing"
    );
}

/// Bytecode in a whole frame that cannot be read as the module fails its
/// import in the importing realm with a `TypeError` naming it, and nothing
/// runs in its place.
#[tokio::test]
async fn an_unreadable_es_module_fails_its_import() {
    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("the group starts");
    let mut garbage = compile_native_module_esm("Echo", ECHO_ESM).expect("the ES module compiles");
    // The frame — the engine id and the length, 16 bytes — stands; the
    // payload is not bytecode.
    garbage[16..].fill(b'x');
    let (mut view, _painter) = esm_view(
        &group,
        vec![Box::new(WithEsm::with_bytecode("Echo", garbage, false))],
        sources(UNREADABLE_ESM_URL),
    )
    .await;
    assert_eq!(
        first_console_message(&mut view),
        "TypeError: module 'lynx:Echo' cannot be read from the bytecode it was supplied as"
    );
    assert!(!requested_a_native_esm());
}
