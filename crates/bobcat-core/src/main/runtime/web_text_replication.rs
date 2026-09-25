//! Replicas of `lynx-stack`'s web-platform text tests that drive the engine
//! the way a compiled card does: through the Element PAPI, from the
//! main-thread script realm.
//!
//! The reference is web-core, not the Android/iOS platform engines
//! (`AGENTS.md`), so every assertion here states what a `.web.bundle` does
//! under `web-core` today. Where this engine does not reach that yet the
//! replica is `#[ignore]`d with the cause, never weakened: an ignored test
//! still compiles and still asserts the reference.
//!
//! One deliberate exception to that rule is on record. The truncation marker
//! `text-maxlength` appends is gated on `text-overflow: ellipsis` here
//! (`crates/hughie/src/text/block/truncate.rs`), which neither the initial
//! value `clip` nor the Lynx UA sheet ever sets, where both web-core and the
//! native platforms append it unconditionally. The 2026-09-14 ruling adopts
//! this engine's gating as the intended behavior, so the maxlength replicas
//! below assert the bare cut and state in their own doc comments what the
//! references render instead.
//!
//! Geometry is measured with the vendored Ahem face, whose glyphs are solid
//! em squares, so a run's advance is exactly its glyph count times its font
//! size and every metric below is an exact number rather than a tolerance.
//!
//! # The cases that are not replicated here
//!
//! Two of the assigned cases have no replica, each for its own reason.
//!
//! `reactlynx/api-SelectorQuery` (`web-core-e2e/tests/reactlynx.spec.ts:1515`)
//! runs 30 selector forms and asks each subject for its `boundingClientRect`,
//! asserting only the `id` the answer carries. Both halves of that are
//! reachable now: a background-thread card builds a `SelectorQuery`
//! (`packages/bobcat-element/src/selector-query.ts`) whose requests
//! `__BobcatQueryNodes` answers against the document's own selector engine,
//! and its `invoke` operation reaches `__InvokeUIMethod`, which answers the
//! rect together with the element's `id` and `dataset`. It is still not
//! replicated here, for a reason that has nothing to do with gaps: the card
//! asserts no text behaviour at all. Its rows are produced *by*
//! `SelectorQuery`, so its screenshot cannot be reconstructed from the
//! outside; what can be replicated of it is the selector matching (in
//! `crates/dom`) and the 8px-on-10px row geometry (in the tree replicas),
//! neither of which belongs here.
//!
//! `web-core-e2e/web-core/add-class-css-og-style-font-size` calls `__AddClass`,
//! which is not implemented and is not a global (`element-papi.ts:81-88`
//! names it among the members a bundle fails at). A replica would die on a
//! `ReferenceError` at its first line and would pin the absence of a *name*,
//! not any text behaviour — it could not state what web-core renders, so it
//! could not be strengthened into a reference assertion later.
//!
//! That is what separates it from `x-text/event-layoutchange`,
//! `text/bindlayout` and the four `text/set-native-props-*` cases, which the
//! assignment files under the same `blocked-by-engine-gap` verdict but which
//! *are* written as `#[ignore]`d replicas below: every member those call
//! exists, so the replica runs the whole card, states the reference, and fails
//! on the missing behaviour rather than on a missing identifier. The
//! `set-native-props` four became writable with `__SetDataset`/`__GetDataset`/
//! `__AddDataset` (`element-papi.ts:973-988`) and the background-thread
//! `SelectorQuery`, whose `setNativeProps` operation
//! (`element-papi.ts:1226-1235`) routes each prop to `__AddInlineStyle` or
//! `__SetAttribute` and flushes.

use std::time::Duration;

use dom::stylo::color::AbsoluteColor;
use dom::stylo::values::computed::FontStyle;
use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerCommand, WorkerEvent, WorkerHome};
use crate::jobs::JsThread;
use crate::link::{DetachedView, block_on_deadline, detached_outbox};
use crate::main::tree::{PageConfig, Viewport};
use crate::main::workers::WorkerFactory;
use crate::resource::StyleSheetSource;
use crate::view::NoWakeup;

/// Solid em squares, so a run's advance is its glyph count times its font
/// size and every metric below is an exact number rather than a tolerance.
const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

/// How long a test waits for a thread that should already be working.
const PATIENCE: Duration = Duration::from_secs(30);

/// A same-thread window onto the realm-owned document, so a replica can read
/// back what script built without going through the runtime's own methods —
/// the stand-in for the `rootDom.querySelector` the ported tests use.
struct DocumentProbe {
    slot: Rc<RefCell<DocumentSlot>>,
    // These replicas exercise only MTS. Keep the worker boundary's far ends
    // open so the realm's own boot succeeds.
    _workers: mpsc::UnboundedReceiver<WorkerCommand>,
    _worker_events: mpsc::UnboundedReceiver<WorkerEvent>,
    /// The engine thread the realm was opened with, held for its life.
    _thread: Rc<JsThread>,
}

impl DocumentProbe {
    /// The document the realm's boot module created.
    fn tree(&self) -> RefMut<'_, LynxDocument> {
        RefMut::filter_map(self.slot.borrow_mut(), |slot| slot.document.as_mut())
            .ok()
            .expect("the realm has created its document")
    }
}

/// A runtime over a document that can shape text.
fn text_runtime() -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let mut text = dom::TextContext::new();
    assert_eq!(text.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
    let mut ingredients =
        DocumentIngredients::for_test(Viewport::new(393.0, 727.0), PageConfig::default());
    ingredients.text_context = Some(text);
    runtime_over(ingredients)
}

/// The same, booted over `card` with the card's own stylesheet — the
/// `styleInfo` half of a bundle, which the Element PAPI never carries. It is
/// mounted the way a view's fetched sheets are, on the document boot created,
/// and here before the entry runs.
fn text_card_with_author_css(
    css: &str,
    card: &str,
    name: &str,
) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script_over_sheets(
            &mut js_runtime,
            vec![(
                "app:///index.css",
                LoadedSource::StyleSheet(StyleSheetSource::Text(css.into())),
            )],
            card,
            name,
        )
        .expect("main-thread script");
    (js_runtime, runtime, elements)
}

fn runtime_over(
    ingredients: DocumentIngredients,
) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let (outbox, _far_end) = detached_outbox(Arc::new(NoWakeup));
    let mut js_runtime = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js_runtime).expect("the shared modules register");
    let (workers, inbox) = mpsc::unbounded_channel();
    let thread = JsThread::new();
    let (runtime, worker_events) = MainThreadRuntime::new(
        &mut js_runtime,
        ingredients,
        bound_metrics(Viewport::new(393.0, 727.0)),
        outbox,
        &WorkerFactory::new(workers, Arc::default()),
        thread.handle(),
        RealmStartup::default(),
    )
    .expect("main-thread runtime");
    let probe = DocumentProbe {
        slot: Rc::clone(&runtime.slot),
        _workers: inbox,
        _worker_events: worker_events,
        _thread: thread,
    };
    (js_runtime, runtime, probe)
}

/// A realm with a real background thread behind it — the shape
/// `worker_tests.rs` builds, narrowed to what a text card needs. The card's
/// main-thread script and its background entry both run for real, and the
/// test plays the view: it takes each worker event off the channel and hands
/// it to the realm, which is what a card's `SelectorQuery` requests travel on.
struct BackgroundPair {
    runtime: MainThreadRuntime,
    js: ScriptRuntime,
    events: mpsc::UnboundedReceiver<WorkerEvent>,
    slot: Rc<RefCell<DocumentSlot>>,
    /// The host end of the view's link, held open for as long as the realm is.
    _view: DetachedView,
    /// The engine thread the realm was opened with, held for its life.
    _thread: Rc<JsThread>,
    /// **Last field, and it must stay last.** Fields drop in declaration
    /// order, and the realm holds a sender on the thread this owns.
    _home: WorkerHome,
}

impl BackgroundPair {
    /// The document the realm's boot module created.
    fn tree(&self) -> RefMut<'_, LynxDocument> {
        RefMut::filter_map(self.slot.borrow_mut(), |slot| slot.document.as_mut())
            .ok()
            .expect("the realm has created its document")
    }

    /// Waits for one worker event and hands it to the realm, as the view's
    /// own task does.
    fn deliver(&mut self) {
        let deadline = ClockInstant::now() + PATIENCE;
        let event = block_on_deadline(self.events.recv(), deadline)
            .flatten()
            .expect("a worker event arrives");
        self.runtime
            .dispatch_worker_event(&mut self.js, event.key, event.payload)
            .expect("the realm accepts its background thread's event");
    }
}

/// A runtime over an Ahem-shaping document, with `background` running as the
/// card's background-thread entry and `main` as its main-thread script.
fn background_pair(main: &str, background: &str) -> BackgroundPair {
    let home =
        WorkerHome::with_entry_for_test((background.to_owned(), "test:bts-entry".to_owned()));
    let (outbox, view) = detached_outbox(Arc::new(NoWakeup));
    let mut js = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js).expect("the shared modules register");
    let mut text = dom::TextContext::new();
    assert_eq!(text.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
    let mut ingredients =
        DocumentIngredients::for_test(Viewport::new(393.0, 727.0), PageConfig::default());
    ingredients.text_context = Some(text);
    let thread = JsThread::new();
    let (mut runtime, events) = MainThreadRuntime::new(
        &mut js,
        ingredients,
        bound_metrics(Viewport::new(393.0, 727.0)),
        outbox,
        &WorkerFactory::new(home.commands(), home.trapped()),
        thread.handle(),
        // The main script boots below, which is what answers this realm's
        // entry request; this names the BTS entry and nothing else.
        RealmStartup {
            background_entry: Some("test:bts-entry".to_owned()),
            ..RealmStartup::default()
        },
    )
    .expect("main-thread runtime");
    let slot = Rc::clone(&runtime.slot);
    runtime
        .run_main_thread_script(&mut js, main, "app:///main.js")
        .expect("main-thread script");
    BackgroundPair {
        runtime,
        js,
        events,
        slot,
        _view: view,
        _thread: thread,
        _home: home,
    }
}

/// Every descendant of `root` in document order — the sequence
/// `querySelectorAll` walks, which is what the ported counting assertions
/// depend on.
fn descendants(tree: &LynxDocument, root: dom::NodeId, out: &mut Vec<dom::NodeId>) {
    let children: Vec<dom::NodeId> = match tree.get(root) {
        Some(node) => node.children().map(dom::Node::id).collect(),
        None => return,
    };
    for child in children {
        out.push(child);
        descendants(tree, child, &mut *out);
    }
}

/// What every `raw-text` under `root` currently carries, in document order.
/// The ported tests read content off the carrier's `text` attribute rather
/// than off `textContent`, so this is the same query they make.
fn carrier_contents(tree: &LynxDocument, root: dom::NodeId) -> Vec<String> {
    let mut ids = Vec::new();
    descendants(tree, root, &mut ids);
    ids.into_iter()
        .filter(|id| tree.get(*id).and_then(dom::Node::tag_name) == Some("raw-text"))
        .map(|id| {
            tree.get(id)
                .and_then(|node| node.attribute("text"))
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// The element script gave `id`, found without the test having to know which
/// packed handle the PAPI minted for it.
fn element_with_id(tree: &LynxDocument, id: &str) -> dom::NodeId {
    let page = tree.document_element().id();
    let mut ids = vec![page];
    descendants(tree, page, &mut ids);
    ids.into_iter()
        .find(|node| tree.get(*node).and_then(|node| node.attribute("id")) == Some(id))
        .unwrap_or_else(|| panic!("no element carries the id {id:?}"))
}

/// The computed colour of a live element.
fn color(tree: &LynxDocument, element: dom::NodeId) -> AbsoluteColor {
    computed(tree, element).clone_color()
}

/// The computed font size, in CSS pixels.
fn font_size(tree: &LynxDocument, element: dom::NodeId) -> f32 {
    computed(tree, element)
        .clone_font_size()
        .computed_size()
        .px()
}

fn font_style(tree: &LynxDocument, element: dom::NodeId) -> FontStyle {
    computed(tree, element).clone_font_style()
}

fn computed(
    tree: &LynxDocument,
    element: dom::NodeId,
) -> dom::stylo::servo_arc::Arc<dom::stylo::properties::ComputedValues> {
    tree.get(element)
        .expect("a live element")
        .computed_style()
        .expect("a flushed element has computed style")
}

fn rgb(red: u8, green: u8, blue: u8) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(red, green, blue, 1.0)
}

/// Replicates `web-core/element-apis/__CreateText`
/// (`web-platform/web-core/tests/element-apis.spec.ts:529`): the node
/// `__CreateText` mints answers `__GetTag` with the Lynx tag `text`.
///
/// The source test leaves the node detached; this boot flushes an element
/// tree, so the replica hangs it under the page. That is the only change —
/// the assertion is still `__GetTag` on what `__CreateText(0)` returned.
#[test]
fn create_text_mints_an_element_tagged_text() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __AppendElement(page, text);
                  globalThis.tag = __GetTag(text);
                };
                ",
            "app:///create-text.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (globalThis.tag !== 'text') throw new Error('got ' + globalThis.tag);",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    let tree = elements.tree();
    assert_eq!(
        tree.document_element()
            .first_child()
            .and_then(dom::Node::tag_name),
        Some("text"),
        "and the host tree agrees with the tag script was handed"
    );
}

/// Replicates `web-core/element-apis/__CreateRawText`
/// (`web-platform/web-core/tests/element-apis.spec.ts:539`): a carrier's tag
/// is `raw-text` and its constructor argument lands on the `text` attribute
/// — Lynx writes a run as an attribute, not as character data.
#[test]
fn create_raw_text_carries_its_content_as_the_text_attribute() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const raw = __CreateRawText('content');
                  __AppendElement(page, raw);
                  globalThis.seen = [__GetTag(raw), __GetAttributeByName(raw, 'text')];
                };
                ",
            "app:///create-raw-text.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (globalThis.seen.join('|') !== 'raw-text|content') {
                  throw new Error('got ' + globalThis.seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    let tree = elements.tree();
    let page = tree.document_element().id();
    assert_eq!(carrier_contents(&tree, page), vec!["content".to_owned()]);
}

/// Replicates `web-core/testing-library-port/should create and append text
/// with raw text`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:94`): the
/// canonical construction path — `__CreateText` + `__CreateRawText` +
/// `__AppendElement`, flushed — produces one text element holding one
/// carrier, and the carrier's string is what the paragraph measures.
#[test]
fn a_text_and_its_carrier_survive_a_flush_as_one_measured_paragraph() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view0 = __CreateView(0);
                  __AppendElement(page, view0);

                  const text0 = __CreateText(0);
                  __SetInlineStyles(text0, 'font-family:Ahem;font-size:20px');
                  const rawText0 = __CreateRawText('Text Element');

                  __AppendElement(text0, rawText0);
                  __AppendElement(view0, text0);
                  __FlushElementTree();
                  globalThis.tag = __GetTag(text0);
                };
                ",
            "app:///create-and-append.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            "if (globalThis.tag !== 'text') throw new Error('got ' + globalThis.tag);",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    let tree = elements.tree();
    let view = tree
        .document_element()
        .first_child()
        .expect("the view")
        .id();
    let text = tree
        .get(view)
        .and_then(dom::Node::first_child)
        .expect("the text")
        .id();
    assert_eq!(tree.get(text).and_then(dom::Node::tag_name), Some("text"));
    assert_eq!(
        carrier_contents(&tree, text),
        vec!["Text Element".to_owned()],
        "one carrier, holding the whole string"
    );
    // Twelve Ahem em squares at 20px. The carrier reflects into a run the
    // text element's paragraph owns, so the measurement lives on the element.
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.width - 240.0).abs() < f32::EPSILON,
        "twelve em squares at 20px, got {measured:?}"
    );
}

/// Replicates `web-core/testing-library-port/text should work with
/// SetAttribute`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:151`):
/// `__SetAttribute(carrier, 'text', ...)` — the call `ReactLynx`'s compiled
/// update slots emit — fully replaces the constructor-supplied content
/// rather than appending to it.
#[test]
fn setting_the_text_attribute_replaces_the_constructor_supplied_content() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text0 = __CreateText(0);
                  __SetInlineStyles(text0, 'font-family:Ahem;font-size:20px');
                  // The constructor content is literally the string
                  // 'raw-text', and none of it may survive.
                  const rawText0 = __CreateRawText('raw-text');

                  __AppendElement(text0, rawText0);
                  __SetAttribute(rawText0, 'text', 'Hello World');

                  __AppendElement(page, text0);
                };
                ",
            "app:///set-attribute.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let text = tree
        .document_element()
        .first_child()
        .expect("the text")
        .id();
    assert_eq!(
        carrier_contents(&tree, text),
        vec!["Hello World".to_owned()],
        "the write replaces the old value, it does not add a second carrier"
    );
    let carrier = tree
        .get(text)
        .and_then(dom::Node::first_child)
        .expect("the carrier")
        .id();
    assert!(
        tree.get(carrier)
            .expect("the carrier")
            .child_ids()
            .is_empty(),
        "the run is generated content off the attribute, not a DOM text child \
         — crates/bobcat-core/src/main/tree/raw_text.rs:1-11 — so the write \
         above is the whole of the content, with nothing left behind it"
    );
    // Eleven em squares: the shorter 'raw-text' is gone from the paragraph.
    // With no DOM text child to read, this measurement is where the new value
    // is observed reaching the run: the replaced 'raw-text' is eight units and
    // would measure 160.
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.width - 220.0).abs() < f32::EPSILON,
        "eleven em squares at 20px, got {measured:?}"
    );
}

/// Replicates `web-core/testing-library-port/should handle empty text
/// content`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:169`): a
/// childless `text` still materialises as an element, reports zero children,
/// and contributes no content — nothing synthesises a placeholder run.
#[test]
fn a_childless_text_exists_and_carries_no_content() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text0 = __CreateText(0);
                  __AppendElement(page, text0);
                  globalThis.seen = [__GetTag(text0), String(__GetChildren(text0).length)];
                };
                ",
            "app:///empty-text.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (globalThis.seen.join('|') !== 'text|0') {
                  throw new Error('got ' + globalThis.seen.join('|'));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    let tree = elements.tree();
    let text = tree
        .document_element()
        .first_child()
        .expect("the text")
        .id();
    assert!(
        tree.get(text).and_then(dom::Node::first_child).is_none(),
        "no substitute child stands in for the missing run"
    );
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        measured.width.abs() < f32::EPSILON && measured.height.abs() < f32::EPSILON,
        "an empty paragraph claims no line box, got {measured:?}"
    );
}

/// Replicates `web-core/testing-library-port/should be case sensitive`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:181`): nothing
/// on the carrier path case-folds a run.
#[test]
fn raw_text_content_round_trips_with_its_case_intact() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text0 = __CreateText(0);
                  const rawText0 = __CreateRawText('Sensitive text');
                  __AppendElement(text0, rawText0);
                  __AppendElement(page, text0);
                  globalThis.content = __GetAttributeByName(rawText0, 'text');
                };
                ",
            "app:///case-sensitive.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                if (globalThis.content !== 'Sensitive text') {
                  throw new Error('got ' + globalThis.content);
                }
                if (globalThis.content === 'sensitive text') {
                  throw new Error('the content was case folded');
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    let tree = elements.tree();
    let page = tree.document_element().id();
    assert_eq!(
        carrier_contents(&tree, page),
        vec!["Sensitive text".to_owned()]
    );
}

/// Replicates `web-core/testing-library-port/to-have-text-content: handles
/// positive test cases`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:229`): a
/// tagged `text` is findable, and its content is read back off the carrier's
/// `text` attribute — the source test's `textContent` assertion is commented
/// out precisely because content lives on the attribute.
///
/// The source tags the element with `__AddDataset(text0, 'testid', ...)` and
/// then finds it with a `[data-testid=…]` selector. `__AddDataset` exists here
/// (`packages/bobcat-element/src/element-papi.ts:985-988`), but it files a
/// typed value in this realm's own per-element store, not a `data-*`
/// attribute on the node — so nothing in the document carries what that
/// selector matches, and the lookup half of the source case stays
/// unreplicable. The replica therefore tags with `__SetID`; what is under test
/// is the content read, not the tagging member.
///
/// Be honest about what that substitution costs. Once the tag becomes an id
/// this replica is structurally the multiple-levels case below
/// (`a_carrier_is_reachable_as_a_descendant_of_the_id_d_text`) with a different
/// content string; the two pin the same property. The dataset lookup itself is
/// unreplicated, and a `[data-*]` selector over a dataset entry would be a new
/// case rather than this one — it would state nothing about text.
#[test]
fn a_tagged_text_is_found_and_reads_its_content_back() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text0 = __CreateText(0);
                  const rawText0 = __CreateRawText('2');
                  __AppendElement(text0, rawText0);
                  __AppendElement(page, text0);
                  __SetID(text0, 'count-value');
                };
                ",
            "app:///text-content-positive.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let text = element_with_id(&tree, "count-value");
    assert_eq!(carrier_contents(&tree, text), vec!["2".to_owned()]);
}

/// Replicates `web-core/testing-library-port/to-have-text-content: can
/// handle multiple levels`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:259`): the
/// carrier is reachable as a *descendant* of the id'd `text`, not only by a
/// direct-child lookup.
///
/// The run itself is a text node rather than an element child, which is why
/// the descendant walk stops at the carrier and the run is read through it.
#[test]
fn a_carrier_is_reachable_as_a_descendant_of_the_id_d_text() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text0 = __CreateText(0);
                  const rawText0 = __CreateRawText('Step 1 of 4');
                  __AppendElement(text0, rawText0);
                  __AppendElement(page, text0);
                  __SetID(text0, 'parent');
                };
                ",
            "app:///text-content-levels.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let parent = element_with_id(&tree, "parent");
    assert_eq!(
        carrier_contents(&tree, parent),
        vec!["Step 1 of 4".to_owned()]
    );
}

/// Replicates `web-core/testing-library-port/to-have-text-content: can
/// handle multiple levels with content spread across descendants`
/// (`web-platform/web-core/tests/testing-library-port.spec.ts:273`): two
/// sibling `text` elements under one `view` yield exactly two carriers, in
/// document order, with nothing merged or reordered.
#[test]
fn content_spread_across_sibling_texts_keeps_its_count_and_order() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const view = __CreateView(0);
                  const text0 = __CreateText(0);
                  const rawText0 = __CreateRawText('Step');
                  const text1 = __CreateText(0);
                  const rawText1 = __CreateRawText('1');

                  __AppendElement(text0, rawText0);
                  __AppendElement(text1, rawText1);
                  __AppendElement(view, text0);
                  __AppendElement(view, text1);
                  __AppendElement(page, view);
                  __SetID(view, 'parent');
                };
                ",
            "app:///text-content-spread.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let parent = element_with_id(&tree, "parent");
    assert_eq!(
        carrier_contents(&tree, parent),
        vec!["Step".to_owned(), "1".to_owned()]
    );
}

/// Replicates `web-core/server-ssr-bulk/__SetInlineStyles object form`
/// (`web-platform/web-core/tests/server-ssr-bulk.spec.ts:10`): the record
/// form of `__SetInlineStyles` takes kebab-case keys and passes the text
/// declarations through with no unit transform and no property rename.
///
/// The source asserts the SSR-emitted HTML string (`color:red;`,
/// `font-size:16px;`, `margin-top:10px;`). There is no SSR serializer here,
/// so the replica asserts the same declarations where they end up instead:
/// the computed colour and font size, and the 10px the margin pushes the box
/// down by.
///
/// 16px is also stylo's own default font size
/// (`vendor/stylo/style/values/specified/font.rs`, `FONT_MEDIUM_PX`), so
/// asserting it against a bare document would pass just as well if the record
/// form silently dropped the key. The card therefore declares
/// `page { font-size: 32px }`: 32px is what the element inherits when the
/// declaration does not arrive, and the run under the element re-states the
/// same fact in layout — four Ahem em squares are 64px wide at 16px and
/// 128px wide at 32px.
#[test]
fn the_record_form_of_set_inline_styles_keeps_kebab_case_declarations() {
    let (_js_runtime, _runtime, elements) = text_card_with_author_css(
        "page { font-family: Ahem; font-size: 32px; }",
        r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const el = __CreateElement('view', 0);
                  __SetAttribute(el, 'id', 'test-bulk');
                  __SetInlineStyles(el, {
                    'color': 'red',
                    'font-size': '16px',
                    'margin-top': '10px',
                  });
                  __AppendElement(page, el);

                  // A run inside the styled element, so the size the
                  // declaration carries is asserted where it is consumed and
                  // not only where it is stored.
                  const text = __CreateText(0);
                  __SetID(text, 'probe');
                  __AppendElement(text, __CreateRawText('abcd'));
                  __AppendElement(el, text);
                };
                ",
        "app:///bulk-inline-styles.js",
    );

    let tree = elements.tree();
    let element = element_with_id(&tree, "test-bulk");
    // The contrast itself has to be live, or the 16px below would pass on
    // stylo's default just as it did before: the page really does hand 32px
    // down, so 16px on the element can only have come from the record.
    assert!(
        (font_size(&tree, tree.document_element().id()) - 32.0).abs() < f32::EPSILON,
        "the card's own sheet reached the tree, got {}px on the page",
        font_size(&tree, tree.document_element().id())
    );
    assert_eq!(color(&tree, element), rgb(255, 0, 0));
    assert!(
        (font_size(&tree, element) - 16.0).abs() < f32::EPSILON,
        "16px arrives as 16px, unrewritten, rather than the 32px the page \
         would have handed down, got {}px",
        font_size(&tree, element)
    );
    let probe = element_with_id(&tree, "probe");
    let measured = tree.text_block_size(probe).expect("a committed paragraph");
    assert!(
        (measured.width - 64.0).abs() < f32::EPSILON,
        "four em squares at the declared 16px, got {measured:?}"
    );
    assert!(
        (tree
            .rounded_layout(element)
            .expect("a committed box")
            .location
            .y
            - 10.0)
            .abs()
            < f32::EPSILON,
        "and the kebab-case margin longhand reached layout"
    );
}

/// The `text/dynamic-text-style-update` card, built once and parameterised
/// by the one thing the two replicas below differ in: how a node is given
/// its string. `write_content` is the body of a `setContent(node, string)`
/// arrow function, so everything else — the five nodes, the two classes, the
/// two inline blocks, the flush and the swap — is shared verbatim.
fn dynamic_text_style_update_card(
    write_content: &str,
) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let card = format!(
        r"
            globalThis.renderPage = function () {{
              const setContent = (node, string) => {{ {write_content} }};
              const page = __CreatePage('card', 0);
              // A fifth node, styled by nothing, stands in for the
              // defaults the swapped-away declarations must fall back to.
              const control = __CreateText(0);
              __SetID(control, 'control');
              setContent(control, 'text-grp-0');
              __AppendElement(page, control);

              const names = ['one', 'two', 'three', 'four'];
              const texts = [];
              for (let index = 0; index < names.length; index++) {{
                const text = __CreateText(0);
                __SetID(text, names[index]);
                setContent(text, 'text-grp-' + (index + 1));
                __AppendElement(page, text);
                texts.push(text);
              }}
              __SetClasses(texts[0], 'normal-text');
              __SetClasses(texts[1], 'active-text');
              __SetInlineStyles(texts[2], 'color: skyblue;');
              __SetInlineStyles(texts[3], 'font-size: 32px;');
              __FlushElementTree();

              // What the tap on the card's button does: every one of the
              // four values is swapped for the neighbouring node's.
              __SetClasses(texts[0], 'active-text');
              __SetClasses(texts[1], 'normal-text');
              __SetInlineStyles(texts[2], 'font-size: 32px;');
              __SetInlineStyles(texts[3], 'color: skyblue;');
            }};
            "
    );
    text_card_with_author_css(
        "text { font-family: Ahem; font-size: 20px; }
         .normal-text { color: #add8e6; }
         .active-text { color: red; font-style: italic; }",
        &card,
        "app:///dynamic-text-style-update.js",
    )
}

/// The six cascade facts the swap must produce: each class swap and each
/// inline-style swap *replaces* the block it stands for rather than merging
/// into it, so the declaration the old value carried is gone.
fn assert_dynamic_style_swap_replaced_the_declarations(document: &LynxDocument) {
    let control = element_with_id(document, "control");
    let one = element_with_id(document, "one");
    let two = element_with_id(document, "two");
    let three = element_with_id(document, "three");
    let four = element_with_id(document, "four");

    assert_eq!(
        color(document, one),
        rgb(255, 0, 0),
        "one took the new class"
    );
    assert_eq!(font_style(document, one), FontStyle::ITALIC);
    assert_eq!(
        color(document, two),
        rgb(173, 216, 230),
        "two took the new class"
    );
    assert_eq!(
        font_style(document, two),
        FontStyle::NORMAL,
        "and lost the italic the old class carried, rather than keeping it"
    );

    assert!((font_size(document, three) - 32.0).abs() < f32::EPSILON);
    assert_eq!(
        color(document, three),
        color(document, control),
        "the replaced inline block took the colour with it"
    );
    assert_eq!(color(document, four), rgb(135, 206, 235));
    assert!(
        (font_size(document, four) - font_size(document, control)).abs() < f32::EPSILON,
        "and the replaced inline block took the font size with it"
    );
}

/// Every node carries a ten-character string, so the paragraph each one
/// measures is ten Ahem em squares wide at whatever font size survived.
fn assert_each_node_measures_ten_em_squares(tree: &LynxDocument) {
    for name in ["control", "one", "two", "three", "four"] {
        let element = element_with_id(tree, name);
        let expected = 10.0 * font_size(tree, element);
        let measured = tree
            .text_block_size(element)
            .expect("a committed paragraph");
        assert!(
            (measured.width - expected).abs() < f32::EPSILON,
            "{name}: ten em squares at {}px, got {measured:?}",
            font_size(tree, element)
        );
    }
}

/// Replicates `text/dynamic-text-style-update`
/// (`web-platform/web-tests/dist/basic-element-text-dynamic-text-style-update`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2749`): re-styling live `text`
/// nodes — a class swap through `__SetClasses` and an inline-style swap
/// through `__SetInlineStyles` — fully *replaces* the previous declarations
/// instead of merging with them, and the paragraphs re-measure.
///
/// This is the fixture-faithful half: the card writes each node's content the
/// way the `ReactLynx` compiler collapses a static text child —
/// `__SetAttribute(textElement, 'text', ...)` on the `text` element itself,
/// with no carrier. web-core implements that as a real custom-element
/// behaviour (the `RawTextAttributes` mixin), and this engine reaches the same
/// rendering through `text[text] { content: attr(text); }`
/// (`crates/bobcat-core/src/main/tree/text.rs:126`), so all four nodes render
/// their string and the paragraphs measure.
///
/// Its twin
/// `swapping_a_text_s_class_and_inline_style_replaces_its_declarations_with_carriers`
/// runs the identical card over the other construction — a `raw-text` carrier
/// per node — and makes the same assertions, so the case is pinned on both
/// paths.
#[test]
fn swapping_a_text_s_class_and_inline_style_replaces_its_declarations() {
    let (_js_runtime, _runtime, elements) =
        dynamic_text_style_update_card("__SetAttribute(node, 'text', string);");
    let tree = elements.tree();
    assert_dynamic_style_swap_replaced_the_declarations(&tree);
    assert_each_node_measures_ten_em_squares(&tree);
}

/// The twin of the case above: the same card, the same swap, the same
/// assertions, with each node's string delivered through a `raw-text`
/// carrier instead of through the element's own `text` attribute.
///
/// The two constructions are separate UA rules — `text[text]` at
/// `crates/bobcat-core/src/main/tree/text.rs:126` and `raw-text` at
/// `crates/bobcat-core/src/main/tree/raw_text.rs:10` — so the pair states
/// that the cascade swap is observed through either one.
#[test]
fn swapping_a_text_s_class_and_inline_style_replaces_its_declarations_with_carriers() {
    let (_js_runtime, _runtime, elements) =
        dynamic_text_style_update_card("__AppendElement(node, __CreateRawText(string));");
    let tree = elements.tree();
    assert_dynamic_style_swap_replaced_the_declarations(&tree);
    assert_each_node_measures_ten_em_squares(&tree);
}

/// The `text/maxline-with-setData` card: a `text` carrying
/// `text-maxlength="5"` is flushed while its dynamic child slot is still
/// empty, and the content arrives afterwards, the way a `setData` a second
/// later delivers it. `overflow`, when not empty, is the `text-overflow` the
/// card declares — the fixture itself declares none, and this engine's
/// truncation marker is gated on it.
fn maxlength_with_set_data_card(
    overflow: &str,
) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let overflow = if overflow.is_empty() {
        String::new()
    } else {
        format!(";text-overflow:{overflow}")
    };
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            &format!(
                r"
                globalThis.renderPage = function () {{
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetInlineStyles(text, 'font-family:Ahem;font-size:20px;line-height:21px{overflow}');
                  __SetAttribute(text, 'text-maxlength', '5');
                  __AppendElement(page, text);
                  // The first flush sees the dynamic slot still empty.
                  __FlushElementTree();
                  // The data update a second later fills it.
                  __AppendElement(text, __CreateRawText('123456'));
                }};
                "
            ),
            "app:///maxline-with-set-data.js",
        )
        .expect("main-thread script");
    (js_runtime, runtime, elements)
}

/// The width of the paragraph the card's single `text` establishes.
/// The card's paragraph as `(width, height)`, so a caller can pin both.
fn card_paragraph_size(elements: &DocumentProbe) -> (f32, f32) {
    let tree = elements.tree();
    let text = tree
        .document_element()
        .first_child()
        .expect("the text")
        .id();
    let size = tree.text_block_size(text).expect("a committed paragraph");
    (size.width, size.height)
}

fn card_paragraph_width(elements: &DocumentProbe) -> f32 {
    card_paragraph_size(elements).0
}

/// Replicates `text/maxline-with-setData`
/// (`web-platform/web-tests/dist/basic-element-text-maxline-with-setData`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2783`), the truncation-marker half:
/// what `123456` under `text-maxlength="5"` ends with once the late content
/// has been clamped.
///
/// web-core renders `12345...`: its marker is unconditional, served by
/// `::after { content: "..." }` (`x-text.css:191-194`), with `text-overflow`
/// never consulted for either truncation attribute — so the reference width is
/// eight em squares on a card that declares no `text-overflow` at all. Native
/// Lynx appends it unconditionally too ("Ellipsis will be appended
/// disregarding the overflowing mode.", Android `TextRenderer.java:126-135`).
/// This engine deliberately renders `12345` instead: the marker is gated on
/// `text-overflow: ellipsis` (`crates/hughie/src/text/block/truncate.rs:135`),
/// which this card leaves at its initial `clip`. On `text-maxlength` that
/// gating matches *neither* reference; it is the intended behavior under the
/// 2026-09-14 ruling and is recorded here rather than normalised away.
///
/// The gate's other side is
/// `a_late_maxlength_clamp_ends_in_the_three_dot_tail_under_text_overflow_ellipsis`,
/// which runs the same card with the declaration web-core does not need and
/// does reach eight em squares. The re-clamp itself is a third half of this
/// case: `a_maxlength_clamp_re_applies_when_the_content_arrives_after_a_flush`.
#[test]
fn a_late_maxlength_clamp_ends_bare_where_web_core_ends_in_a_tail() {
    let (_js_runtime, _runtime, elements) = maxlength_with_set_data_card("");
    let width = card_paragraph_width(&elements);
    assert!(
        (width - 100.0).abs() < f32::EPSILON,
        "the five kept units at 20px and no marker after them — web-core's \
         `12345...` would be 160 — got {width}"
    );
}

/// The gated-open counterpart of
/// `a_late_maxlength_clamp_ends_bare_where_web_core_ends_in_a_tail`: the same
/// card with `text-overflow: ellipsis` declared, which is the one state in
/// which this engine emits the marker
/// (`crates/hughie/src/text/block/truncate.rs:135`).
///
/// This reproduces what web-core renders for the fixture unconditionally, and
/// keeps the marker machinery covered from the PAPI side: without it a
/// regression that stopped emitting markers at all would read as the ruled
/// behavior everywhere and go unnoticed.
#[test]
fn a_late_maxlength_clamp_ends_in_the_three_dot_tail_under_text_overflow_ellipsis() {
    let (_js_runtime, _runtime, elements) = maxlength_with_set_data_card("ellipsis");
    let (width, height) = card_paragraph_size(&elements);
    assert!(
        (width - 160.0).abs() < f32::EPSILON,
        "the five kept units plus the three dots the ellipsis appends, at 20px \
         each — the maxlength candidate is never backed off, so the dots are \
         added to the five rather than taken out of them — got {width}"
    );
    // The height the pre-ruling test asserted, kept here: the tail rides the
    // cut line rather than opening a second one.
    assert!(
        (height - 21.0).abs() < f32::EPSILON,
        "one line of the card's 21px line-height — got {height}"
    );
}

/// The same case's re-clamp half: the clamp established at the first flush,
/// when the child slot was empty, still applies to content appended after
/// it.
///
/// Width is the discriminator, not line count — an unclamped `123456` is
/// 120px wide and would still sit on one line. Five kept em squares at 20px
/// is 100px. The clamp is a layout cut rather than an edit, so the carrier
/// still holds the whole string.
#[test]
fn a_maxlength_clamp_re_applies_when_the_content_arrives_after_a_flush() {
    let (_js_runtime, _runtime, elements) = maxlength_with_set_data_card("");
    let tree = elements.tree();
    let text = tree
        .document_element()
        .first_child()
        .expect("the text")
        .id();
    assert_eq!(
        carrier_contents(&tree, text),
        vec!["123456".to_owned()],
        "the clamp cuts the paragraph, it does not rewrite the content"
    );
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.width - 100.0).abs() < f32::EPSILON,
        "five kept em squares at 20px — six would be 120 — got {measured:?}"
    );
    assert!(
        (measured.height - 21.0).abs() < f32::EPSILON,
        "and the clamp keeps it on one line, got {measured:?}"
    );
}

/// Replicates `x-text/event-layoutchange`
/// (`web-elements/tests/fixtures/x-text/event-layoutchange.html`,
/// `web-elements/tests/web-elements.spec.ts:420`): widening a `text` from
/// 100px to 200px dispatches one `layoutchange` whose detail carries the
/// border-box geometry as numbers, plus the element's id.
///
/// The x-view case at `web-elements.spec.ts:105` makes the identical
/// assertion, so a `text` has to take part in the same layout-observation
/// path as every other Lynx box.
///
/// The trailing geometry assertion documents rather than pins: the JS
/// verification throws first while the event is missing, so it is only
/// reached once the gap closes, and it is there to state which box the
/// detail should have described.
#[test]
#[ignore = "GAP: no layout/layoutchange event exists — the only engine-synthesized \
            event names are the pointer set plus `tap` and `longpress`, \
            crates/bobcat-core/src/paint/gesture.rs:81,84"]
fn widening_a_text_dispatches_a_layoutchange_carrying_its_geometry() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.detail = undefined;
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetID(text, 'target');
                  __SetInlineStyles(text, 'width:100px;height:400px');
                  __AppendElement(page, text);
                  // A registration is weak by its handle, so the card holds
                  // the elements its listeners are filed on.
                  globalThis.held = [page, text];
                  __AddEventListener(
                    text,
                    'layoutchange',
                    (event) => { globalThis.detail = event.detail; },
                    {},
                  );
                  __FlushElementTree();
                  // The click handler in the fixture: 100px -> 200px.
                  __SetInlineStyles(text, 'width:200px;height:400px');
                };
                ",
            "app:///event-layoutchange.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                const detail = globalThis.detail;
                if (!detail) throw new Error('no layoutchange was dispatched');
                for (const key of ['width', 'height', 'left', 'right', 'top', 'bottom']) {
                  if (typeof detail[key] !== 'number') {
                    throw new Error(key + ' is a ' + typeof detail[key]);
                  }
                }
                if (detail.id !== 'target') throw new Error('id is ' + detail.id);
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    // The geometry the detail should have carried is the box's own, and the
    // host already has it: what is missing is the observation, not the data.
    let tree = elements.tree();
    let target = element_with_id(&tree, "target");
    let box_ = tree.rounded_layout(target).expect("a committed box");
    assert!(
        (box_.size.width - 200.0).abs() < f32::EPSILON
            && (box_.size.height - 400.0).abs() < f32::EPSILON,
        "the widened box, got {:?}",
        box_.size
    );
}

/// Replicates `text/bindlayout`
/// (`web-platform/web-tests/dist/basic-element-text-bindlayout`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2775`): a `text` that wraps reports
/// its line count and, per line, the source range it covers and how many
/// ellipsis units it ends with.
///
/// The source test is permanently skipped upstream (`test.skip(true, 'the
/// text layout event should be improved')`) and has no golden, so the
/// expectation here is stated from the payload the handler destructures:
/// `detail.lineCount` and `detail.lines[i].{start, end, ellipsisCount}`. The
/// card registers through `__AddEvent(el, 'bindEvent', 'layout', ...)`; the
/// replica registers the same name through `__AddEventListener`, which needs
/// no worklet runtime to deliver to.
#[test]
#[ignore = "GAP: no layout event delivery, and the payload has no host-visible \
            query — LineInfo is computed at crates/hughie/src/text/block/mod.rs:66-83 \
            but Document::text_block is pub(crate), crates/dom/src/layout/mod.rs:303"]
fn a_wrapping_text_reports_its_line_count_and_line_ranges() {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.detail = undefined;
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  // Five em squares of room, eleven units of content: the
                  // break falls at the space, so 'abcde ' and then 'fghij'.
                  __SetInlineStyles(
                    text,
                    'font-family:Ahem;font-size:20px;line-height:21px;width:100px',
                  );
                  __AppendElement(text, __CreateRawText('abcde fghij'));
                  __AppendElement(page, text);
                  globalThis.held = [page, text];
                  __AddEventListener(
                    text,
                    'layout',
                    (event) => { globalThis.detail = event.detail; },
                    {},
                  );
                };
                ",
            "app:///bindlayout.js",
        )
        .expect("main-thread script");

    runtime
        .evaluate_module(
            &mut js_runtime,
            r"
                const detail = globalThis.detail;
                if (!detail) throw new Error('no layout event was dispatched');
                if (detail.lineCount !== 2) throw new Error('lineCount ' + detail.lineCount);
                const second = detail.lines[1];
                if (second.start !== 6 || second.end !== 11 || second.ellipsisCount !== 0) {
                  throw new Error('lines[1] ' + JSON.stringify(second));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

    // The wrap the payload describes is real: two 21px lines.
    let tree = elements.tree();
    let text = tree
        .document_element()
        .first_child()
        .expect("the text")
        .id();
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.height - 42.0).abs() < f32::EPSILON,
        "two lines of 21px, got {measured:?}"
    );
}

/// The compiled `countdown` card the `text/set-native-props-*` fixtures share,
/// with the parts only a screenshot needs left out: one `.countdown__num--h`
/// `text` holding mixed children — a `raw-text` run and two nested inline
/// `text` elements carrying their strings as `text` attributes, exactly as the
/// snapshots build them. Every string is Ahem em squares at 10px, so the
/// paragraph's advance counts characters. `leading` writes the `raw-text` the
/// fixtures put in front of the nested texts, and `limit` the `text-maxlength`
/// one of them declares. The `__SetID` is the test's own handle on the node;
/// the selector the card is about is the class one the query uses.
fn countdown_card(leading: bool, limit: &str) -> String {
    let maxlength = if limit.is_empty() {
        String::new()
    } else {
        format!("__SetAttribute(num, 'text-maxlength', '{limit}');")
    };
    let leading = if leading {
        "__AppendElement(num, __CreateRawText('--'));"
    } else {
        ""
    };
    format!(
        r"
        globalThis.renderPage = function () {{
          const page = __CreatePage('card', 0);
          const block = __CreateView(0);
          __SetClasses(block, 'countdown__block');
          __AppendElement(page, block);

          const num = __CreateText(0);
          __SetID(num, 'target');
          __SetClasses(num, 'countdown__num countdown__num--h');
          __SetInlineStyles(num, 'font-family:Ahem;font-size:10px');
          {maxlength}
          __AppendElement(block, num);
          {leading}
          const hello = __CreateText(0);
          __SetAttribute(hello, 'text', 'hello');
          __AppendElement(num, hello);
          __AppendElement(num, __CreateRawText('--'));
          const world = __CreateText(0);
          __SetAttribute(world, 'text', 'world');
          __AppendElement(num, world);
          __FlushElementTree();
        }};
        "
    )
}

/// What the card's `useEffect` does a tick after mount, minus the timer: one
/// `setNativeProps({text})` through a background-thread `SelectorQuery`.
const COUNTDOWN_PUSH: &str = r"
    lynx.createSelectorQuery()
      .select('.countdown__num--h')
      .setNativeProps({text: 'the count is:1'})
      .exec();
    ";

/// Replicates `text/set-native-props-text`
/// (`web-tests/dist/basic-element-text-set-native-props-text/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2792`): a `setNativeProps({text})`
/// pushed from the background thread onto a `text` whose first element child is
/// a `raw-text` replaces *that run* and leaves the rest of the paragraph
/// standing.
///
/// The original asserts DOM text counts — exactly one node reading
/// `the count is:1` — which is a claim about where the pushed string went, not
/// only that it arrived. web-core's own handler is where the answer is: for a
/// `text` prop on an `X-TEXT` it retargets to the host's first `RAW-TEXT`
/// element child before writing
/// (`web-core/ts/client/mainthread/crossThreadHandlers/registerSetNativePropsHandler.ts:13-20`),
/// and `RawTextAttributes` then swaps that carrier's text node for the new
/// string (`web-elements/src/elements/XText/RawText.ts:19-26`). So the
/// paragraph reads `the count is:1` + `hello` + `--` + `world`: twenty-six em
/// squares, with the pushed string standing exactly once and in first place.
///
/// Its sibling `a_native_props_text_push_appends_where_there_is_no_leading_run`
/// is the same push onto the same card without that leading carrier, which is
/// the other branch of the same handler.
#[test]
#[ignore = "GAP (two of them). The push is not retargeted onto a leading \
            `raw-text` child: packages/bobcat-element/src/element-papi.ts:1226-1235 \
            routes every prop to `__SetAttribute` on the selected element \
            itself, where web-core hands a `text` prop to the host's first \
            `raw-text` element child instead \
            (web-core/ts/client/mainthread/crossThreadHandlers/registerSetNativePropsHandler.ts:13-20). \
            And the attribute it writes instead REPLACES the element's \
            children: `text[text] { content: attr(text) }` \
            (crates/bobcat-core/src/main/tree/text.rs:126) is a CSS content \
            list, and a content list replaces rendered children \
            (crates/dom/src/layout/text_block.rs:246-248), where web-core's \
            `RawTextAttributes` appends the attribute's text node after them \
            (web-elements/src/elements/XText/RawText.ts:19-26)"]
fn a_native_props_text_push_replaces_the_leading_raw_text_run() {
    let mut pair = background_pair(&countdown_card(true, ""), COUNTDOWN_PUSH);
    pair.deliver();

    let tree = pair.tree();
    let target = element_with_id(&tree, "target");
    assert_eq!(
        carrier_contents(&tree, target),
        vec!["the count is:1".to_owned(), "--".to_owned()],
        "the push landed on the leading carrier — the card still holds two of \
         them, and the second is untouched"
    );
    let measured = tree.text_block_size(target).expect("a committed paragraph");
    assert!(
        (measured.width - 260.0).abs() < f32::EPSILON,
        "twenty-six em squares at 10px: the pushed string, then the `hello`, \
         `--` and `world` it did not disturb, got {measured:?}"
    );
}

/// Replicates `text/set-native-props-with-maxlength`
/// (`web-tests/dist/basic-element-text-set-native-props-with-maxlength/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2802`): the same push onto a target
/// that already carries `text-maxlength="5"`, so the clamp applies to content
/// that arrived through the query rather than through a render.
///
/// The pushed string becomes the paragraph's leading run, so the five
/// characters the clamp keeps are `the c`.
///
/// web-core follows them with a marker — `12345...`-style, from
/// `x-text[text-maxlength]::part(inner-box)::after`'s outright
/// `content: "..."` (`x-text.css:191-194`), with `text-overflow` consulted for
/// neither truncation attribute — and so does native Lynx ("Ellipsis will be
/// appended disregarding the overflowing mode.", Android
/// `TextRenderer.java:126-135`). This engine deliberately does not: the marker
/// is gated on `text-overflow: ellipsis`
/// (`crates/hughie/src/text/block/truncate.rs:135`), which this card leaves at
/// its initial `clip`. On `text-maxlength` that gating matches *neither*
/// reference — it is a Lynx-vello-specific behavior, intended under the
/// 2026-09-14 ruling — so the width asserted here is web-core's eight em
/// squares minus its three dots: `the c` alone, five at 10px. Nothing is
/// backed off to make room for a marker that is not emitted, so all five
/// characters the limit allows survive.
///
/// The marker is therefore no longer among this case's gaps. The two that
/// remain are its sibling
/// `a_native_props_text_push_replaces_the_leading_raw_text_run`'s, and they
/// are why this stays ignored: the push never reaches the leading carrier, so
/// the first assertion below is the one that fails.
#[test]
#[ignore = "GAP (two of them). The push is not retargeted onto a leading \
            `raw-text` child (packages/bobcat-element/src/element-papi.ts:1226-1235 \
            against web-core's \
            web-core/ts/client/mainthread/crossThreadHandlers/registerSetNativePropsHandler.ts:13-20), \
            and the `text` attribute it writes instead replaces the element's \
            children rather than appending to them \
            (`text[text] { content: attr(text) }`, \
            crates/bobcat-core/src/main/tree/text.rs:126, through \
            crates/dom/src/layout/text_block.rs:246-248)"]
fn a_maxlength_clamps_the_text_a_native_props_push_delivers() {
    let mut pair = background_pair(&countdown_card(true, "5"), COUNTDOWN_PUSH);
    pair.deliver();

    let tree = pair.tree();
    let target = element_with_id(&tree, "target");
    assert_eq!(
        carrier_contents(&tree, target),
        vec!["the count is:1".to_owned(), "--".to_owned()],
        "the clamp cuts the paragraph, so the carrier still holds the whole \
         pushed string"
    );
    let measured = tree.text_block_size(target).expect("a committed paragraph");
    assert!(
        (measured.width - 50.0).abs() < f32::EPSILON,
        "the five kept characters — `the c` — at 10px each, with no marker \
         after them, got {measured:?}"
    );
}

/// Replicates `text/set-native-props-text-do-not-change-inline-text`
/// (`web-tests/dist/basic-element-text-set-native-props-text-do-not-change-inline-text/index.web.
/// json`, `web-core-e2e/tests/reactlynx.spec.ts:2815`): the same push onto a target
/// whose first element child is a nested inline `text` rather than a
/// `raw-text`. The original asserts that `hello`, `--` and `world` each still
/// appear exactly once afterwards — the push must not clobber the children it
/// found.
///
/// This is the other branch of web-core's handler: with no leading `raw-text`
/// to retarget to, the `text` prop is written on the host itself
/// (`registerSetNativePropsHandler.ts:13-20`), and `RawTextAttributes` appends
/// a text node for it after the existing children
/// (`web-elements/src/elements/XText/RawText.ts:19-26`). The paragraph reads
/// `hello` + `--` + `world` + `the count is:1`: twenty-six em squares, the
/// pushed string last.
///
/// So this case isolates the second of the two defects its sibling
/// `a_native_props_text_push_replaces_the_leading_raw_text_run` carries. No
/// retargeting is involved here — both engines write the attribute on the
/// element itself — and what is left is what the attribute then does to the
/// children.
#[test]
#[ignore = "GAP: a `text` attribute REPLACES the element's children here. \
            `text[text] { content: attr(text) }` \
            (crates/bobcat-core/src/main/tree/text.rs:126) is a CSS content \
            list, and a content list replaces rendered children \
            (crates/dom/src/layout/text_block.rs:246-248), so the push leaves \
            the paragraph holding nothing but the pushed string — where \
            web-core's `RawTextAttributes` appends the attribute's text node \
            after the children it found \
            (web-elements/src/elements/XText/RawText.ts:19-26)"]
fn a_native_props_text_push_appends_where_there_is_no_leading_run() {
    let mut pair = background_pair(&countdown_card(false, ""), COUNTDOWN_PUSH);
    pair.deliver();

    let tree = pair.tree();
    let target = element_with_id(&tree, "target");
    assert_eq!(
        tree.get(target).and_then(|node| node.attribute("text")),
        Some("the count is:1"),
        "with no leading carrier to retarget to, the push is written on the \
         element itself"
    );
    assert_eq!(
        carrier_contents(&tree, target),
        vec!["--".to_owned()],
        "and it becomes no carrier of its own: the one the card wrote is still \
         the only one"
    );
    let measured = tree.text_block_size(target).expect("a committed paragraph");
    assert!(
        (measured.width - 260.0).abs() < f32::EPSILON,
        "twenty-six em squares at 10px: `hello`, `--` and `world` survive the \
         push, and the pushed string follows them, got {measured:?}"
    );
}

/// Replicates `text/set-native-props-with-setData`
/// (`web-tests/dist/basic-element-text-set-native-props-with-setData/index.web.json`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2831`): `setNativeProps({text})` and
/// ordinary data updates, interleaved on one `text`. The card's paragraph is a
/// leading `raw-text`, two dynamic slots built as wrapper elements, and a
/// static nested `text` between them; four taps drive push, update, push,
/// update, and the original counts DOM text after each one — after the first
/// push the leading `--` is gone, and each pushed string stands exactly once
/// while the re-rendered slots keep changing under it.
///
/// The taps are dropped and their four actions kept in order: the background
/// thread pushes, then asks the main thread for the update a `setState`
/// performs (both directions travel the one FIFO, so the order is the card's),
/// then pushes again, then asks for the second update. A compiled update slot
/// rewrites a dynamic string child by writing the carrier's `text` attribute,
/// which is what the main-thread listener does here.
///
/// Because each push retargets onto the leading carrier, the final paragraph
/// reads `2ndNative` + `world` + `text` + `world` — twenty-three em squares at
/// 10px — and the `--` the card started with is gone after the first push,
/// which is the fixture's own count assertion.
#[test]
#[ignore = "GAP (two of them). The push is not retargeted onto the leading \
            `raw-text` child: packages/bobcat-element/src/element-papi.ts:1226-1235 \
            routes every prop to `__SetAttribute` on the selected element \
            itself, where web-core hands a `text` prop to the host's first \
            `raw-text` element child \
            (web-core/ts/client/mainthread/crossThreadHandlers/registerSetNativePropsHandler.ts:13-20). \
            And the attribute it writes instead replaces the element's \
            children — `text[text] { content: attr(text) }` \
            (crates/bobcat-core/src/main/tree/text.rs:126) through \
            crates/dom/src/layout/text_block.rs:246-248 — so the re-rendered \
            slots stop rendering at all, where web-core appends the pushed \
            string after them \
            (web-elements/src/elements/XText/RawText.ts:19-26)"]
fn a_native_props_push_survives_the_data_updates_interleaved_with_it() {
    let mut pair = background_pair(
        r"
        globalThis.renderPage = function () {
          const page = __CreatePage('card', 0);
          const container = __CreateText(0);
          __SetID(container, 'container');
          __SetClasses(container, 'container');
          __SetInlineStyles(container, 'font-family:Ahem;font-size:10px');
          __AppendElement(page, container);

          __AppendElement(container, __CreateRawText('--'));
          const first = __CreateRawText('initial');
          const slot0 = __CreateWrapperElement(0);
          __AppendElement(slot0, first);
          __AppendElement(container, slot0);
          const label = __CreateText(0);
          __SetAttribute(label, 'text', 'text');
          __AppendElement(container, label);
          const second = __CreateRawText('initial');
          const slot1 = __CreateWrapperElement(0);
          __AppendElement(slot1, second);
          __AppendElement(container, slot1);
          __FlushElementTree();

          // What a `setState` does to this card: both dynamic slots are
          // rewritten through the call a compiled update slot emits, and
          // nothing else in the tree is touched.
          lynx.getJSContext().addEventListener('setData', (event) => {
            __SetAttribute(first, 'text', event.data);
            __SetAttribute(second, 'text', event.data);
            __FlushElementTree();
          });
        };
        ",
        r"
        const query = lynx.createSelectorQuery();
        const core = lynx.getCoreContext();
        query.select('.container').setNativeProps({text: 'nativeText'}).exec();
        core.dispatchEvent({type: 'setData', data: 'hello'});
        query.select('.container').setNativeProps({text: '2ndNative'}).exec();
        core.dispatchEvent({type: 'setData', data: 'world'});
        ",
    );
    for _ in 0..4 {
        pair.deliver();
    }

    let tree = pair.tree();
    let container = element_with_id(&tree, "container");
    assert_eq!(
        carrier_contents(&tree, container),
        vec![
            "2ndNative".to_owned(),
            "world".to_owned(),
            "world".to_owned()
        ],
        "the second push replaced the leading carrier the first push had \
         already written, and the two data updates reached the slots without \
         disturbing it"
    );
    let measured = tree
        .text_block_size(container)
        .expect("a committed paragraph");
    assert!(
        (measured.width - 230.0).abs() < f32::EPSILON,
        "twenty-three em squares at 10px: the pushed string, both re-rendered \
         slots and the static run between them, got {measured:?}"
    );
}
