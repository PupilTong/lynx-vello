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
//! Geometry is measured with the vendored Ahem face, whose glyphs are solid
//! em squares, so a run's advance is exactly its glyph count times its font
//! size and every metric below is an exact number rather than a tolerance.
//!
//! # The cases that are not replicated here
//!
//! Six of the assigned cases have no replica, and all six for one reason:
//! the member the case drives is not a global at all. `text/set-native-props-*`
//! (four cases) call `__SetNativeProps`, `reactlynx/api-SelectorQuery` calls
//! `createSelectorQuery`, and `web-core-e2e/web-core/add-class-css-og-style-font-size`
//! calls `__AddClass`; the dataset members, selector querying and `__AddClass`
//! are on this runtime's not-implemented list by name
//! (`packages/bobcat-element/src/element-papi.mjs:72-78`), and nothing else
//! defines them. A replica would die on a `ReferenceError` at its first line
//! and would therefore pin the absence of a *name*, not any text behaviour —
//! it could not state what web-core renders, so it could not be strengthened
//! into a reference assertion later.
//!
//! That is what separates them from `x-text/event-layoutchange` and
//! `text/bindlayout`, which the assignment files under the same
//! `blocked-by-engine-gap` verdict but which *are* written as `#[ignore]`d
//! replicas below: every member those two call (`__AddEventListener`,
//! `__SetInlineStyles`, `__CreateRawText`) exists, so the replica runs the
//! whole card, states the reference payload, and fails on the missing
//! behaviour rather than on a missing identifier.

use dom::stylo::color::AbsoluteColor;
use dom::stylo::values::computed::FontStyle;
use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerCommand, WorkerEvent};
use crate::link::detached_outbox;
use crate::main::tree::{PageConfig, Viewport};
use crate::main::workers::WorkerFactory;
use crate::resource::StyleSheetSource;
use crate::view::NoWakeup;

/// A same-thread window onto the realm-owned document, so a replica can read
/// back what script built without going through the runtime's own methods —
/// the stand-in for the `rootDom.querySelector` the ported tests use.
struct DocumentProbe {
    slot: Rc<RefCell<DocumentSlot>>,
    // These replicas exercise only MTS. Keep the worker boundary's far ends
    // open so the realm's own boot succeeds.
    _workers: mpsc::UnboundedReceiver<WorkerCommand>,
    _worker_events: mpsc::UnboundedReceiver<WorkerEvent>,
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
    text_runtime_with_author_css("")
}

/// The same, plus the card's own stylesheet — the `styleInfo` half of a
/// bundle, which the Element PAPI never carries. It is staged the way a
/// view's fetched sheets are, because the document does not exist until the
/// boot module creates it.
fn text_runtime_with_author_css(css: &str) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

    let mut text = dom::TextContext::new();
    assert_eq!(text.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
    let mut ingredients =
        DocumentIngredients::for_test(Viewport::new(393.0, 727.0), PageConfig::default());
    ingredients.text_context = Some(text);
    if !css.is_empty() {
        ingredients.sheets.push(StyleSheetSource::Text(css.into()));
    }
    runtime_over(ingredients)
}

fn runtime_over(
    ingredients: DocumentIngredients,
) -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let (outbox, _far_end) = detached_outbox(Arc::new(NoWakeup));
    let mut js_runtime = ScriptRuntime::new().expect("the test runtime starts");
    install_shared_modules(&mut js_runtime).expect("the shared modules register");
    let (workers, inbox) = mpsc::unbounded_channel();
    let (runtime, worker_events) = MainThreadRuntime::new(
        &mut js_runtime,
        ingredients,
        outbox,
        &WorkerFactory::new(workers),
        "app:///main.js",
        None,
        PageData::default(),
    )
    .expect("main-thread runtime");
    let probe = DocumentProbe {
        slot: Rc::clone(&runtime.slot),
        _workers: inbox,
        _worker_events: worker_events,
    };
    (js_runtime, runtime, probe)
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
/// The source tags the element with `__AddDataset(text0, 'testid', ...)`.
/// The dataset members are deliberately not implemented here
/// (`packages/bobcat-element/src/element-papi.mjs:72-78`), so the replica
/// tags it with `__SetID` instead; what is under test is the content read,
/// not the tagging member.
///
/// Be honest about what that substitution costs. The source case's
/// distinguishing half is the *dataset* lookup — `[data-testid=…]` — and once
/// the tag becomes an id this replica is structurally the multiple-levels case
/// below (`a_carrier_is_reachable_as_a_descendant_of_the_id_d_text`) with a
/// different content string; the two pin the same property. The dataset lookup
/// itself is therefore unreplicated, and it is a gap rather than an omission:
/// no member writes a dataset entry, so nothing can carry the attribute a
/// `[data-*]` selector would match. It is not written as an `#[ignore]`d
/// replica for the reason the module header gives — the script would die on a
/// `ReferenceError` at `__AddDataset` and would state nothing about text.
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
    let (mut js_runtime, mut runtime, elements) =
        text_runtime_with_author_css("page { font-family: Ahem; font-size: 32px; }");
    runtime
        .run_main_thread_script(
            &mut js_runtime,
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
        )
        .expect("main-thread script");

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
    let (mut js_runtime, mut runtime, elements) = text_runtime_with_author_css(
        "text { font-family: Ahem; font-size: 20px; }
         .normal-text { color: #add8e6; }
         .active-text { color: red; font-style: italic; }",
    );
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
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            &card,
            "app:///dynamic-text-style-update.js",
        )
        .expect("main-thread script");
    (js_runtime, runtime, elements)
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
/// (`crates/bobcat-core/src/main/tree/text.rs:95`), so all four nodes render
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
/// `crates/bobcat-core/src/main/tree/text.rs:95` and `raw-text` at
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
/// later delivers it.
fn maxlength_with_set_data_card() -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let (mut js_runtime, mut runtime, elements) = text_runtime();
    runtime
        .run_main_thread_script(
            &mut js_runtime,
            r"
                globalThis.renderPage = function () {
                  const page = __CreatePage('card', 0);
                  const text = __CreateText(0);
                  __SetInlineStyles(text, 'font-family:Ahem;font-size:20px;line-height:21px');
                  __SetAttribute(text, 'text-maxlength', '5');
                  __AppendElement(page, text);
                  // The first flush sees the dynamic slot still empty.
                  __FlushElementTree();
                  // The data update a second later fills it.
                  __AppendElement(text, __CreateRawText('123456'));
                };
                ",
            "app:///maxline-with-set-data.js",
        )
        .expect("main-thread script");
    (js_runtime, runtime, elements)
}

/// Replicates `text/maxline-with-setData`
/// (`web-platform/web-tests/dist/basic-element-text-maxline-with-setData`,
/// `web-core-e2e/tests/reactlynx.spec.ts:2783`), the truncation-marker half:
/// `123456` under `text-maxlength="5"` becomes five kept units plus the
/// marker.
///
/// The marker is unconditional in web-core: `text-maxlength` is served by
/// `::after { content: "..." }` in `x-text.css:179-182`, and `text-overflow`
/// is never consulted for either truncation attribute. So the reference
/// width is eight em squares — the five kept characters and three dots — on
/// a card that declares no `text-overflow` at all.
///
/// The re-clamp itself is the other half of this case and is *not* behind
/// this ignore: `a_maxlength_clamp_re_applies_when_the_content_arrives_after_a_flush`
/// runs the identical card and asserts the five-unit cut on one line. The
/// tail is deliberately left out of that width rather than folded into it.
#[test]
#[ignore = "GAP: the truncation marker is gated on `TextOverflow::Ellipsis`, whose \
            initial value is `clip` and which the Lynx UA sheet never declares — \
            crates/hughie/src/text/block/truncate.rs:127-146"]
fn a_late_maxlength_clamp_ends_in_the_three_dot_tail() {
    let (_js_runtime, _runtime, elements) = maxlength_with_set_data_card();
    let tree = elements.tree();
    let text = tree
        .document_element()
        .first_child()
        .expect("the text")
        .id();
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.width - 160.0).abs() < f32::EPSILON,
        "five kept units plus the three-dot tail, at 20px each, got {measured:?}"
    );
    assert!(
        (measured.height - 21.0).abs() < f32::EPSILON,
        "and the clamp keeps it on one line, got {measured:?}"
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
    let (_js_runtime, _runtime, elements) = maxlength_with_set_data_card();
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
            but Document::text_block is pub(crate), crates/dom/src/layout/mod.rs:252"]
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
