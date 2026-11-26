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
    FetchOutcome, LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase,
    ResourceFetcher, RetryAdvice, SourceCompletion, SourceRequest, StyleSheetProbe,
    StyleSheetSource,
};
use crate::style::PreparsedStyleSheet;
use crate::view::{
    DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, NoWakeup, StyleThreads,
    ViewSources,
};

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
pub(crate) mod fixtures;

const PATIENCE: Duration = Duration::from_secs(30);

const ENTRY: &str = "app:///main.js";

const BASE: &str = "app:///";

const BACKGROUND_ENTRY: &str = "app:///background.js";

pub(crate) enum TestSheet {
    Text(String),
    Preparsed(Arc<PreparsedStyleSheet>),
    Missing,
}

pub(crate) struct InlineFetcher {
    entry: String,
    background: Option<String>,
    modules: FxHashMap<String, String>,
    sheets: FxHashMap<String, TestSheet>,
    fonts: FxHashMap<String, terminus::link::FontBlob>,
    fetchable: FxHashSet<String>,
    fetched: Option<Arc<Mutex<FxHashSet<String>>>>,
    probes_style_sheets: bool,
    pub(crate) fetch_requests: Mutex<Vec<(String, bool)>>,
}

fn resolved(url: &str) -> String {
    url::Url::parse(BASE)
        .and_then(|base| base.join(url))
        .map_or_else(|_| url.to_owned(), String::from)
}

impl terminus::link::FrameImages for InlineFetcher {
    type PlatformFrame = std::convert::Infallible;

    fn describe(
        &self,
        _picture: &terminus::link::PictureKey,
        _hint: terminus::link::ImageSizeHint,
    ) -> Option<terminus::link::FrameImage> {
        None
    }

    fn want(&self, _picture: &terminus::link::PictureKey, _hint: terminus::link::ImageSizeHint) {}

    fn retain(&self, _frame: &[terminus::link::PictureKey]) {}
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
            SourceRequest::Fetch { url, timed } if self.fetchable.contains(&resolved(&url)) => {
                self.note_fetch(&url, timed);
                if let Some(fetched) = &self.fetched {
                    fetched
                        .lock()
                        .expect("the fetched set is never poisoned")
                        .insert(resolved(&url));
                }
                Ok(LoadedSource::Fetched(self.fetch_outcome(&url, timed)))
            }
            SourceRequest::Fetch { url, timed } => {
                self.note_fetch(&url, timed);
                Err(missing(&url))
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
            SourceRequest::Module(url) => Err(missing(&url)),
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

impl InlineFetcher {
    fn note_fetch(&self, url: &str, timed: bool) {
        self.fetch_requests
            .lock()
            .expect("the fetch log is never poisoned")
            .push((url.to_owned(), timed));
    }

    fn fetch_outcome(&self, url: &str, timed: bool) -> FetchOutcome {
        if !timed {
            return FetchOutcome::default();
        }
        let prefix = resolved(url);
        let size = self
            .modules
            .iter()
            .filter(|(served, _)| served.starts_with(&prefix))
            .filter_map(|(_, source)| u64::try_from(source.len()).ok())
            .sum();
        let arrived = crate::performance::Instant::now();
        let installed = crate::performance::Instant::now();
        FetchOutcome {
            size_bytes: Some(size),
            received: Some(arrived),
            installed: Some(arrived..installed),
        }
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

pub(crate) type TestView = LynxView<Rc<InlineFetcher>>;

pub(crate) struct TestEngine {
    pub(crate) view: TestView,
    pub(crate) painter: Painter,
    after_boot: Vec<EngineEvent>,
}

impl TestEngine {
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

    pub(crate) fn dispatch_input(&mut self, event: terminus::link::input::InputEvent) {
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

    pub(crate) fn published_frame(&mut self) -> Option<Arc<terminus::link::CommittedFrame>> {
        self.view.published_frame()
    }
}

enum TestTarget {
    None,
    Offscreen,
}

pub(crate) struct TestViewSpec {
    entry: String,
    background: Option<String>,
    modules: Vec<(String, String)>,
    config: crate::main::tree::PageConfig,
    sheets: Vec<(String, TestSheet)>,
    served_sheets: Vec<(String, TestSheet)>,
    fonts: Vec<(String, terminus::link::FontBlob)>,
    default_font: Option<(String, terminus::link::FontBlob)>,
    locale: String,
    fetchable: FxHashSet<String>,
    probes_fetches: bool,
    probes_style_sheets: bool,
    echo_modules: Vec<(&'static str, Option<crate::native_module::NativeModuleEsm>)>,
    target: TestTarget,
    width: f32,
    height: f32,
}

struct EchoModule(&'static str, Option<crate::native_module::NativeModuleEsm>);

impl crate::native_module::NativeModule for EchoModule {
    fn name(&self) -> &str {
        self.0
    }

    fn esm(&self) -> Option<crate::native_module::NativeModuleEsm> {
        self.1.clone()
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
            locale: "en-US".to_owned(),
            fetchable: FxHashSet::default(),
            probes_fetches: false,
            probes_style_sheets: false,
            echo_modules: Vec::new(),
            target: TestTarget::None,
            width: 393.0,
            height: 727.0,
        }
    }

    pub(crate) fn with_background_entry(mut self, source: &str) -> Self {
        self.background = Some(source.to_owned());
        self
    }

    pub(crate) fn with_module(mut self, url: &str, source: &str) -> Self {
        self.modules.push((url.to_owned(), source.to_owned()));
        self
    }

    pub(crate) const fn with_config(mut self, config: crate::main::tree::PageConfig) -> Self {
        self.config = config;
        self
    }

    pub(crate) fn with_font(mut self, url: &str, bytes: &'static [u8]) -> Self {
        self.fonts
            .push((url.to_owned(), terminus::link::FontBlob::from_static(bytes)));
        self
    }

    pub(crate) fn with_default_font(mut self, family: &str, bytes: &'static [u8]) -> Self {
        self.default_font = Some((
            family.to_owned(),
            terminus::link::FontBlob::from_static(bytes),
        ));
        self
    }

    pub(crate) fn with_locale(mut self, tag: &str) -> Self {
        tag.clone_into(&mut self.locale);
        self
    }

    pub(crate) fn with_fetchable(mut self, url: &str) -> Self {
        self.fetchable.insert(resolved(url));
        self
    }

    pub(crate) fn serving_style_sheet(mut self, url: &str, css: &str) -> Self {
        self.served_sheets
            .push((resolved(url), TestSheet::Text(css.to_owned())));
        self
    }

    pub(crate) fn with_fetch_probe(mut self) -> Self {
        self.probes_fetches = true;
        self
    }

    pub(crate) fn with_style_sheet_probe(mut self) -> Self {
        self.probes_style_sheets = true;
        self
    }

    pub(crate) fn with_echo_module(mut self, name: &'static str) -> Self {
        self.echo_modules.push((name, None));
        self
    }

    pub(crate) fn with_echo_esm_module(
        mut self,
        name: &'static str,
        source: &str,
        import_before_entry: bool,
    ) -> Self {
        let bytecode = crate::native_module::compile_native_module_esm(name, source)
            .expect("the test ES module compiles");
        self.echo_modules.push((
            name,
            Some(crate::native_module::NativeModuleEsm {
                bytecode: bytecode.into(),
                import_before_entry,
            }),
        ));
        self
    }

    pub(crate) fn with_style_sheet(mut self, css: &str) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets.push((url, TestSheet::Text(css.to_owned())));
        self
    }

    pub(crate) fn with_preparsed_style_sheet(mut self, sheet: PreparsedStyleSheet) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets
            .push((url, TestSheet::Preparsed(Arc::new(sheet))));
        self
    }

    pub(crate) fn with_missing_style_sheet(mut self) -> Self {
        let url = format!("app:///sheet-{}.css", self.sheets.len());
        self.sheets.push((url, TestSheet::Missing));
        self
    }

    pub(crate) fn offscreen(mut self, width: f32, height: f32) -> Self {
        self.target = TestTarget::Offscreen;
        self.width = width;
        self.height = height;
        self
    }

    pub(crate) fn create_view<R: EventRequester>(self, requester: Arc<R>) -> TestView {
        let (view, _) = self.build(requester, false);
        view
    }

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
            locale,
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
            locale,
            style_sheets: sheets.iter().map(|(url, _)| url.clone()).collect(),
            background_entry: background.as_ref().map(|_| BACKGROUND_ENTRY.to_owned()),
            ..ViewSources::new(
                BASE,
                ENTRY,
                crate::ScreenMetrics::for_viewport(width, height, 1.0),
                "en-US",
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
            fetch_requests: Mutex::default(),
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
                        .map(|(name, esm)| {
                            Box::new(EchoModule(name, esm))
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

    pub(crate) fn boot(self) -> TestEngine {
        let mut engine = self.create(Arc::new(NoWakeup));
        wait_for_boot(&mut engine);
        engine
    }
}

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

pub(crate) fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a test runtime asks the platform for nothing")
        .block_on(future)
}

pub(crate) type WatcherLog = Arc<Mutex<Vec<String>>>;

pub(crate) fn take_watched(log: &WatcherLog) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

pub(crate) struct IntersectionWatcher {
    pub(crate) log: WatcherLog,
    pub(crate) mutate: Arc<AtomicBool>,
}

impl IntersectionWatcher {
    pub(crate) fn new(log: &WatcherLog) -> Self {
        Self {
            log: Arc::clone(log),
            mutate: Arc::default(),
        }
    }
}

impl terminus::link::CustomElement<()> for IntersectionWatcher {
    fn intersections_changed(
        &self,
        document: &mut terminus::link::Document<()>,
        element: terminus::link::NodeId,
        observer: terminus::link::IntersectionObserverId,
        entries: Vec<terminus::link::IntersectionObserverEntry>,
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
