//! The in-crate test harness: a real group, a real view, and a host that
//! answers from memory.
//!
//! There is no lighter seam on purpose. Boot is one straight-line task now —
//! sources, then the realm, then the entry — so a harness that skipped it
//! would have to reimplement it, and would be testing its own copy. What this
//! replaces instead is the *host*: an [`InlineFetcher`] answers every source
//! request from a string it was given, and a painter with nowhere to draw
//! pays for no GPU device.

use std::future::Future;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::paint::Painter;
use crate::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest, StyleSheetProbe, StyleSheetSource,
};
use crate::style::PreparsedStyleSheet;
use crate::view::{
    DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, NoWakeup, StyleThreads,
    ViewSources,
};

/// The compiled `ReactLynx` cards, built by `pnpm --filter
/// reactlynx-test-fixtures build`: one module for every test in the crate
/// that boots one, since the registry embeds every bundle.
#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
pub(crate) mod fixtures;

/// How long a test waits on work that should already be happening.
///
/// A hang detector rather than a performance assertion: the first document a
/// test process builds pays for the platform's font registry, which is
/// seconds on some machines and nothing on others.
const PATIENCE: Duration = Duration::from_secs(30);

/// The URL every [`TestViewSpec`] view names its entry by, and the one
/// module URL an [`InlineFetcher`] answers with its entry script.
const ENTRY: &str = "app:///main.js";

/// The base URL every [`TestViewSpec`] view is built with, which its
/// [`InlineFetcher`] resolves a stylesheet or a plain fetch against.
const BASE: &str = "app:///";

/// The URL a [`TestViewSpec`] view given a BTS entry names it by, which its
/// [`InlineFetcher`] answers with that script.
const BACKGROUND_ENTRY: &str = "app:///background.js";

/// One author stylesheet, in whichever of the two forms a host has it, or
/// one the view lists and the host does not have.
pub(crate) enum TestSheet {
    Text(String),
    Preparsed(Arc<PreparsedStyleSheet>),
    /// Answered with the not-found error an unknown URL gets.
    Missing,
}

/// A host whose whole resource system is the strings it was built with.
pub(crate) struct InlineFetcher {
    /// The script served for [`ENTRY`]; every other module is missing. A
    /// card's MTS body as `bobcat-source` registers it, with
    /// `MTS_CHUNK_PREAMBLE` in front, since the engine adds nothing to it.
    entry: String,
    /// The script served for [`BACKGROUND_ENTRY`], when the view names one:
    /// a plain module, served as it was given.
    background: Option<String>,
    /// Every other module this host serves, by URL, each served as it was
    /// given: what a compiled card's BTS asks for by bundle path.
    modules: FxHashMap<String, String>,
    sheets: FxHashMap<String, TestSheet>,
    /// The faces this host serves an `@font-face` `src` from, by URL. A URL
    /// that is not in here fails, which is how a test drives the fall-through
    /// to a rule's next source.
    fonts: FxHashMap<String, dom::FontBlob>,
    /// The absolute URLs a [`SourceRequest::Fetch`] succeeds for; every
    /// other plain fetch fails.
    fetchable: FxHashSet<String>,
    /// The absolute URLs a plain fetch has succeeded for, which
    /// [`ResourceFetcher::fetch_probe`] answers from when this host has one.
    fetched: Option<Arc<Mutex<FxHashSet<String>>>>,
    /// Whether this host answers [`ResourceFetcher::style_sheet_probe`]: from
    /// the sheets it holds, by absolute URL. Without one, every
    /// `hasStyleSheet` is `true`.
    probes_style_sheets: bool,
}

/// `url` resolved against [`BASE`], as a fetcher resolves the stylesheets
/// and plain fetches it is handed; one that does not resolve stays as it is.
fn resolved(url: &str) -> String {
    url::Url::parse(BASE)
        .and_then(|base| base.join(url))
        .map_or_else(|_| url.to_owned(), String::from)
}

impl dom::FrameImages for InlineFetcher {
    fn read(
        &self,
        _source: &str,
        _hint: dom::ImageSizeHint,
    ) -> Option<dom::vello::peniko::ImageData> {
        None
    }

    /// Nothing is held, so there is no working set to narrow.
    fn retain(&self, _frame: &[Arc<str>]) {}
}

impl ResourceFetcher for InlineFetcher {
    fn request_source(&self, request: SourceRequest, completion: SourceCompletion) {
        let answer = match request {
            SourceRequest::Module(url) if url == ENTRY => Ok(LoadedSource::Module {
                source: self.entry.clone(),
                url,
            }),
            SourceRequest::Module(url) if url == BACKGROUND_ENTRY => match &self.background {
                Some(source) => Ok(LoadedSource::Module {
                    source: source.clone(),
                    url,
                }),
                None => Err(missing(&url)),
            },
            SourceRequest::Module(url) if self.modules.contains_key(&url) => {
                Ok(LoadedSource::Module {
                    source: self.modules[&url].clone(),
                    url,
                })
            }
            SourceRequest::Fetch { url } if self.fetchable.contains(&resolved(&url)) => {
                if let Some(fetched) = &self.fetched {
                    fetched
                        .lock()
                        .expect("the fetched set is never poisoned")
                        .insert(resolved(&url));
                }
                Ok(LoadedSource::Fetched)
            }
            SourceRequest::StyleSheet(url) => match self.sheets.get(&resolved(&url)) {
                Some(TestSheet::Text(css)) => Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(
                    css.clone(),
                ))),
                Some(TestSheet::Preparsed(sheet)) => Ok(LoadedSource::StyleSheet(
                    StyleSheetSource::Preparsed(Arc::clone(sheet)),
                )),
                Some(TestSheet::Missing) | None => Err(missing(&url)),
            },
            SourceRequest::Font { url } => self.fonts.get(&url).map_or_else(
                || Err(missing(&url)),
                |blob| Ok(LoadedSource::Font(blob.clone())),
            ),
            // This double serves no module but the two entries and the ones it
            // was given, and no plain fetch but the ones it was told succeed.
            SourceRequest::Module(url) | SourceRequest::Fetch { url } => Err(missing(&url)),
        };
        completion.complete(answer);
    }

    fn fetch_probe(&self) -> Option<crate::resource::FetchProbe> {
        let fetched = Arc::clone(self.fetched.as_ref()?);
        Some(Arc::new(move |url: &str| {
            fetched
                .lock()
                .expect("the fetched set is never poisoned")
                .contains(&resolved(url))
        }))
    }

    fn style_sheet_probe(&self) -> Option<StyleSheetProbe> {
        if !self.probes_style_sheets {
            return None;
        }
        let held: FxHashSet<String> = self
            .sheets
            .iter()
            .filter(|(_, sheet)| !matches!(sheet, TestSheet::Missing))
            .map(|(url, _)| url.clone())
            .collect();
        Some(Arc::new(move |url: &str| held.contains(&resolved(url))))
    }
}

fn missing(locator: &str) -> crate::LynxViewError {
    ResourceError {
        kind: ResourceErrorKind::NotFound,
        phase: ResourceErrorPhase::Resolve,
        locator: Some(locator.into()),
        message: format!("the harness was given nothing for `{locator}`").into(),
        retry: RetryAdvice::Never,
    }
    .into()
}

/// A view the crate's own tests drive, over the host above.
pub(crate) type TestView = LynxView<Rc<InlineFetcher>>;

/// A view and the painter watching it, which is what most of this crate's
/// tests want: the two halves an embedder owns, driven as one.
///
/// The split itself — attaching, detaching, one painter across two views — is
/// asserted from outside, in `tests/painter.rs`, over the public API. What a
/// test in here is usually about is a document, and for that the pair is one
/// thing.
pub(crate) struct TestEngine {
    pub(crate) view: TestView,
    pub(crate) painter: Painter,
    /// What a turn [`wait_for_boot`] took reported after boot's
    /// `ScriptFinished`: the test's, not the wait's, so the next
    /// [`Self::pump`] hands it back first.
    after_boot: Vec<EngineEvent>,
}

impl TestEngine {
    /// One host turn: the painter's, then the view's — the order every
    /// embedder takes them in.
    pub(crate) fn pump(&mut self) -> Vec<EngineEvent> {
        self.painter
            .pump()
            .expect("an in-crate painter's target does not fail");
        let mut events = std::mem::take(&mut self.after_boot);
        events.extend(self.view.pump());
        events
    }

    pub(crate) fn tick(&mut self, force: bool) -> Result<bool, crate::view::EngineError> {
        self.painter.tick(force)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn capture(&mut self) -> Result<crate::view::Screenshot, crate::view::EngineError> {
        self.painter.capture()
    }

    pub(crate) fn dispatch_input(&mut self, event: dom::input::InputEvent) {
        self.painter.dispatch_input(event);
    }

    pub(crate) fn is_animating(&self) -> bool {
        self.painter.is_animating()
    }

    pub(crate) fn probe_document<T: Send + 'static>(
        &mut self,
        probe: impl FnOnce(&mut crate::main::tree::LynxDocument) -> T + Send + 'static,
    ) -> Option<T> {
        self.view.probe_document(probe)
    }

    pub(crate) fn published_frame(&mut self) -> Option<Arc<dom::CommittedFrame>> {
        self.view.published_frame()
    }
}

/// Where a test's painter draws, if it draws anywhere.
enum TestTarget {
    /// Nowhere at all, which is what a test about routing, events or timers
    /// wants: no GPU device is built for it.
    None,
    /// A texture of its own, for a test that reads pixels back.
    Offscreen,
}

/// One view to build: its entry, its author sheets in listed order, and
/// where — if anywhere — its painter draws.
pub(crate) struct TestViewSpec {
    entry: String,
    background: Option<String>,
    modules: Vec<(String, String)>,
    config: crate::main::tree::PageConfig,
    sheets: Vec<(String, TestSheet)>,
    /// Sheets the host holds and the view does not list: what an adoption
    /// asks for by URL.
    served_sheets: Vec<(String, TestSheet)>,
    fonts: Vec<(String, dom::FontBlob)>,
    /// A face the view registers at construction and its family, the page's
    /// default font.
    default_font: Option<(String, dom::FontBlob)>,
    fetchable: FxHashSet<String>,
    probes_fetches: bool,
    probes_style_sheets: bool,
    /// The names of the [`EchoModule`]s the view is built with.
    echo_modules: Vec<&'static str>,
    target: TestTarget,
    width: f32,
    height: f32,
}

/// A native module that answers every call through its first function
/// argument, with the call's own argument list: what a test needs to see
/// that a realm's call reached the embedder and its answer came back.
struct EchoModule(&'static str);

impl crate::native_module::NativeModule for EchoModule {
    fn name(&self) -> &str {
        self.0
    }

    fn methods(&self) -> Vec<String> {
        vec!["echo".to_owned()]
    }

    fn invoke(&self, call: crate::native_module::ModuleCall) {
        if let Some(callback) = call.callbacks.into_iter().next() {
            callback.invoke(call.arguments);
        }
    }
}

impl TestViewSpec {
    /// A phone-shaped view whose painter has nowhere to draw, which is what a
    /// test about routing, events or timers wants.
    ///
    /// `entry` is a card's MTS body, which the host serves the way
    /// `bobcat-source` registers a card's root: `MTS_CHUNK_PREAMBLE`, then the
    /// body on the preamble's own line.
    pub(crate) fn new(entry: &str) -> Self {
        Self {
            entry: entry.to_owned(),
            background: None,
            modules: Vec::new(),
            config: crate::main::tree::PageConfig::default(),
            sheets: Vec::new(),
            served_sheets: Vec::new(),
            fonts: Vec::new(),
            default_font: None,
            fetchable: FxHashSet::default(),
            probes_fetches: false,
            probes_style_sheets: false,
            echo_modules: Vec::new(),
            target: TestTarget::None,
            width: 393.0,
            height: 727.0,
        }
    }

    /// A BTS entry, which the view names at [`BACKGROUND_ENTRY`] and the host
    /// serves as this module source — a plain module, which imports what it
    /// uses from `bobcat:bts-runtime` itself. A view built without one has a
    /// BTS that imports nothing.
    pub(crate) fn with_background_entry(mut self, source: &str) -> Self {
        self.background = Some(source.to_owned());
        self
    }

    /// One more module this host serves at `url`, as it is given: a body a
    /// compiled card's BTS loads by bundle path, beside the base URL
    /// `app:///`.
    pub(crate) fn with_module(mut self, url: &str, source: &str) -> Self {
        self.modules.push((url.to_owned(), source.to_owned()));
        self
    }

    /// The page configuration the view is built with, as a bundle's own
    /// switches give it.
    pub(crate) const fn with_config(mut self, config: crate::main::tree::PageConfig) -> Self {
        self.config = config;
        self
    }

    /// One face this host serves, at the URL an `@font-face` `src` names.
    pub(crate) fn with_font(mut self, url: &str, bytes: &'static [u8]) -> Self {
        self.fonts
            .push((url.to_owned(), dom::FontBlob::from_static(bytes)));
        self
    }

    /// A face the view registers at construction, as an embedder's
    /// `ViewSources::fonts` does, and `family`, its name, as the page's
    /// default font: text no author rule gives a family shapes with it, and
    /// not with whatever the machine has.
    pub(crate) fn with_default_font(mut self, family: &str, bytes: &'static [u8]) -> Self {
        self.default_font = Some((family.to_owned(), dom::FontBlob::from_static(bytes)));
        self
    }

    /// A URL a plain fetch (`fetchResource`) succeeds for, resolved against
    /// the view's base. What it registers is whatever else this host was
    /// given: [`Self::with_module`] and [`Self::serving_style_sheet`] stand
    /// in for a container installer.
    pub(crate) fn with_fetchable(mut self, url: &str) -> Self {
        self.fetchable.insert(resolved(url));
        self
    }

    /// One sheet this host holds at `url` as CSS text, which the view does
    /// not list: an adoption by that URL is what mounts it.
    pub(crate) fn serving_style_sheet(mut self, url: &str, css: &str) -> Self {
        self.served_sheets
            .push((resolved(url), TestSheet::Text(css.to_owned())));
        self
    }

    /// Answers [`ResourceFetcher::fetch_probe`] from the plain fetches this
    /// host has completed, as a caching resource system does: a repeat
    /// `fetchResource` of one settles at once, so a `frame`'s card for it
    /// starts in a microtask rather than a later job.
    pub(crate) fn with_fetch_probe(mut self) -> Self {
        self.probes_fetches = true;
        self
    }

    /// Answers `hasStyleSheet` from the sheets this host holds, listed and
    /// served alike, rather than `true` for every URL.
    pub(crate) fn with_style_sheet_probe(mut self) -> Self {
        self.probes_style_sheets = true;
        self
    }

    /// A native module the view is built with, named `name`, with one
    /// method, `echo`, which answers through its first function argument
    /// with the call's argument list as JSON text.
    pub(crate) fn with_echo_module(mut self, name: &'static str) -> Self {
        self.echo_modules.push(name);
        self
    }

    /// One author sheet as CSS text, mounted before the entry runs.
    pub(crate) fn with_style_sheet(mut self, css: &str) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets.push((url, TestSheet::Text(css.to_owned())));
        self
    }

    /// One author sheet the host decoded itself, as a bundle embedder's does.
    pub(crate) fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets
            .push((url, TestSheet::Preparsed(Arc::new(sheet))));
        self
    }

    /// One author sheet the view lists and this host has nothing for, which
    /// fails the view's startup: boot's own `__FlushElementTree` throws the
    /// not-found error the host answers it with.
    pub(crate) fn with_missing_style_sheet(mut self) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets.push((url, TestSheet::Missing));
        self
    }

    /// A painter that renders into a texture of its own, for a test that
    /// reads pixels back.
    pub(crate) fn offscreen(mut self, width: f32, height: f32) -> Self {
        self.target = TestTarget::Offscreen;
        self.width = width;
        self.height = height;
        self
    }

    /// Builds the view alone, without waiting for it to boot — for a test
    /// about a view nobody is painting.
    pub(crate) fn create_view<R: EventRequester>(self, requester: Arc<R>) -> TestView {
        let (view, _) = self.build(requester, false);
        view
    }

    /// Builds the view and its painter without waiting for boot.
    pub(crate) fn create<R: EventRequester>(self, requester: Arc<R>) -> TestEngine {
        let (view, painter) = self.build(requester, true);
        TestEngine {
            view,
            painter: painter.expect("a painter was asked for"),
            after_boot: Vec::new(),
        }
    }

    fn build<R: EventRequester>(
        self,
        requester: Arc<R>,
        with_painter: bool,
    ) -> (TestView, Option<Painter>) {
        let Self {
            entry,
            background,
            modules,
            config,
            sheets,
            served_sheets,
            fonts,
            default_font,
            fetchable,
            probes_fetches,
            probes_style_sheets,
            echo_modules,
            target,
            width,
            height,
        } = self;
        let (default_font_family, registered) = default_font
            .map_or((None, Vec::new()), |(family, blob)| {
                (Some(family), vec![blob])
            });
        let sources = ViewSources {
            config,
            fonts: registered,
            default_font_family,
            style_sheets: sheets.iter().map(|(url, _)| url.clone()).collect(),
            background_entry: background.as_ref().map(|_| BACKGROUND_ENTRY.to_owned()),
            ..ViewSources::new(
                BASE,
                ENTRY,
                crate::ScreenMetrics::for_viewport(width, height, 1.0),
            )
        };
        let fetcher = Rc::new(InlineFetcher {
            entry: crate::main::runtime::card_entry(&entry),
            background,
            modules: modules.into_iter().collect(),
            sheets: sheets.into_iter().chain(served_sheets).collect(),
            fonts: fonts.into_iter().collect(),
            fetchable,
            fetched: probes_fetches.then(Arc::default),
            probes_style_sheets,
        });
        block_on(async move {
            let group = LynxGroup::new(requester, StyleThreads::Sequential)
                .await
                .expect("the test group starts");
            let view = group
                .create_lynx_view(
                    width,
                    height,
                    1.0,
                    |_reports| fetcher,
                    echo_modules
                        .into_iter()
                        .map(|name| {
                            Box::new(EchoModule(name))
                                as Box<dyn crate::native_module::NativeModule>
                        })
                        .collect(),
                    sources,
                )
                .expect("the test view is built");
            let painter = if with_painter {
                let mut painter = match target {
                    TestTarget::None => Painter::without_output(width, height, 1.0),
                    TestTarget::Offscreen => {
                        Painter::new(DrawTarget::Offscreen, width, height, 1.0)
                            .await
                            .expect("the test painter's target is built")
                    }
                };
                painter.attach(&view).expect("a fresh view takes a painter");
                Some(painter)
            } else {
                None
            };
            (view, painter)
        })
    }

    /// Builds the view and its painter and waits for the entry to finish.
    pub(crate) fn boot(self) -> TestEngine {
        let mut engine = self.create(Arc::new(NoWakeup));
        wait_for_boot(&mut engine);
        engine
    }
}

/// Drives ordinary turns until the entry finishes.
///
/// What the view reported before `ScriptFinished` is boot's, and this wait
/// takes it. What the same turn reported after it is not: one turn hands
/// over everything since the last, so a host that took no turn for a while —
/// a test thread the OS left unscheduled under parallel load — gets boot's
/// end and whatever the view did next in one batch, a background thread's
/// first answers included. That rest goes back to the engine, which the next
/// [`TestEngine::pump`] returns first.
pub(crate) fn wait_for_boot(engine: &mut TestEngine) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let mut events = engine.pump().into_iter();
        while let Some(event) = events.next() {
            match event {
                EngineEvent::ScriptFinished => {
                    assert!(
                        engine.published_frame().is_some(),
                        "boot's flush publishes before ScriptFinished is pumped"
                    );
                    engine.after_boot.extend(events);
                    return;
                }
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry module failed: {error}"),
                EngineEvent::Panicked(error) => panic!("the engine panicked: {error}"),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "the entry module did not finish");
        std::thread::yield_now();
    }
}

/// Runs `future` to completion on this thread.
///
/// Building a draw target is the only asynchronous part of the public API, so
/// every test needs an executor for exactly that and then drives the view and
/// its painter synchronously, as an embedder does.
pub(crate) fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a test runtime asks the platform for nothing")
        .block_on(future)
}

/// Where an [`IntersectionWatcher`] records the entries it was delivered.
pub(crate) type WatcherLog = Arc<Mutex<Vec<String>>>;

/// Everything `log` holds, leaving it empty.
pub(crate) fn take_watched(log: &WatcherLog) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

/// An engine component that observes intersection, for the tests that pin
/// when its entries are delivered: a `dom::CustomElement` defined by the test
/// itself, through a document probe, rather than a component the engine
/// ships. Its observers are created with a `dom::ElementHandler` naming it,
/// which calls its `intersections_changed`.
///
/// It records every entry it hears as `observer:target:intersecting:ratio`,
/// naming the target by its `id` attribute, and does nothing on connection —
/// the test creates its observers from the probe that created the element.
pub(crate) struct IntersectionWatcher {
    pub(crate) log: WatcherLog,
    /// Whether the next delivery also writes to the tree: set, it writes an
    /// inline `opacity` on the watcher itself and clears the flag, so one
    /// delivery mutates and the rest do not.
    pub(crate) mutate: Arc<AtomicBool>,
}

impl IntersectionWatcher {
    /// A watcher that records into `log`, with [`Self::mutate`] clear.
    pub(crate) fn new(log: &WatcherLog) -> Self {
        Self {
            log: Arc::clone(log),
            mutate: Arc::default(),
        }
    }
}

impl dom::CustomElement<()> for IntersectionWatcher {
    fn intersections_changed(
        &self,
        document: &mut dom::Document<()>,
        element: dom::NodeId,
        observer: dom::IntersectionObserverId,
        entries: Vec<dom::IntersectionObserverEntry>,
    ) {
        {
            let mut log = self.log.lock().expect("the log is never poisoned");
            for entry in entries {
                let target = document
                    .get(entry.target)
                    .and_then(|node| node.attribute("id"))
                    .expect("every target a watcher observes carries an id");
                log.push(format!(
                    "{}:{target}:{}:{}",
                    observer.get(),
                    entry.is_intersecting,
                    entry.intersection_ratio,
                ));
            }
        }
        if self.mutate.swap(false, Ordering::Relaxed) {
            document.set_inline_style_property(element, "opacity", "0.5");
        }
    }
}

mod tests {
    use super::*;

    /// A host that took no turn while boot finished and the view went on gets
    /// both in its next turn, and [`wait_for_boot`] takes boot's end alone:
    /// what came after it reaches the test's next [`TestEngine::pump`].
    ///
    /// A test thread the OS leaves unscheduled under parallel load takes that
    /// turn, with a card's background thread answering behind
    /// `ScriptFinished`. The wait used to return from the middle of it and
    /// drop the rest.
    ///
    /// The log is armed from a timer the entry armed: an epilogue runs the
    /// timers that are due before it reports the boot it finished, so the
    /// outer one may run ahead of `ScriptFinished`, and the one it arms comes
    /// due in a later epilogue. No turn is taken until that one has run — a
    /// probe and the attribute it sets are what say so — so the first turn
    /// the wait takes reports boot's end and then the log.
    #[test]
    fn what_the_turn_that_finished_boot_reported_after_it_reaches_the_next_pump() {
        let mut engine = TestViewSpec::new(
            r"
            globalThis.renderPage = function () {
              const page = __CreatePage('card', 0);
              globalThis.held = [page];
              setTimeout(() => {
                setTimeout(() => {
                  console.log('after boot');
                  __SetAttribute(page, 'logged', 'yes');
                  __FlushElementTree();
                }, 0);
              }, 0);
              __FlushElementTree();
            };
            ",
        )
        .create(Arc::new(NoWakeup));
        let deadline = Instant::now() + PATIENCE;
        while engine
            .probe_document(|document| {
                let page = document.document_element().id();
                document
                    .get(page)
                    .and_then(|node| node.attribute("logged"))
                    .is_none()
            })
            .expect("the view's task answers probes")
        {
            assert!(Instant::now() < deadline, "the inner timer never ran");
            std::thread::sleep(Duration::from_millis(1));
        }

        wait_for_boot(&mut engine);
        let logged: Vec<String> = engine
            .pump()
            .into_iter()
            .filter_map(|event| match event {
                EngineEvent::ConsoleMessage { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(logged, ["after boot"]);
    }
}
