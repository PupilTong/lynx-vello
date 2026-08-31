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

use std::sync::Arc;

use dom::event::EventSteps;

use crate::main::runtime::{MainThreadRuntime, entry_module_source};
use crate::main::tree::{LynxDocument, PageConfig, Viewport, new_document};
use crate::paint::PresenterLink;
use crate::view::{NoWakeup, main_link};

/// A booted Element PAPI realm over a private Lynx document.
///
/// The same pair the engine's main thread owns: one realm and the document
/// it holds outright.
#[derive(Debug)]
pub struct ScriptHarness {
    runtime: MainThreadRuntime<NoWakeup>,
    /// The presenting end of the same link the engine builds, so a benchmark
    /// can ask the question the router asks — and pay what it pays.
    link: PresenterLink,
}

impl ScriptHarness {
    /// Boots a realm over a fresh document at a phone-shaped viewport.
    ///
    /// # Panics
    ///
    /// Panics if the realm cannot be created, which for a benchmark is the
    /// only useful response.
    #[must_use]
    pub fn new() -> Self {
        let document = new_document(Viewport::new(393.0, 727.0), PageConfig::default());
        let (presenter, main) = main_link(Arc::new(NoWakeup));
        let runtime =
            MainThreadRuntime::new(document, main.notify).expect("the benchmark realm boots");
        Self {
            runtime,
            link: presenter,
        }
    }

    /// Runs a main-thread script and its global-function or engine-event boot,
    /// as the engine does for a card's entry script.
    ///
    /// # Panics
    ///
    /// Panics if the script fails.
    pub fn boot(&mut self, source: &str) {
        self.runtime
            .run_main_thread_script(source, "bench:///main.js")
            .expect("the benchmark script boots");
    }

    /// Evaluates a snippet in the booted realm, as one more module in the
    /// graph the entry booted through.
    ///
    /// The entry's own preamble is prepended so a step sees exactly the PAPI
    /// surface a card's entry module sees. The import is a registry lookup
    /// after the first step — `bobcat:element` is already instantiated — so
    /// what a step measures is still the work in its body.
    ///
    /// # Panics
    ///
    /// Panics if the snippet throws.
    pub fn evaluate(&mut self, source: &str) {
        let source = entry_module_source(source);
        self.runtime
            .evaluate_module(&source, "bench:///step.mjs", "running a benchmark step")
            .expect("the benchmark step runs");
    }

    /// Computes the path an event on `target` would take, as delivery does
    /// on the document's owner thread.
    ///
    /// # Panics
    ///
    /// Panics if the id is malformed.
    #[must_use]
    pub fn event_path(&mut self, target: u64) -> EventSteps {
        let target = dom::NodeId::from_bits(target).expect("a well-formed packed handle");
        self.runtime
            .with_document(|document| document.event_steps(target, true, true))
    }

    /// Delivers one routed event to `target`, reporting whether anything ran.
    ///
    /// # Panics
    ///
    /// Panics if a listener throws or the id is malformed.
    pub fn dispatch(&mut self, target: u64, name: &Arc<str>, detail: &Arc<str>) -> bool {
        let target = dom::NodeId::from_bits(target).expect("a well-formed packed handle");
        self.runtime
            .dispatch_event(target, name, detail)
            .expect("the benchmark dispatch completes")
    }

    /// Whether the realm has published a listener for this event name — the
    /// question the presenting side asks before it builds anything.
    ///
    /// Resyncs first, as the start of a routing pass does, so the answer
    /// accounts for every registration the realm has made so far rather than
    /// only those a previous call happened to drain.
    pub fn has_listeners(&mut self, name: &str) -> bool {
        self.link.sync();
        self.link.has_listener(name)
    }

    /// The serialized inline style of an element, so a benchmark can assert
    /// it actually did the work it timed.
    ///
    /// # Panics
    ///
    /// Panics if the id is malformed.
    #[must_use]
    pub fn inline_style(&mut self, node: u64) -> Option<String> {
        let node = dom::NodeId::from_bits(node).expect("a well-formed packed handle");
        self.with_document(|document| {
            document
                .get(node)
                .and_then(|node| node.attribute("style"))
                .map(str::to_owned)
        })
    }

    /// The page root's handle, as `__CreatePage` returns it.
    #[must_use]
    pub fn page(&mut self) -> u64 {
        self.with_document(|document| document.document_element().id().to_bits())
    }

    /// The handle of a child by position, so a benchmark can name a node by
    /// where it is rather than by guessing at the order ids were issued in.
    ///
    /// # Panics
    ///
    /// Panics if the id is malformed.
    #[must_use]
    pub fn child(&mut self, parent: u64, index: usize) -> Option<u64> {
        let parent = dom::NodeId::from_bits(parent).expect("a well-formed packed handle");
        self.with_document(|document| {
            let children = document.get(parent)?.child_ids();
            children.get(index).copied().map(dom::NodeId::to_bits)
        })
    }

    /// How many children an element has.
    ///
    /// # Panics
    ///
    /// Panics if the id is malformed.
    #[must_use]
    pub fn child_count(&mut self, parent: u64) -> usize {
        let parent = dom::NodeId::from_bits(parent).expect("a well-formed packed handle");
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

impl Default for ScriptHarness {
    fn default() -> Self {
        Self::new()
    }
}
