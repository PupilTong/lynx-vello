//! The seam the crate's own benchmarks drive the private runtime through.
//!
//! Everything the composed script boundary does — the Element PAPI evaluating
//! in a realm, host members reaching the document, an event path walking back
//! into the realm — lives behind `pub(crate)` types, because none of it is a
//! contract an embedder should see. A benchmark is a separate crate, so it
//! cannot reach any of it.
//!
//! This module is the deliberate exception: a small, purpose-built harness,
//! not a re-export of the runtime. It is `#[doc(hidden)]` and carries no
//! stability promise.

use std::rc::Rc;
use std::sync::Arc;

use terminus::link::event::EventSteps;
use tokio::sync::{mpsc, watch};

use crate::background::{WorkerCommand, WorkerEvent};
use crate::esm::{MTS_CHUNK_PREAMBLE, build_main_runtime};
use crate::jobs::JsThread;
use crate::link::{DetachedView, InputEventPayload, ScrollEntry, detached_outbox};
use crate::main::WorkerFactory;
use crate::main::quickjs::ScriptRuntime;
use crate::main::runtime::{DocumentIngredients, MainThreadRuntime, RealmStartup};
use crate::main::tree::{LynxDocument, PageConfig, Viewport};
use crate::view::NoWakeup;

pub struct ScriptHarness {
    js_runtime: ScriptRuntime,
    runtime: MainThreadRuntime,
    _thread: Rc<JsThread>,
    view: DetachedView,
    _workers: mpsc::UnboundedReceiver<WorkerCommand>,
    _worker_events: mpsc::UnboundedReceiver<WorkerEvent>,
}

impl std::fmt::Debug for ScriptHarness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScriptHarness")
            .field("js_runtime", &self.js_runtime)
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl ScriptHarness {
    #[must_use]
    pub fn new() -> Self {
        let viewport = Viewport::new(393.0, 727.0);
        let ingredients = DocumentIngredients {
            viewport,
            config: PageConfig::default(),
            text_context: crate::view::stage_text_context(Vec::new(), None, Vec::new(), "en-US")
                .expect("en-US is a language tag"),
            style_pool: None,
        };
        let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
        let mut js_runtime = build_main_runtime().expect("the benchmark runtime builds");
        let (workers, inbox) = mpsc::unbounded_channel();
        let thread = JsThread::new();
        let (runtime, worker_events) = MainThreadRuntime::new(
            &mut js_runtime,
            ingredients,
            watch::channel(Some(viewport)).1,
            outbox,
            &WorkerFactory::new(workers, Arc::default()),
            thread.handle(),
            RealmStartup::default(),
        )
        .expect("the benchmark realm boots");
        Self {
            js_runtime,
            runtime,
            _thread: thread,
            view,
            _workers: inbox,
            _worker_events: worker_events,
        }
    }

    pub fn boot(&mut self, source: &str) {
        self.runtime
            .run_main_thread_script(&mut self.js_runtime, source, "bench:///main.js")
            .expect("the benchmark script boots");
    }

    pub fn evaluate(&mut self, source: &str) {
        let source = format!("{MTS_CHUNK_PREAMBLE}{source}");
        self.runtime
            .evaluate_module(
                &mut self.js_runtime,
                &source,
                "bench:///step.mjs",
                "running a benchmark step",
            )
            .expect("the benchmark step runs");
    }

    #[must_use]
    pub fn event_path(&mut self, target: u64) -> EventSteps {
        let target =
            terminus::link::NodeId::from_bits(target).expect("a well-formed packed handle");
        self.with_document(|document| document.event_steps(target, true, true))
    }

    pub fn dispatch(&mut self, target: u64, name: &Arc<str>) -> bool {
        let target =
            terminus::link::NodeId::from_bits(target).expect("a well-formed packed handle");
        let payload = InputEventPayload {
            detail: terminus::link::event::EventDetail::Pointer {
                position: terminus::link::Point2D::new(123.5, 456.25),
                id: 1,
                device: terminus::link::input::PointerKind::Touch,
                button: 0,
                buttons: 0,
            },
            ..InputEventPayload::default()
        };
        let mut failures = Vec::new();
        let called = self.runtime.dispatch_input_event(
            &mut self.js_runtime,
            target,
            name,
            &payload,
            None,
            &mut failures,
        );
        assert!(
            failures.is_empty(),
            "the benchmark dispatch completes: {failures:?}"
        );
        called
    }

    pub fn scroll_frame(&mut self, posts: &[ScrollPost]) -> bool {
        self.runtime.adopt_scroll_offsets(posts.iter().map(|post| {
            let node =
                terminus::link::NodeId::from_bits(post.node).expect("a well-formed packed handle");
            let offset = terminus::link::Vector2D::new(0.0, post.y);
            let entry = ScrollEntry {
                offset,
                at: 0.0,
                rest: post.rest.then_some(offset),
                request: None,
                anchor_shift: terminus::link::Vector2D::zero(),
                reach: terminus::link::Vector2D::zero(),
            };
            (node, entry)
        }));
        self.runtime.commit_if_dirty();
        self.runtime.queue_instant_scroll_events();
        if !self.runtime.has_component_events() || self.runtime.needs_render() {
            return false;
        }
        let failures = self.runtime.dispatch_component_events(&mut self.js_runtime);
        assert!(failures.is_empty(), "no listener throws: {failures:?}");
        true
    }

    pub fn has_listeners(&mut self, name: &str) -> bool {
        self.view.published.sync();
        self.view.published.has_listener(name)
    }

    #[must_use]
    pub fn inline_style(&mut self, node: u64) -> Option<String> {
        let node = terminus::link::NodeId::from_bits(node).expect("a well-formed packed handle");
        self.with_document(|document| {
            document
                .get(node)
                .and_then(|node| node.attribute("style"))
                .map(std::borrow::Cow::into_owned)
        })
    }

    #[must_use]
    pub fn page(&mut self) -> u64 {
        self.with_document(|document| document.document_element().id().to_bits())
    }

    #[must_use]
    pub fn child(&mut self, parent: u64, index: usize) -> Option<u64> {
        let parent =
            terminus::link::NodeId::from_bits(parent).expect("a well-formed packed handle");
        self.with_document(|document| {
            let children = document.get(parent)?.child_ids();
            children
                .get(index)
                .copied()
                .map(terminus::link::NodeId::to_bits)
        })
    }

    #[must_use]
    pub fn child_count(&mut self, parent: u64) -> usize {
        let parent =
            terminus::link::NodeId::from_bits(parent).expect("a well-formed packed handle");
        self.with_document(|document| {
            document
                .get(parent)
                .map_or(0, |node| node.child_ids().len())
        })
    }

    fn with_document<R>(&mut self, probe: impl FnOnce(&mut LynxDocument) -> R) -> R {
        self.runtime.with_document(probe)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollPost {
    pub node: u64,
    pub y: f32,
    pub rest: bool,
}

impl Default for ScriptHarness {
    fn default() -> Self {
        Self::new()
    }
}
