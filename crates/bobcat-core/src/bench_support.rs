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
//!
//! It is compiled unconditionally rather than behind a feature on purpose.
//! `cargo codspeed build` builds every benchmark in the workspace with no
//! extra features, so a `required-features` benchmark would be skipped
//! silently — a measurement that quietly stops running is worse than a small
//! hidden module.

use std::sync::Arc;

use dom::event::EventSteps;

use crate::engine::{SharedListenerNames, SharedTree};
use crate::runtime::{ENTRY_PREAMBLE, MainThreadRuntime};
use crate::tree::{LynxDocument, PageConfig, Viewport, new_document};

/// A booted Element PAPI realm over a private Lynx document.
///
/// The same pair the engine's script thread owns: one realm, one document,
/// and the hand-off slot between them.
#[derive(Debug)]
pub struct ScriptHarness {
    runtime: MainThreadRuntime,
    elements: SharedTree,
    listener_names: Arc<SharedListenerNames>,
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
        let elements = SharedTree::new(new_document(
            Viewport::new(393.0, 727.0),
            PageConfig::default(),
        ));
        let factory = crate::quickjs::engine_factory();
        let listener_names = Arc::new(SharedListenerNames::default());
        let runtime = MainThreadRuntime::new(
            factory.as_ref(),
            elements.clone(),
            Arc::clone(&listener_names),
            || {},
        )
        .expect("the benchmark realm boots");
        Self {
            runtime,
            elements,
            listener_names,
        }
    }

    /// Runs a main-thread script and its `renderPage` boot, as the engine
    /// does for a card's entry script.
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
        let source = format!("{ENTRY_PREAMBLE}{source}");
        self.runtime
            .evaluate_module(&source, "bench:///step.mjs", "running a benchmark step")
            .expect("the benchmark step runs");
    }

    /// Computes the path an event on `target` would take, as the presenting
    /// side does while it holds the document.
    ///
    /// # Panics
    ///
    /// Panics if the document is not in its slot or the id is malformed.
    #[must_use]
    pub fn event_path(&self, target: u64) -> EventSteps {
        let target = dom::NodeId::from_bits(target).expect("a well-formed packed handle");
        self.document().event_steps(target, true, true)
    }

    /// Delivers one already-computed path, reporting whether anything ran.
    ///
    /// # Panics
    ///
    /// Panics if a listener throws.
    pub fn dispatch(&mut self, path: &EventSteps, name: &Arc<str>, detail: &Arc<str>) -> bool {
        self.runtime
            .dispatch_event(path, name, detail)
            .expect("the benchmark dispatch completes")
    }

    /// Whether the realm has published a listener for this event name — the
    /// question the presenting side asks before it builds anything.
    #[must_use]
    pub fn has_listeners(&self, name: &str) -> bool {
        self.listener_names.contains(name)
    }

    /// The serialized inline style of an element, so a benchmark can assert
    /// it actually did the work it timed.
    ///
    /// # Panics
    ///
    /// Panics if the document is not in its slot or the id is malformed.
    #[must_use]
    pub fn inline_style(&self, node: u64) -> Option<String> {
        let node = dom::NodeId::from_bits(node).expect("a well-formed packed handle");
        self.document()
            .get(node)
            .and_then(|node| node.attribute("style"))
            .map(str::to_owned)
    }

    /// The page root's handle, as `__CreatePage` returns it.
    ///
    /// # Panics
    ///
    /// Panics if the document is not in its slot.
    #[must_use]
    pub fn page(&self) -> u64 {
        self.document().document_element().id().to_bits()
    }

    /// The handle of a child by position, so a benchmark can name a node by
    /// where it is rather than by guessing at the order ids were issued in.
    ///
    /// # Panics
    ///
    /// Panics if the document is not in its slot or the id is malformed.
    #[must_use]
    pub fn child(&self, parent: u64, index: usize) -> Option<u64> {
        let parent = dom::NodeId::from_bits(parent).expect("a well-formed packed handle");
        let document = self.document();
        let children = document.get(parent)?.child_ids();
        children.get(index).map(|child| child.to_bits())
    }

    /// How many children an element has.
    ///
    /// # Panics
    ///
    /// Panics if the document is not in its slot or the id is malformed.
    #[must_use]
    pub fn child_count(&self, parent: u64) -> usize {
        let parent = dom::NodeId::from_bits(parent).expect("a well-formed packed handle");
        self.document()
            .get(parent)
            .map_or(0, |node| node.child_ids().len())
    }

    fn document(&self) -> impl std::ops::Deref<Target = LynxDocument> + '_ {
        self.elements
            .try_tree()
            .expect("no batch is open between benchmark steps")
    }
}

impl Default for ScriptHarness {
    fn default() -> Self {
        Self::new()
    }
}
