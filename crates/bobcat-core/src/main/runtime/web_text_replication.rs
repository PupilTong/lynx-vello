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
//! (`crates/seldon/src/text/block/truncate.rs`), which neither the initial
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
//! `crates/terminus`) and the 8px-on-10px row geometry (in the tree replicas),
//! neither of which belongs here.
//!
//! `web-core-e2e/web-core/add-class-css-og-style-font-size` calls `__AddClass`,
//! which is not implemented and is not a global (`element-papi.ts:81-88`
//! names it among the members a bundle fails at). A replica would die on a
//! `ReferenceError` at its first line and would pin the absence of a *name*,
//! not any text behaviour — it could not state what web-core renders, so it
//! could not be strengthened into a reference assertion later.
//!
//! That is what separates it from `x-text/event-layoutchange` and
//! `text/bindlayout`, which the assignment files under the same
//! `blocked-by-engine-gap` verdict but which *are* written as replicas below:
//! every member those call exists, so the replica runs the whole card and
//! states the reference. Both pass, over the `layoutchange` resize delivery and
//! the `text` `layout` component event (`crate::main::layout_change`).
//!
//! The four `text/set-native-props-*` cases are not replicated: they are about
//! web-core retargeting a `setNativeProps({text})` push onto a text's leading
//! `raw-text`, a behaviour this engine does not support (user, 2026-10-21),
//! and their replicas were deleted with the e2e cards.

use terminus::link::stylo::color::AbsoluteColor;
use terminus::link::stylo::values::computed::FontStyle;
use tokio::sync::mpsc;

use super::*;
use crate::background::{WorkerCommand, WorkerEvent};
use crate::esm::build_runtime;
use crate::jobs::JsThread;
use crate::link::detached_outbox;
use crate::main::tree::{PageConfig, Viewport};
use crate::main::workers::WorkerFactory;
use crate::resource::{LoadedSource, StyleSheetSource};
use crate::view::NoWakeup;

const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");

struct DocumentProbe {
    slot: Rc<RefCell<DocumentSlot>>,
    _workers: mpsc::UnboundedReceiver<WorkerCommand>,
    _worker_events: mpsc::UnboundedReceiver<WorkerEvent>,
    _thread: Rc<JsThread>,
}

impl DocumentProbe {
    fn tree(&self) -> RefMut<'_, LynxDocument> {
        RefMut::filter_map(self.slot.borrow_mut(), |slot| slot.document.as_mut())
            .ok()
            .expect("the realm has created its document")
    }
}

fn text_runtime() -> (ScriptRuntime, MainThreadRuntime, DocumentProbe) {
    let mut text = terminus::link::TextContext::new();
    assert_eq!(
        text.register_fonts(terminus::link::FontBlob::from_static(AHEM)),
        1
    );
    let mut ingredients =
        DocumentIngredients::for_test(Viewport::new(393.0, 727.0), PageConfig::default());
    assert!(text.set_locale("en-US"));
    ingredients.text_context = text;
    runtime_over(ingredients)
}

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
    let mut js_runtime = build_runtime().expect("the test runtime builds");
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

fn descendants(
    tree: &LynxDocument,
    root: terminus::link::NodeId,
    out: &mut Vec<terminus::link::NodeId>,
) {
    let children: Vec<terminus::link::NodeId> = match tree.get(root) {
        Some(node) => node.children().map(terminus::link::Node::id).collect(),
        None => return,
    };
    for child in children {
        out.push(child);
        descendants(tree, child, &mut *out);
    }
}

fn carrier_contents(tree: &LynxDocument, root: terminus::link::NodeId) -> Vec<String> {
    let mut ids = Vec::new();
    descendants(tree, root, &mut ids);
    ids.into_iter()
        .filter(|id| tree.get(*id).and_then(terminus::link::Node::tag_name) == Some("raw-text"))
        .map(|id| {
            tree.get(id)
                .and_then(|node| node.attribute("text"))
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default()
        })
        .collect()
}

fn element_with_id(tree: &LynxDocument, id: &str) -> terminus::link::NodeId {
    let page = tree.document_element().id();
    let mut ids = vec![page];
    descendants(tree, page, &mut ids);
    ids.into_iter()
        .find(|node| {
            tree.get(*node)
                .and_then(|node| node.attribute("id"))
                .as_deref()
                == Some(id)
        })
        .unwrap_or_else(|| panic!("no element carries the id {id:?}"))
}

fn color(tree: &LynxDocument, element: terminus::link::NodeId) -> AbsoluteColor {
    computed(tree, element).get_color().solid_color()
}

fn font_size(tree: &LynxDocument, element: terminus::link::NodeId) -> f32 {
    computed(tree, element)
        .slow_clone_font_size()
        .computed_size()
        .px()
}

fn font_style(tree: &LynxDocument, element: terminus::link::NodeId) -> FontStyle {
    *computed(tree, element).get_font_style()
}

fn computed(
    tree: &LynxDocument,
    element: terminus::link::NodeId,
) -> terminus::link::stylo::servo_arc::Arc<terminus::link::stylo::properties::ComputedValues> {
    tree.get(element)
        .expect("a live element")
        .computed_style()
        .expect("a flushed element has computed style")
}

fn rgb(red: u8, green: u8, blue: u8) -> AbsoluteColor {
    AbsoluteColor::srgb_legacy(red, green, blue, 1.0)
}

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
            .and_then(terminus::link::Node::tag_name),
        Some("text"),
        "and the host tree agrees with the tag script was handed"
    );
}

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
        .and_then(terminus::link::Node::first_child)
        .expect("the text")
        .id();
    assert_eq!(
        tree.get(text).and_then(terminus::link::Node::tag_name),
        Some("text")
    );
    assert_eq!(
        carrier_contents(&tree, text),
        vec!["Text Element".to_owned()],
        "one carrier, holding the whole string"
    );
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.width - 240.0).abs() < f32::EPSILON,
        "twelve em squares at 20px, got {measured:?}"
    );
}

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
        .and_then(terminus::link::Node::first_child)
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
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.width - 220.0).abs() < f32::EPSILON,
        "eleven em squares at 20px, got {measured:?}"
    );
}

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
        tree.get(text)
            .and_then(terminus::link::Node::first_child)
            .is_none(),
        "no substitute child stands in for the missing run"
    );
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        measured.width.abs() < f32::EPSILON && measured.height.abs() < f32::EPSILON,
        "an empty paragraph claims no line box, got {measured:?}"
    );
}

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
                  __AddDataset(text0, 'testid', 'count-value');
                };
                ",
            "app:///text-content-positive.js",
        )
        .expect("main-thread script");

    let tree = elements.tree();
    let page = tree.document_element().id();
    let text = tree
        .query_selector(page, r#"[data-testid="count-value"]"#)
        .expect("the selector parses")
        .expect("the tagged text is found by its dataset entry");
    assert_eq!(
        tree.get(text).and_then(terminus::link::Node::tag_name),
        Some("text")
    );
    assert_eq!(carrier_contents(&tree, text), vec!["2".to_owned()]);
}

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

#[test]
fn swapping_a_text_s_class_and_inline_style_replaces_its_declarations() {
    let (_js_runtime, _runtime, elements) =
        dynamic_text_style_update_card("__SetAttribute(node, 'text', string);");
    let tree = elements.tree();
    assert_dynamic_style_swap_replaced_the_declarations(&tree);
    assert_each_node_measures_ten_em_squares(&tree);
}

#[test]
fn swapping_a_text_s_class_and_inline_style_replaces_its_declarations_with_carriers() {
    let (_js_runtime, _runtime, elements) =
        dynamic_text_style_update_card("__AppendElement(node, __CreateRawText(string));");
    let tree = elements.tree();
    assert_dynamic_style_swap_replaced_the_declarations(&tree);
    assert_each_node_measures_ten_em_squares(&tree);
}

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
    assert!(
        (height - 21.0).abs() < f32::EPSILON,
        "one line of the card's 21px line-height — got {height}"
    );
}

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

#[test]
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

    assert!(
        runtime.update_resize_observations(),
        "the registration's first observation is queued"
    );
    let failures = runtime.notify_resize_observers(&mut js_runtime);
    assert!(failures.is_empty(), "no listener threw");
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
                if (JSON.stringify(detail.dataset) !== '{}') {
                  throw new Error('dataset is ' + JSON.stringify(detail.dataset));
                }
                if (detail.width !== 200 || detail.height !== 400) {
                  throw new Error('size ' + JSON.stringify(detail));
                }
                if (detail.right !== detail.left + 200 || detail.bottom !== detail.top + 400) {
                  throw new Error('edges ' + JSON.stringify(detail));
                }
                ",
            "app:///verify.js",
            "verifying",
        )
        .expect("verification");

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

#[test]
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

    runtime.queue_text_layout_events();
    let failures = runtime.dispatch_component_events(&mut js_runtime);
    assert!(failures.is_empty(), "no listener threw");
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
                const first = detail.lines[0];
                if (first.start !== 0 || first.ellipsisCount !== 0) {
                  throw new Error('lines[0] ' + JSON.stringify(first));
                }
                if (detail.size.width !== 100 || detail.size.height !== 42) {
                  throw new Error('size ' + JSON.stringify(detail.size));
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
    let measured = tree.text_block_size(text).expect("a committed paragraph");
    assert!(
        (measured.height - 42.0).abs() < f32::EPSILON,
        "two lines of 21px, got {measured:?}"
    );
}
