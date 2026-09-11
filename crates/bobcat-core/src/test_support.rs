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
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;

use crate::paint::Painter;
use crate::resource::{
    LoadedSource, ResourceError, ResourceErrorKind, ResourceErrorPhase, ResourceFetcher,
    RetryAdvice, SourceCompletion, SourceRequest, StyleSheetSource,
};
use crate::style::PreparsedStyleSheet;
use crate::view::{
    DrawTarget, EngineEvent, EventRequester, LynxGroup, LynxView, NoWakeup, StyleThreads,
    ViewSources,
};

/// How long a test waits on work that should already be happening.
///
/// A hang detector rather than a performance assertion: the first document a
/// test process builds pays for the platform's font registry, which is
/// seconds on some machines and nothing on others.
const PATIENCE: Duration = Duration::from_secs(30);

/// One author stylesheet, in whichever of the two forms a host has it.
pub(crate) enum TestSheet {
    Text(String),
    Preparsed(Arc<PreparsedStyleSheet>),
}

/// A host whose whole resource system is the strings it was built with.
pub(crate) struct InlineFetcher {
    entry: String,
    sheets: FxHashMap<String, TestSheet>,
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
            SourceRequest::Entry(url) => Ok(LoadedSource::Entry {
                source: self.entry.clone(),
                url,
            }),
            SourceRequest::StyleSheet(url) => match self.sheets.get(&url) {
                Some(TestSheet::Text(css)) => Ok(LoadedSource::StyleSheet(StyleSheetSource::Text(
                    css.clone(),
                ))),
                Some(TestSheet::Preparsed(sheet)) => Ok(LoadedSource::StyleSheet(
                    StyleSheetSource::Preparsed(Arc::clone(sheet)),
                )),
                None => Err(missing(&url)),
            },
            SourceRequest::Module(url) => Err(missing(&url)),
            SourceRequest::Worker { specifier, .. } => Err(missing(&specifier)),
        };
        completion.complete(answer);
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
}

impl TestEngine {
    /// One host turn: the painter's, then the view's — the order every
    /// embedder takes them in.
    pub(crate) fn pump(&mut self) -> Vec<EngineEvent> {
        self.painter
            .pump()
            .expect("an in-crate painter's target does not fail");
        self.view.pump()
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

/// One view to build: its entry, its author sheets in cascade order, and
/// where — if anywhere — its painter draws.
pub(crate) struct TestViewSpec {
    entry: String,
    sheets: Vec<(String, TestSheet)>,
    target: TestTarget,
    width: f32,
    height: f32,
}

impl TestViewSpec {
    /// A phone-shaped view whose painter has nowhere to draw, which is what a
    /// test about routing, events or timers wants.
    pub(crate) fn new(entry: &str) -> Self {
        Self {
            entry: entry.to_owned(),
            sheets: Vec::new(),
            target: TestTarget::None,
            width: 393.0,
            height: 727.0,
        }
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
        }
    }

    fn build<R: EventRequester>(
        self,
        requester: Arc<R>,
        with_painter: bool,
    ) -> (TestView, Option<Painter>) {
        let Self {
            entry,
            sheets,
            target,
            width,
            height,
        } = self;
        let sources = ViewSources {
            style_sheets: sheets.iter().map(|(url, _)| url.clone()).collect(),
            ..ViewSources::new("app:///main.js")
        };
        let fetcher = Rc::new(InlineFetcher {
            entry,
            sheets: sheets.into_iter().collect(),
        });
        block_on(async move {
            let group = LynxGroup::new(requester, StyleThreads::Sequential)
                .await
                .expect("the test group starts");
            let view = group
                .create_lynx_view(width, height, 1.0, |_reports| fetcher, sources)
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
pub(crate) fn wait_for_boot(engine: &mut TestEngine) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        for event in engine.pump() {
            match event {
                EngineEvent::ScriptFinished => {
                    assert!(
                        engine.published_frame().is_some(),
                        "boot's flush publishes before ScriptFinished is pumped"
                    );
                    return;
                }
                EngineEvent::StartupFailed(error) => panic!("the view did not boot: {error}"),
                EngineEvent::ScriptRunError(error) => panic!("the entry module failed: {error}"),
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
