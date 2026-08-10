#![cfg(feature = "quickjs")]

//! web-core's Element-PAPI test suite, ported to run against this engine's
//! real PAPI globals from JavaScript.
//!
//! # Provenance
//!
//! - `lynx-stack/packages/web-platform/web-core/tests/element-apis.spec.ts` — 87 cases.
//! - `lynx-stack/packages/web-platform/web-core/tests/testing-library-port.spec.ts` — 15 cases.
//!
//! web-core's own suite calls `mtsGlobalThis.__CreateView(0)` and friends on a
//! main-thread global object built by `createElementAPI`. The harness here is
//! the same shape one layer down: a [`MainThreadRuntime`] installs the PAPI on
//! a `QuickJS` realm's global object, the case body becomes
//! `globalThis.renderPage`, and `run_main_thread_script` drives web-core's
//! post-evaluation sequence (`processData` → `renderPage` →
//! `__FlushElementTree`). Assertions are made from JavaScript where web-core
//! makes them, and additionally from Rust through
//! [`SharedTree::tree`](bobcat_core::engine::SharedTree::tree) wherever the
//! runtime state is observable there.
//!
//! Conventions follow `tests/main_thread.rs`: the same `VIEWPORT`, the same
//! `SharedTree`/`MainThreadRuntime` composition, scripts as raw-string JS.
//!
//! # Skipped-case ledger
//!
//! Every case of the two suites that is *not* ported, with its web-core name,
//! its line in the file named above, and the reason:
//!
//! - **(b) DOM artifact** — asserts an HTML-DOM detail with no counterpart here: custom-element tag
//!   mapping (`x-view`/`x-input`/…), HTML string serialization, CSS custom-property unit rewriting,
//!   `getComputedStyle` strings, SSR output, or a wasm binding callback.
//! - **(c) member not implemented** — see `lynx-element`'s crate docs for the families this subset
//!   deliberately omits.
//!
//! ## `element-apis.spec.ts`
//!
//! | # | case | line | why |
//! | --- | --- | --- | --- |
//! | 1 | `#commonEventHandler should filter out -1 uniqueId` | :42 | c — events |
//! | 2 | `#commonEventHandler should not crash on invalid uniqueId` | :71 | c — events |
//! | 3 | `#commonEventHandler should not crash on empty path` | :90 | c — events |
//! | 5 | `createElement maps input to x-input` | :113 | b — `x-input` custom element |
//! | 7 | `createCrossThreadEvent properly sets touch detail x and y` | :129 | c — events |
//! | 8 | `createCrossThreadEvent clone touches for untrusted events` | :143 | c — events |
//! | 9 | `createCrossThreadEvent sets empty detail if no touches` | :169 | c — events |
//! | 10 | `createCrossThreadEvent shifts layoutchange detail by lynx-view offset` | :183 | c — events |
//! | 11 | `createCrossThreadEvent shifts click detail while keeping top-level viewport coords` | :233 | c — events |
//! | 12 | `createCrossThreadEvent emits lynx-view-relative x/y for mouse events with preserved viewport coords` | :251 | c — events |
//! | 13 | `BoundingClientRectService caches the parent rect and invalidates on lynx-view transitionend` | :272 | b — `transitionend` + `getBoundingClientRect` |
//! | 14 | `createInvokeUIMethod returns lynx-view-relative boundingClientRect` | :300 | c — `__InvokeUIMethod` |
//! | 15 | `createInvokeUIMethod dispatches DOM methods and reports unknown methods` | :329 | c — `__InvokeUIMethod` |
//! | 16 | `createCrossThreadEvent forwards keyboard properties for keydown` | :359 | c — events |
//! | 17 | `createCrossThreadEvent forwards keyboard properties for keyup` | :382 | c — events |
//! | 19 | `__GetComputedStyleByKey returns the computed value` | :415 | c — `__GetComputedStyleByKey` |
//! | 20 | `__GetComputedStyleByKey falls back to an empty string` | :433 | c — `__GetComputedStyleByKey` |
//! | 21 | `__CreateComponent drops __Card__ entryName` | :444 | c — `__CreateComponent` |
//! | 45 | `__GetAttributes` | :707 | c — `__GetAttributes` |
//! | 51 | `__SetInlineStyles with rpx` | :810 | b — `calc()` over `--rpx-unit` |
//! | 52 | `__SetInlineStyles with ppx` | :823 | b — `calc()` over `--ppx-unit` |
//! | 53 | `__SetInlineStyles with vw and vh when enabled` | :836 | b — `calc()` over `--vw-unit` |
//! | 54 | `__SetInlineStyles with object and vw/vh when enabled` | :858 | b — `calc()` over `--vw-unit` |
//! | 56 | `__GetConfig__AddConfig` | :911 | c — `__AddConfig`/`__GetConfig` |
//! | 60 | `complicated_dom_tree_opt` | :944 | c — `__ReplaceElements` |
//! | 61 | `__ReplaceElements` | :1086 | c — `__ReplaceElements` |
//! | 62 | `__ReplaceElements_2` | :1138 | c — `__ReplaceElements` |
//! | 63 | `__ReplaceElements_3` | :1235 | c — `__ReplaceElements` |
//! | 64 | `with_querySelector` | :1381 | b — `rootDom.querySelector` over a flat shadow tree |
//! | 66 | `__ReplaceElements should accept not array` | :1436 | c — `__ReplaceElements` |
//! | 71 | ``event upper case `Tap` works`` | :1586 | c — events |
//! | 72 | `publicComponentEvent` | :1616 | c — events |
//! | 73 | `event with bubbles: false should not bubble to parent` | :1673 | c — events |
//! | 74 | `__UpdateComponentInfo` | :1711 | c — `__UpdateComponentInfo` |
//! | 75 | `__UpdateComponentInfo updates componentCSSID` | :1735 | c — `__UpdateComponentInfo` |
//! | 76 | `__MarkTemplate_and_Get_Parts` | :1769 | c — element templates |
//! | 77 | `should optimize event enable/disable for whitelisted events` | :1816 | c — events |
//! | 78 | `should handle worklet events enable/disable` | :1870 | c — worklets |
//! | 79 | `should handle global-bind events for cross-thread handlers` | :1904 | c — events |
//! | 80 | `should handle global-bind events for run-worklet handlers` | :1958 | c — worklets |
//! | 81 | `getClassList` | :2011 | b — wasm→JS `WeakRef` callback |
//! | 82 | `ssr __SetInlineStyles and __SetAttribute style transformations` | :2027 | b — SSR HTML |
//! | 83 | `ssr __GetComputedStyleByKey returns an empty string` | :2096 | c — `__GetComputedStyleByKey` |
//! | 84 | `create element infer css id from parent component in SSR` | :2119 | b — SSR HTML |
//! | 85 | `create element wont infer css id if parent css id is 0 in SSR` | :2156 | b — SSR HTML (and the case has no `expect`) |
//! | 86 | `push_style_sheet` | :2192 | b — `<style>` element in a shadow root |
//! | 87 | `__LoadLepusChunk with dynamicComponentEntry` | :2224 | c — `__LoadLepusChunk` |
//!
//! Case 18 (`__CreateComponent`, :399) is split: its
//! `__UpdateComponentID`/`__GetComponentID` half is ported by
//! [`update_component_id_and_get_component_id_round_trip`]; its
//! `__CreateComponent` and `name`-attribute half is (c).
//!
//! ## `testing-library-port.spec.ts`
//!
//! | # | case | line | why |
//! | --- | --- | --- | --- |
//! | T7 | `should add event listener` | :126 | c — events |
//! | T9 | `should handle empty text content` | :169 | b — `textContent` of an `x-text` element |
//!
//! Totals: 87 + 15 = 102 cases; 49 skipped (13 for reason b, 36 for reason c),
//! 53 ported or adapted.
//!
//! # One recorded divergence
//!
//! Case 40 (`__SwapElement`, :642) does not hold here:
//! `ElementTree::swap_element` mis-orders an **adjacent** pair. The case is
//! ported as [`swap_element_exchanges_adjacent_siblings`], pinning web-core's
//! order, and marked `#[ignore]` with the full measured matrix in its doc
//! comment rather than weakened to the order this engine currently produces.
//! The non-adjacent half of the case passes.
//!
//! # Three recorded boundary limits
//!
//! - **One CSS-fragment-id field, not two.** web-core stores `css_id` (an element's own scope) and
//!   `component_css_id` (the scope its creations seed) separately, and only `__CreatePage`/
//!   `__CreateComponent` write the second (`main_thread_context.rs:91-112`,
//!   `element_data.rs:26-27`); `__SetCSSId` writes only the first (`style_apis.rs:16-54`).
//!   `lynx-element` keeps one field, so here `__SetCSSId` also changes what later creations
//!   inherit. Cases 22/67/68 are ported over that substitution, since this subset has no
//!   `__CreateComponent`.
//! - Case 69 (`__GetElementUniqueID for incorrect fiber object`, :1538) also passes a plain object
//!   `{}`. Handles cross this runtime's host boundary as `u32` numbers and the boundary is
//!   primitives-only, so an object argument is rejected before any PAPI code runs. The `null`,
//!   `undefined`, never-issued-handle and `0` halves of the case are ported.
//! - Case 25/#4 both round-trip `scroll-view`. In web-core they do *not* agree:
//!   `__CreateScrollView` hard-codes the `scroll-view` tag while `__CreateElement('scroll-view',
//!   …)` maps to `x-scroll-view`, which the reverse map has no key for. This engine stores Lynx tag
//!   names verbatim, so both round-trip; that is the behavior `__GetTag`'s contract asks for.

use bobcat_core::engine::SharedTree;
use bobcat_core::quickjs::MainThreadRuntime;
use lynx_element::{ElementTree, PageConfig, Viewport};

const VIEWPORT: Viewport = Viewport::new(393.0, 727.0);

/// The page's permanent handle. `ElementTree` pre-creates the document
/// element, so `__CreatePage` always answers `1` and the first script-created
/// element is `2`.
const PAGE: u32 = 1;

/// Assertion helpers the ported bodies share, evaluated at the top of the
/// main-thread wrapper so `renderPage` closes over them.
///
/// `tagsOf` cannot be written `handles.map(__GetTag)`: `Array.prototype.map`
/// passes `(element, index, array)`, and the host boundary below carries
/// primitives only, so the array argument would be rejected before `__GetTag`
/// ran.
const HELPERS: &str = r"
function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}
function assertEqual(actual, expected, what) {
  if (actual !== expected) {
    throw new Error(what + ': expected ' + expected + ', got ' + actual);
  }
}
function tagsOf(handles) {
  return handles.map(function (handle) { return __GetTag(handle); }).join(',');
}
";

/// The single-threaded composition `tests/main_thread.rs` uses: the realm
/// takes the tree per batch and every `__FlushElementTree` puts it back.
fn runtime() -> (MainThreadRuntime, SharedTree) {
    let elements = SharedTree::new(ElementTree::new(VIEWPORT, PageConfig::default()));
    let runtime = MainThreadRuntime::new(elements.clone(), || {}).expect("QuickJS realm");
    (runtime, elements)
}

/// Runs `body` as the whole of `renderPage`, requiring it to succeed, and
/// hands back the committed tree for Rust-side assertions.
fn render(body: &str) -> SharedTree {
    let (mut runtime, elements) = runtime();
    runtime
        .run_main_thread_script(&script(body))
        .expect("the ported case must not throw");
    elements
}

/// The same, for a body that is expected to throw: returns the message the
/// PAPI raised into JavaScript.
fn render_error(body: &str) -> String {
    let (mut runtime, _elements) = runtime();
    runtime
        .run_main_thread_script(&script(body))
        .expect_err("the ported case must throw")
        .to_string()
}

fn script(body: &str) -> String {
    format!("{HELPERS}\nglobalThis.renderPage = function () {{\n{body}\n}};\n")
}

// ------------------------------------------------------------------ creation

/// web-core `element-apis.spec.ts:108` (`createElementView`), `:120`
/// (`createElement maps textarea to x-textarea`, `__GetTag` half),
/// `testing-library-port.spec.ts:68` (`should create and append svg element`)
/// and `:81` (`should create and append custom element`).
///
/// The rule those four share is that `__GetTag` answers the Lynx tag name
/// `__CreateElement` was given, including for a tag with no dedicated
/// `__Create*` member and for one web-core does not map at all.
#[test]
fn create_element_round_trips_every_tag_through_get_tag() {
    render(
        r"
        __CreatePage('card', 0);
        const tags = [
          'view', 'text', 'image', 'scroll-view', 'wrapper', 'frame',
          'textarea', 'input', 'svg', 'list', 'custom-element', 'div',
        ];
        for (const tag of tags) {
          assertEqual(__GetTag(__CreateElement(tag, 0)), tag, '__GetTag(' + tag + ')');
        }
        ",
    );
}

/// web-core `:479` (`__CreateView`), `:529` (`__CreateText`), `:534`
/// (`__CreateImage`), `:490` (`__CreateScrollView`), `:545`
/// (`__CreateWrapperElement`), plus `__CreateFrame`, which `ReactLynx` emits
/// and web-core's suite never covers.
#[test]
fn each_dedicated_constructor_produces_its_lynx_tag() {
    render(
        r"
        __CreatePage('card', 0);
        assertEqual(__GetTag(__CreateView(0)), 'view', '__CreateView');
        assertEqual(__GetTag(__CreateText(0)), 'text', '__CreateText');
        assertEqual(__GetTag(__CreateImage(0)), 'image', '__CreateImage');
        assertEqual(__GetTag(__CreateScrollView(0)), 'scroll-view', '__CreateScrollView');
        assertEqual(__GetTag(__CreateWrapperElement(0)), 'wrapper', '__CreateWrapperElement');
        assertEqual(__GetTag(__CreateFrame(0)), 'frame', '__CreateFrame');
        ",
    );
}

/// web-core `:484` (`__CreatePage tag reverse mapping`). web-core's page is a
/// `div` underneath and `__GetTag` reverse-maps it; this engine stores the
/// Lynx tag directly, and the observable contract is the same.
#[test]
fn create_page_returns_an_element_whose_tag_is_page() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        assertEqual(__GetTag(page), 'page', '__GetTag(page)');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.page(), Some(PAGE));
    assert_eq!(tree.tag(PAGE), Some("page"));
}

/// `__GetPageElement` is `undefined` until `__CreatePage` has run and the page
/// handle afterwards — web-core's `createElementAPI.ts:526` returns its
/// uninitialized `page` binding, and `ElementTree::page` reproduces that with
/// `Option`.
#[test]
fn get_page_element_is_undefined_before_create_page_and_the_page_after() {
    let elements = render(
        r"
        assertEqual(typeof __GetPageElement(), 'undefined', 'before __CreatePage');
        const page = __CreatePage('card', 0);
        assertEqual(__GetPageElement(), page, 'after __CreatePage');
        ",
    );
    assert_eq!(elements.tree().page(), Some(PAGE));
}

/// web-core `:539` (`__CreateRawText`) and
/// `testing-library-port.spec.ts:94` (`should create and append text with raw
/// text`). A `raw-text` element carries its content in the `text` attribute on
/// every Lynx target, which is why the framework can later rewrite it with
/// `__SetAttribute`.
#[test]
fn create_raw_text_stores_its_content_as_the_text_attribute() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const text = __CreateText(0);
        const raw = __CreateRawText('Text Element');
        assertEqual(__GetTag(raw), 'raw-text', '__GetTag');
        assertEqual(__GetAttributeByName(raw, 'text'), 'Text Element', 'text attribute');
        __AppendElement(text, raw);
        __AppendElement(page, text);
        // The DOM text node mirroring the attribute is not an element and is
        // never addressable through a handle.
        assertEqual(__GetChildren(raw).length, 0, 'raw-text element children');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.attribute(3, "text"), Some("Text Element"));
}

/// `testing-library-port.spec.ts:151` (`text should work with SetAttribute`) —
/// the constructor's content is overwritable, because the content *is* an
/// attribute.
#[test]
fn set_attribute_overwrites_raw_text_content() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const text = __CreateText(0);
        const raw = __CreateRawText('raw-text');
        __AppendElement(text, raw);
        __SetAttribute(raw, 'text', 'Hello World');
        __AppendElement(page, text);
        assertEqual(__GetAttributeByName(raw, 'text'), 'Hello World', 'rewritten text');
        ",
    );
    assert_eq!(elements.tree().attribute(3, "text"), Some("Hello World"));
}

/// `testing-library-port.spec.ts:181` (`should be case sensitive`), `:245`
/// (`normalizes whitespace (attribute check)`) and `:229` (`handles positive
/// test cases`). The content is stored verbatim — no case folding, no
/// whitespace collapsing, and a digit string stays a string.
#[test]
fn raw_text_content_preserves_whitespace_and_case_verbatim() {
    render(
        r"
        __CreatePage('card', 0);
        const sensitive = __CreateRawText('Sensitive text');
        assertEqual(__GetAttributeByName(sensitive, 'text'), 'Sensitive text', 'case');
        assert(__GetAttributeByName(sensitive, 'text') !== 'sensitive text', 'not case folded');
        const spaced = __CreateRawText('  Step 1 of 4');
        assertEqual(__GetAttributeByName(spaced, 'text'), '  Step 1 of 4', 'whitespace');
        const digits = __CreateRawText('2');
        assertEqual(__GetAttributeByName(digits, 'text'), '2', 'digits');
        ",
    );
}

/// `testing-library-port.spec.ts:114` (`should handle detached elements`) —
/// creating an element does not insert it anywhere. web-core probes with
/// `rootDom.querySelectorAll(...).length === 0`; the engine-neutral probe is
/// `__GetParent`.
#[test]
fn creating_an_element_does_not_attach_it() {
    let elements = render(
        r"
        __CreatePage('card', 0);
        const detached = __CreateElement('custom-element', 0);
        assertEqual(__GetTag(detached), 'custom-element', '__GetTag');
        assertEqual(__GetParent(detached), null, '__GetParent of a fresh element');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.parent_element(2), 0);
    assert_eq!(tree.first_element(PAGE), 0);
}

// ------------------------------------------------------------------ identity

/// web-core `:699` (`__GetElementUniqueID`) — `ret0 + 1 === ret1`, i.e. unique
/// ids are consecutive integers issued in creation order.
#[test]
fn unique_ids_are_consecutive_in_creation_order() {
    let elements = render(
        r"
        __CreatePage('card', 0);
        const first = __CreateView(0);
        const second = __CreateView(0);
        assertEqual(first + 1, second, 'consecutive ids');
        assertEqual(__GetElementUniqueID(first), first, 'the handle is the unique id');
        assertEqual(__GetElementUniqueID(second), second, 'the handle is the unique id');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.unique_id(2), 2);
    assert_eq!(tree.unique_id(3), 3);
}

/// web-core `:1538` (`__GetElementUniqueID for incorrect fiber object`) —
/// `-1` for anything that is not a live element, and never a throw
/// (`pureElementPAPIs.ts:218-220`, `renderer_functions.cc:3953`).
///
/// web-core's object argument is left out: handles cross this host boundary as
/// numbers and the boundary carries primitives only, so `{}` is rejected
/// before any PAPI code runs.
#[test]
fn get_element_unique_id_answers_minus_one_for_anything_that_is_not_a_live_element() {
    let elements = render(
        r"
        __CreatePage('card', 0);
        const live = __CreateView(0);
        assertEqual(__GetElementUniqueID(null), -1, 'null');
        assertEqual(__GetElementUniqueID(undefined), -1, 'undefined');
        assertEqual(__GetElementUniqueID(0), -1, 'the null handle 0');
        assertEqual(__GetElementUniqueID(4242), -1, 'a never-issued handle');
        assertEqual(__GetElementUniqueID(live), live, 'a live element');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.unique_id(0), -1);
    assert_eq!(tree.unique_id(4242), -1);
    assert_eq!(tree.unique_id(2), 2);
}

/// web-core `:687` (`__ElementIsEqual`) — identity, `false` when either side
/// is `null`.
#[test]
fn element_is_equal_compares_handle_identity() {
    render(
        r"
        __CreatePage('card', 0);
        const first = __CreateView(0);
        const second = __CreateView(0);
        const alias = first;
        assertEqual(__ElementIsEqual(first, second), false, 'distinct elements');
        assertEqual(__ElementIsEqual(first, alias), true, 'the same element');
        assertEqual(__ElementIsEqual(first, null), false, 'null operand');
        ",
    );
}

// ---------------------------------------------------------------- css scope

/// The single most load-bearing tolerance in the whole member set:
/// `parentComponentUniqueID` names *no parent* and links nothing — web-core's
/// `create_element_common` looks it up only to seed the new element's CSS
/// fragment id and falls back to "unscoped" on a miss
/// (`main_thread_context.rs:91-99`). Rejecting a stale handle would turn
/// `ReactLynx`'s page-teardown race, where `__pageId` has been reset to `0`,
/// into a hard failure instead of an unstyled element.
#[test]
fn a_stale_parent_component_handle_yields_an_unscoped_element_rather_than_an_error() {
    let elements = render(
        r"
        __CreatePage('card', 0);
        const stale = __CreateView(999999);
        const negative = __CreateElement('view', -1);
        const zero = __CreateText(0);
        assert(stale > 0 && negative > 0 && zero > 0, 'every creation still returns a handle');
        ",
    );
    let tree = elements.tree();
    for id in 2..=4 {
        assert_eq!(
            tree.element(id).expect("a live element").css_id(),
            0,
            "element {id} must be unscoped"
        );
    }
}

/// web-core `:1486` (`create element infer css id from parent component id`)
/// and `:459` (`client component_css_id properly cascades to child element`).
///
/// The inference is by the creation-time `parentComponentUniqueID` argument,
/// not by tree position: web-core appends the inheriting element somewhere
/// else entirely and still expects the id.
///
/// The page stands in for `__CreateComponent(…, cssId, …)`, which this subset
/// omits — and it is the right stand-in, not a convenient one: web-core seeds
/// inheritance from `component_css_id`, which only `__CreatePage` and
/// `__CreateComponent` write (`main_thread_context.rs:88-99`,
/// `element_data.rs:26-27`). `__SetCSSId` would not do: it writes the
/// element's own `css_id` and leaves later creations unscoped
/// (`style_apis.rs:16-54`).
#[test]
fn css_id_is_inherited_from_the_parent_component_handle_not_from_the_tree() {
    let elements = render(
        r"
        const page = __CreatePage('card', 100);
        const elsewhere = __CreateView(page);
        __AppendElement(page, elsewhere);

        // Created under the page's handle, attached under `elsewhere`.
        const scoped = __CreateText(page);
        __AppendElement(elsewhere, scoped);
        assertEqual(__GetParent(scoped), elsewhere, 'the inheriting element hangs off the view');

        // Created under a plain view's handle. That view is itself scoped to
        // 100, but a view is not a component, so it seeds nothing onward — a
        // scope is not handed down transitively.
        const unscoped = __CreateText(elsewhere);
        __AppendElement(elsewhere, unscoped);
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.element(2).expect("elsewhere").css_id(), 100);
    assert_eq!(tree.element(3).expect("scoped").css_id(), 100);
    assert_eq!(tree.element(4).expect("unscoped").css_id(), 0);
}

/// web-core `:1512` (`create element wont infer for cssid 0`) — `0` is the
/// "no scope" sentinel and is not inherited. web-core draws the contrast
/// across two cases (`:1486` versus `:1512`); it is drawn here by rendering
/// the same script under both `componentCSSID`s, so the `0` assertion can
/// actually fail.
#[test]
fn css_id_zero_is_not_inherited() {
    const SCRIPT: &str = r"
        const page = __CreatePage('card', CSS_ID);
        __AppendElement(page, __CreateText(page));
    ";
    for (component_css_id, expected) in [(100, 100), (0, 0)] {
        let elements = render(&SCRIPT.replace("CSS_ID", &component_css_id.to_string()));
        let tree = elements.tree();
        assert_eq!(
            tree.element(1).expect("page").css_id(),
            expected,
            "componentCSSID {component_css_id}"
        );
        assert_eq!(
            tree.element(2).expect("child").css_id(),
            expected,
            "componentCSSID {component_css_id}"
        );
    }
}

/// `__SetCSSId(elements, cssId, entryName)` takes either one handle or an
/// array of them — the declaration `ReactLynx` compiles against
/// (`packages/react/runtime/types/types.d.ts:94`) is scalar-or-array and makes
/// `entryName` optional, which is wider than web-core's array-only
/// `SetCSSIdPAPI`. A nullish id means `0`.
#[test]
fn set_css_id_accepts_one_handle_or_an_array_and_coalesces_a_nullish_id_to_zero() {
    let elements = render(
        r"
        __CreatePage('card', 0);
        const single = __CreateView(0);
        const left = __CreateView(0);
        const right = __CreateView(0);
        const cleared = __CreateView(0);
        __SetCSSId(single, 7);
        __SetCSSId([left, right], 9, 'entry');
        __SetCSSId(cleared, 11);
        __SetCSSId(cleared, null);
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.element(2).expect("single").css_id(), 7);
    assert_eq!(tree.element(3).expect("left").css_id(), 9);
    assert_eq!(tree.element(4).expect("right").css_id(), 9);
    assert_eq!(tree.element(5).expect("cleared").css_id(), 0);
}

// ----------------------------------------------------------------- structure

/// web-core `:550` (`__AppendElement-children-count`) and `:672`
/// (`__GetChildren`) — `__GetChildren` is a real `Array` whose length tracks
/// appends, and `__AppendElement` answers the child.
#[test]
fn get_children_is_a_real_array_whose_length_tracks_appends() {
    let elements = render(
        r"
        __CreatePage('card', 0);
        const parent = __CreateView(0);
        assert(Array.isArray(__GetChildren(parent)), '__GetChildren must return an Array');
        assertEqual(__GetChildren(parent).length, 0, 'a fresh element has no children');
        const first = __CreateView(0);
        const second = __CreateView(0);
        assertEqual(__AppendElement(parent, first), first, '__AppendElement returns the child');
        __AppendElement(parent, second);
        const children = __GetChildren(parent);
        assertEqual(children.length, 2, 'child count');
        assertEqual(children[0], first, 'children[0]');
        assertEqual(children[1], second, 'children[1]');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.first_element(2), 3);
    assert_eq!(tree.last_element(2), 4);
}

/// web-core `:559` (`__AppendElement-__RemoveElement`) — the child list
/// shrinks by one.
#[test]
fn remove_element_shrinks_the_child_list() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const parent = __CreateView(0);
        __AppendElement(page, parent);
        const first = __CreateView(0);
        const second = __CreateView(0);
        __AppendElement(parent, first);
        __AppendElement(parent, second);
        assertEqual(__GetChildren(parent).length, 2, 'before removal');
        __RemoveElement(parent, first);
        assertEqual(__GetChildren(parent).length, 1, 'after removal');
        assertEqual(__GetChildren(parent)[0], second, 'the surviving child');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.first_element(2), 4);
    assert_eq!(tree.next_element(4), 0);
}

/// `testing-library-port.spec.ts:197` (`__RemoveElement should work`) plus the
/// contract web-core's `parent.removeChild(child)` implies
/// (`pureElementPAPIs.ts:81-84`): removal detaches and destroys nothing, so
/// the handle keeps resolving and re-insertion restores the element. That is
/// what `ReactLynx`'s reconciler relies on when it reorders children.
#[test]
fn remove_element_detaches_without_destroying_the_element() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const parent = __CreateView(0);
        __AppendElement(page, parent);
        const children = [];
        for (let index = 0; index < 6; index += 1) {
          const child = __CreateView(0);
          __AppendElement(parent, child);
          __SetID(child, 'child-' + index);
          children.push(child);
        }
        assertEqual(__GetChildren(parent).length, 6, 'before removal');
        __RemoveElement(parent, children[0]);
        __RemoveElement(parent, children[4]);
        assertEqual(__GetChildren(parent).length, 4, 'after removal');
        assertEqual(__GetParent(children[0]), null, 'a removed child is detached');
        // Detached, not destroyed: the handle still resolves and still carries
        // everything it was given.
        assertEqual(__GetElementUniqueID(children[0]), children[0], 'the handle survives');
        assertEqual(__GetID(children[0]), 'child-0', 'its id survives');
        __AppendElement(parent, children[0]);
        assertEqual(__GetParent(children[0]), parent, 're-insertable');
        assertEqual(__GetChildren(parent).length, 5, 'after re-insertion');
        ",
    );
    let tree = elements.tree();
    for id in 3..=8 {
        assert!(tree.element(id).is_some(), "handle {id} must still be live");
    }
    assert_eq!(tree.id_attribute(7), Some("child-4"));
    assert_eq!(tree.parent_element(7), 0, "child-4 stays detached");
}

/// web-core `:728` (`__SetDataset`) appends the same child to the same parent
/// twice and still expects one element. Appending an attached child moves it;
/// it never duplicates.
#[test]
fn appending_the_same_child_twice_moves_it_rather_than_duplicating_it() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const first = __CreateView(0);
        const second = __CreateView(0);
        __AppendElement(page, first);
        __AppendElement(page, second);
        __AppendElement(page, first);
        const children = __GetChildren(page);
        assertEqual(children.length, 2, 'child count after the repeated append');
        assertEqual(children[0], second, 'the repeated append moved the child to the end');
        assertEqual(children[1], first, 'children[1]');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.first_element(PAGE), 3);
    assert_eq!(tree.last_element(PAGE), 2);
}

/// web-core `:569` (`__InsertElementBefore`) — a `null` reference appends, a
/// non-null one inserts before it. The case builds `[text, image, view]` by
/// inserting each new child before the previous one.
#[test]
fn insert_element_before_appends_on_a_null_reference_and_inserts_before_a_real_one() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        const image = __CreateImage(0);
        const text = __CreateText(0);
        __InsertElementBefore(page, view, null);
        assertEqual(tagsOf(__GetChildren(page)), 'view', 'a null reference appends');
        __InsertElementBefore(page, image, view);
        __InsertElementBefore(page, text, image);
        const children = __GetChildren(page);
        assertEqual(children.length, 3, 'child count');
        assertEqual(tagsOf(children), 'text,image,view', 'insertion order');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.tag(tree.first_element(PAGE)), Some("text"));
    assert_eq!(tree.tag(tree.last_element(PAGE)), Some("view"));
}

/// `parent.insertBefore(child, child)` is a legal DOM no-op and web-core's
/// `__InsertElementBefore` (`pureElementPAPIs.ts:67-71`) passes it straight
/// through, so a diffing framework does emit it. The DOM core below
/// debug-asserts against it, which is why `ElementTree` intercepts the case.
#[test]
fn insert_element_before_the_child_itself_is_a_no_op() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const first = __CreateView(0);
        const second = __CreateImage(0);
        __AppendElement(page, first);
        __AppendElement(page, second);
        __InsertElementBefore(page, first, first);
        assertEqual(tagsOf(__GetChildren(page)), 'view,image', 'order is unchanged');
        assertEqual(__GetParent(first), page, 'the child is still attached');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.first_element(PAGE), 2);
    assert_eq!(tree.last_element(PAGE), 3);
}

/// web-core `:583` (`__FirstElement`), `:597` (`__LastElement`) and `:611`
/// (`__NextElement`) each begin by asserting the query is falsy when there is
/// no such relative. This runtime answers `null` there, matching web-core.
#[test]
fn navigation_queries_answer_null_when_there_is_no_such_relative() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        assertEqual(__FirstElement(page), null, '__FirstElement of an empty element');
        assertEqual(__LastElement(page), null, '__LastElement of an empty element');
        assertEqual(__NextElement(page), null, '__NextElement of the page');
        assertEqual(__GetParent(page), null, '__GetParent of the page');
        const only = __CreateView(0);
        __AppendElement(page, only);
        assertEqual(__NextElement(only), null, '__NextElement of the last child');
        assertEqual(__FirstElement(only), null, '__FirstElement of a leaf');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.parent_element(PAGE), 0);
    assert_eq!(tree.next_element(2), 0);
}

/// web-core `:583`/`:597` — after `[text, image, view]` has been built,
/// `__FirstElement` is the `text` and `__LastElement` the `view`.
#[test]
fn first_and_last_element_report_the_ends_of_the_child_list() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        const image = __CreateImage(0);
        const text = __CreateText(0);
        __InsertElementBefore(page, view, null);
        __InsertElementBefore(page, image, view);
        __InsertElementBefore(page, text, image);
        assertEqual(__GetTag(__FirstElement(page)), 'text', '__FirstElement');
        assertEqual(__GetTag(__LastElement(page)), 'view', '__LastElement');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.tag(tree.first_element(PAGE)), Some("text"));
    assert_eq!(tree.tag(tree.last_element(PAGE)), Some("view"));
}

/// web-core `:611` (`__NextElement`) — the sibling walk `__ChildAt` and every
/// list-diffing framework depend on.
#[test]
fn next_element_walks_the_child_list_in_order() {
    render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        const image = __CreateImage(0);
        const text = __CreateText(0);
        __InsertElementBefore(page, view, null);
        __InsertElementBefore(page, image, view);
        __InsertElementBefore(page, text, image);
        const first = __FirstElement(page);
        assertEqual(__GetTag(__NextElement(first)), 'image', 'the second child');
        assertEqual(__GetTag(__NextElement(__NextElement(first))), 'view', 'the third child');
        assertEqual(__NextElement(__NextElement(__NextElement(first))), null, 'past the end');
        ",
    );
}

/// web-core `:659` (`__GetParent`).
#[test]
fn get_parent_reports_the_element_a_child_was_appended_to() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const parent = __CreateView(0);
        __AppendElement(page, parent);
        const child = __CreateView(0);
        __AppendElement(parent, child);
        assertEqual(__GetParent(child), parent, '__GetParent of a child');
        assertEqual(__GetParent(parent), page, '__GetParent of the parent');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.parent_element(3), 2);
    assert_eq!(tree.parent_element(2), PAGE);
}

/// web-core `:625` (`__ReplaceElement`) — the argument order is **new element
/// first**, because the member is `oldElement.replaceWith(newElement)`
/// (`pureElementPAPIs.ts:86-89`) and the parent is read off the old element.
#[test]
fn replace_element_takes_the_new_element_first() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        const image = __CreateImage(0);
        const text = __CreateText(0);
        __InsertElementBefore(page, view, null);
        __InsertElementBefore(page, image, view);
        __InsertElementBefore(page, text, image);
        const replacement = __CreateScrollView(0);
        __ReplaceElement(replacement, image);
        assertEqual(tagsOf(__GetChildren(page)), 'text,scroll-view,view', 'the replacement is in place');
        assertEqual(__GetTag(__NextElement(__FirstElement(page))), 'scroll-view', 'second child');
        assertEqual(__GetParent(image), null, 'the replaced element is detached');
        ",
    );
    let tree = elements.tree();
    // Detached, not destroyed: the same rule `__RemoveElement` follows.
    assert!(tree.element(3).is_some(), "the replaced element stays live");
    assert_eq!(tree.parent_element(3), 0);
}

/// web-core `:642` (`__SwapElement`), non-adjacent half — the two elements
/// exchange positions in place and every other child keeps its slot.
#[test]
fn swap_element_exchanges_two_non_adjacent_children() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        const image = __CreateImage(0);
        const text = __CreateText(0);
        __AppendElement(page, view);
        __AppendElement(page, image);
        __AppendElement(page, text);
        __SwapElement(view, text);
        assertEqual(tagsOf(__GetChildren(page)), 'text,image,view', 'first and third swapped');
        __SwapElement(text, view);
        assertEqual(tagsOf(__GetChildren(page)), 'view,image,text', 'swapping back restores the order');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.first_element(PAGE), 2);
    assert_eq!(tree.last_element(PAGE), 4);
}

/// web-core `:642` (`__SwapElement`), verbatim: children `[view, image, text]`
/// and `__SwapElement(child_0, child_1)` must leave `[image, view, text]`. The
/// native engine agrees — it removes both and re-inserts by saved index
/// (`renderer_functions.cc:3469-3485`).
///
/// **Ignored: this engine does not do that yet.** `ElementTree::swap_element`
/// saves each node's successor before detaching both, and when one saved
/// successor *is* the other swapped node the re-insertion degrades to an
/// append instead of following through to that node's own saved successor.
/// The order observed for this case is `[view, text, image]`.
///
/// Measured over every adjacent configuration, with children `[t0, t1, t2]`
/// and `[t0, t1]`:
///
/// | call | expected | observed |
/// | --- | --- | --- |
/// | `__SwapElement(t0, t1)` of three | `t1,t0,t2` | `t0,t2,t1` |
/// | `__SwapElement(t1, t0)` of three | `t1,t0,t2` | `t0,t2,t1` |
/// | `__SwapElement(t1, t2)` of three | `t0,t2,t1` | `t0,t2,t1` |
/// | `__SwapElement(t2, t1)` of three | `t0,t2,t1` | `t0,t1,t2` |
/// | `__SwapElement(t0, t1)` of two | `t1,t0` | `t1,t0` |
/// | `__SwapElement(t1, t0)` of two | `t1,t0` | `t0,t1` |
///
/// An adjacent pair therefore lands correctly only when it is passed
/// (earlier, later) *and* the later element is the last child; the member is
/// also argument-order sensitive, which it must not be. Non-adjacent pairs are
/// unaffected in either argument order — neither saved successor can be the
/// other swapped node — which is why
/// [`swap_element_exchanges_two_non_adjacent_children`] passes. Un-ignore once
/// `swap_element` chains through the other node's saved successor.
#[test]
fn swap_element_exchanges_adjacent_siblings() {
    render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        const image = __CreateImage(0);
        const text = __CreateText(0);
        __AppendElement(page, view);
        __AppendElement(page, image);
        __AppendElement(page, text);
        __SwapElement(view, image);
        assertEqual(tagsOf(__GetChildren(page)), 'image,view,text', 'first and second swapped');
        __SwapElement(image, view);
        assertEqual(tagsOf(__GetChildren(page)), 'view,image,text', 'swapping back restores the order');
        ",
    );
}

// ---------------------------------------------------------------- attributes

/// web-core `:495` (`create-scroll-view-with-set-attribute`) writes the
/// **boolean** `true` and reads back `'true'`. Everything that is not nullish
/// goes through ECMAScript string coercion, so `false` and `0` are written as
/// `"false"` and `"0"` rather than removed.
#[test]
fn set_attribute_stringifies_booleans_and_numbers_including_the_falsy_ones() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateScrollView(0);
        __AppendElement(page, view);
        __SetAttribute(view, 'scroll-x', true);
        __SetAttribute(view, 'scroll-y', false);
        __SetAttribute(view, 'count', 12);
        __SetAttribute(view, 'zero', 0);
        assertEqual(__GetAttributeByName(view, 'scroll-x'), 'true', 'true');
        assertEqual(__GetAttributeByName(view, 'scroll-y'), 'false', 'false is written, not removed');
        assertEqual(__GetAttributeByName(view, 'count'), '12', 'a number');
        assertEqual(__GetAttributeByName(view, 'zero'), '0', '0 is written, not removed');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.attribute(2, "scroll-y"), Some("false"));
    assert_eq!(tree.attribute(2, "zero"), Some("0"));
}

/// web-core `:1428` (`__setAttribute_null_value`) — a nullish value removes
/// the attribute (`setElementPropertyOrAttribute`), which is the only way the
/// member deletes anything.
#[test]
fn a_nullish_attribute_value_removes_the_attribute() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetAttribute(view, 'test-attr', 'val');
        assertEqual(__GetAttributeByName(view, 'test-attr'), 'val', 'written');
        __SetAttribute(view, 'test-attr', null);
        assertEqual(__GetAttributeByName(view, 'test-attr'), null, 'null removes');
        __SetAttribute(view, 'other-attr', 'val');
        __SetAttribute(view, 'other-attr', undefined);
        assertEqual(__GetAttributeByName(view, 'other-attr'), null, 'undefined removes');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.attribute(2, "test-attr"), None);
    assert_eq!(tree.attribute(2, "other-attr"), None);
}

/// web-core `:718` (`__GetAttributeByName`) — the page element is an ordinary
/// attribute target.
#[test]
fn get_attribute_by_name_reads_back_what_set_attribute_wrote_on_the_page() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        __SetAttribute(page, 'test-attr', 'val');
        assertEqual(__GetAttributeByName(page, 'test-attr'), 'val', 'page attribute');
        assertEqual(__GetAttributeByName(page, 'absent'), null, 'an absent attribute is null');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.attribute(PAGE, "test-attr"), Some("val"));
    assert_eq!(tree.attribute(PAGE, "absent"), None);
}

/// web-core `:507` (`__SetID`) and `:516` (`__SetID to remove id`) — a nullish
/// id clears (`pureElementPAPIs.ts:140-141`). web-core probes with
/// `rootDom.querySelector('#target')`; the engine-neutral probe is `__GetID`.
#[test]
fn set_id_writes_the_id_and_a_nullish_id_clears_it() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetID(view, 'target');
        assertEqual(__GetID(view), 'target', '__GetID');
        assertEqual(__GetAttributeByName(view, 'id'), 'target', 'the id attribute');
        __SetID(view, null);
        assertEqual(__GetID(view), null, 'null clears the id');
        assertEqual(__GetAttributeByName(view, 'id'), null, 'the id attribute is gone');
        ",
    );
    assert_eq!(elements.tree().id_attribute(2), None);
}

/// web-core `:744` (`__GetClasses`) — the class list is **ordered**, not a
/// set: three `__AddClass` calls read back as `['a','b','c']`.
#[test]
fn get_classes_preserves_insertion_order() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddClass(view, 'a');
        __AddClass(view, 'b');
        __AddClass(view, 'c');
        const classes = __GetClasses(view);
        assert(Array.isArray(classes), '__GetClasses must return an Array');
        assertEqual(classes.length, 3, 'class count');
        assertEqual(classes.join(','), 'a,b,c', 'insertion order');
        ",
    );
    let tree = elements.tree();
    let classes: Vec<&str> = tree.classes(2).collect();
    assert_eq!(classes, ["a", "b", "c"]);
}

/// web-core `:744`, second half — `__SetClasses('c b a')` replaces the list
/// and reads back `['c','b','a']`, which is the same proof that the list is
/// ordered.
#[test]
fn set_classes_replaces_the_list_in_the_given_order() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddClass(view, 'a');
        __AddClass(view, 'b');
        __AddClass(view, 'c');
        __SetClasses(view, 'c b a');
        const classes = __GetClasses(view);
        assertEqual(classes.length, 3, 'class count');
        assertEqual(classes.join(','), 'c,b,a', 'the order given to __SetClasses');
        assertEqual(__GetAttributeByName(view, 'class'), 'c b a', 'the class attribute');
        ",
    );
    let tree = elements.tree();
    let classes: Vec<&str> = tree.classes(2).collect();
    assert_eq!(classes, ["c", "b", "a"]);
}

/// `__SetClasses(element, null)` removes the `class` attribute entirely
/// (`pureElementPAPIs.ts:162-169`).
#[test]
fn set_classes_with_a_nullish_argument_clears_the_class_list() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetClasses(view, 'a b');
        __SetClasses(view, null);
        assertEqual(__GetClasses(view).length, 0, 'the class list is empty');
        assertEqual(__GetAttributeByName(view, 'class'), null, 'the class attribute is gone');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.classes(2).count(), 0);
}

// -------------------------------------------------------------- inline styles

/// web-core `:937` (`__AddInlineStyle_raw_string`) — `__SetInlineStyles`
/// accepts a raw declaration string, with or without a trailing `;`.
#[test]
fn set_inline_styles_accepts_a_declaration_string() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetInlineStyles(view, 'height:80px');
        assertEqual(__GetAttributeByName(view, 'style'), 'height:80px', 'no trailing semicolon');
        __SetInlineStyles(view, 'color:red;margin-top:10px;');
        assertEqual(__GetAttributeByName(view, 'style'), 'color:red;margin-top:10px;', 'trailing semicolon');
        ",
    );
    assert_eq!(
        elements.tree().attribute(2, "style"),
        Some("color:red;margin-top:10px;")
    );
}

/// web-core `:790` (`__SetInlineStyles`) passes a record with camelCase keys
/// (`marginTop`), and `server-ssr-bulk.spec.ts:10` passes kebab-case ones
/// (`margin-top`). Both are accepted and hyphenated into one declaration
/// block, in key order.
///
/// web-core's `rpx`/`ppx`/`vw`/`vh` → `calc()` rewriting is not reproduced: it
/// is a workaround for a browser that cannot resolve Lynx units, and this
/// engine resolves them in the cascade.
#[test]
fn set_inline_styles_accepts_camel_case_and_kebab_case_record_keys() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const camel = __CreateView(0);
        const kebab = __CreateView(0);
        __AppendElement(page, camel);
        __AppendElement(page, kebab);
        __SetInlineStyles(camel, { marginTop: '20px', marginLeft: '30px' });
        assertEqual(
          __GetAttributeByName(camel, 'style'),
          'margin-top:20px;margin-left:30px;',
          'camelCase keys are hyphenated'
        );
        __SetInlineStyles(kebab, { 'color': 'red', 'font-size': '16px', 'margin-top': '10px' });
        assertEqual(
          __GetAttributeByName(kebab, 'style'),
          'color:red;font-size:16px;margin-top:10px;',
          'kebab-case keys pass through'
        );
        ",
    );
    let tree = elements.tree();
    assert_eq!(
        tree.attribute(2, "style"),
        Some("margin-top:20px;margin-left:30px;")
    );
}

/// web-core `:790` opens with `__SetInlineStyles(target, undefined)` — a
/// nullish or empty value is legal and leaves no `style` attribute behind.
#[test]
fn set_inline_styles_with_a_nullish_value_leaves_no_style_attribute() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetInlineStyles(view, undefined);
        assertEqual(__GetAttributeByName(view, 'style'), null, 'undefined on a fresh element');
        __SetInlineStyles(view, 'height:80px;');
        __SetInlineStyles(view, undefined);
        assertEqual(__GetAttributeByName(view, 'style'), null, 'undefined clears an existing block');
        __SetInlineStyles(view, 'height:80px;');
        __SetInlineStyles(view, null);
        assertEqual(__GetAttributeByName(view, 'style'), null, 'null clears an existing block');
        ",
    );
    assert_eq!(elements.tree().attribute(2, "style"), None);
}

/// web-core `:930` (`__AddInlineStyle_key_is_name`) — the string-key form
/// merges one declaration into the block. A nullish value removes just that
/// declaration; web-core's `__AddInlineStyle` null branch does the same.
#[test]
fn add_inline_style_merges_one_declaration_and_a_nullish_value_removes_it() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetInlineStyles(view, 'color:red;');
        __AddInlineStyle(view, 'height', '80px');
        assertEqual(__GetAttributeByName(view, 'style'), 'color:red;height:80px;', 'merged');
        __AddInlineStyle(view, 'width', '10px');
        assertEqual(
          __GetAttributeByName(view, 'style'),
          'color:red;height:80px;width:10px;',
          'a second declaration merges too'
        );
        __AddInlineStyle(view, 'height', null);
        assertEqual(
          __GetAttributeByName(view, 'style'),
          'color:red;width:10px;',
          'a nullish value removes only that declaration'
        );
        ",
    );
    assert_eq!(
        elements.tree().attribute(2, "style"),
        Some("color:red;width:10px;")
    );
}

/// `__SetInlineStyles` writes the `style` block wholesale, so it discards
/// every declaration a previous `__AddInlineStyle` layered on — web-core's
/// member assigns `element.style.cssText`/`setAttribute('style', …)` rather
/// than merging.
#[test]
fn set_inline_styles_discards_earlier_add_inline_style_declarations() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddInlineStyle(view, 'height', '80px');
        __SetInlineStyles(view, 'color:red;');
        assertEqual(__GetAttributeByName(view, 'style'), 'color:red;', 'the whole block was replaced');
        ",
    );
    assert_eq!(elements.tree().attribute(2, "style"), Some("color:red;"));
}

/// web-core `:1574` (`__AddInlineStyle_value_number_0`) — the numeric value
/// `0` must reach the block rather than being dropped as falsy. web-core
/// passes the property as the numeric id `51`; the string name carries the
/// same rule here.
#[test]
fn add_inline_style_keeps_a_numeric_zero_value() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddInlineStyle(view, 'flex-shrink', 0);
        const style = __GetAttributeByName(view, 'style');
        assert(style.indexOf('flex-shrink') !== -1, 'flex-shrink is present, got ' + style);
        assertEqual(style, 'flex-shrink:0;', 'the 0 was written');
        ",
    );
    assert_eq!(
        elements.tree().attribute(2, "style"),
        Some("flex-shrink:0;")
    );
}

/// web-core `:923` (`__AddInlineStyle`) calls `__AddInlineStyle(root, 26,
/// '80px')`, pinning the native CSS property id table (`24` is `display`, `26`
/// `height`, `51` `flex-shrink`).
///
/// This crate does not carry that table, so the numeric form is a precise
/// error naming the limitation rather than a silently dropped declaration.
#[test]
fn add_inline_style_rejects_a_numeric_property_id_with_a_precise_error() {
    let message = render_error(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddInlineStyle(view, 26, '80px');
        ",
    );
    assert!(
        message.contains("numeric CSS property id 26"),
        "the error must name the rejected id: {message}"
    );
    assert!(
        message.contains("only string property names are accepted"),
        "the error must name the limitation: {message}"
    );
}

/// web-core `:884` (`__SetAttribute style with rpx, ppx, vw, vh`) — the
/// client's `__SetAttribute(el, 'style', …)` writes the text through
/// untransformed, unlike `__SetInlineStyles`. Here nothing transforms units at
/// all, so the same verbatim-passthrough assertion holds for both entry
/// points.
#[test]
fn set_attribute_style_stores_the_declaration_text_verbatim() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetAttribute(view, 'style', 'width: 50vw; height: 100vh; margin: 10rpx; padding: 5ppx;');
        assertEqual(
          __GetAttributeByName(view, 'style'),
          'width: 50vw; height: 100vh; margin: 10rpx; padding: 5ppx;',
          'no unit rewriting'
        );
        ",
    );
    assert_eq!(
        elements.tree().attribute(2, "style"),
        Some("width: 50vw; height: 100vh; margin: 10rpx; padding: 5ppx;")
    );
}

// -------------------------------------------------------------------- dataset

/// web-core `:728` (`__SetDataset`) and `testing-library-port.spec.ts:54`
/// (`should add dataset to view`) — a truthy value is stored *and* mirrored as
/// a `data-<key>` attribute (`createElementAPI.ts:426-437`).
#[test]
fn add_dataset_stores_the_value_and_mirrors_a_truthy_one_as_a_data_attribute() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddDataset(view, 'testid', 'view-element');
        assertEqual(__GetDataByKey(view, 'testid'), 'view-element', '__GetDataByKey');
        assertEqual(__GetAttributeByName(view, 'data-testid'), 'view-element', 'the mirrored attribute');
        assertEqual(__GetDataByKey(view, 'absent'), undefined, 'an absent key');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.attribute(2, "data-testid"), Some("view-element"));
    assert!(tree.data_by_key(2, "testid").is_some());
    assert!(tree.data_by_key(2, "absent").is_none());
}

/// The store, not the DOM, is the dataset: `__AddDataset` mirrors the
/// `data-*` attribute only when the value is truthy
/// (`createElementAPI.ts:426-437`), so `false`, `0` and `''` stay readable
/// through `__GetDataByKey` while the attribute is removed.
#[test]
fn add_dataset_keeps_a_falsy_value_readable_but_removes_the_data_attribute() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __AddDataset(view, 'flag', false);
        __AddDataset(view, 'count', 0);
        __AddDataset(view, 'label', '');
        assertEqual(__GetDataByKey(view, 'flag'), false, 'false is stored');
        assertEqual(__GetDataByKey(view, 'count'), 0, '0 is stored');
        assertEqual(__GetDataByKey(view, 'label'), '', 'the empty string is stored');
        assertEqual(__GetAttributeByName(view, 'data-flag'), null, 'no data-flag attribute');
        assertEqual(__GetAttributeByName(view, 'data-count'), null, 'no data-count attribute');
        assertEqual(__GetAttributeByName(view, 'data-label'), null, 'no data-label attribute');
        // A truthy overwrite of the same key writes the attribute back.
        __AddDataset(view, 'flag', true);
        assertEqual(__GetAttributeByName(view, 'data-flag'), 'true', 'the attribute returns');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.attribute(2, "data-count"), None);
    assert!(
        tree.data_by_key(2, "count").is_some(),
        "the falsy value stays in the store"
    );
}

/// `__SetDataset(element, dataset)` replaces the whole map rather than merging
/// into it (`createElementAPI.ts:422-425`), and the mirrored attributes of the
/// dropped keys go with it.
#[test]
fn set_dataset_replaces_the_whole_map() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const view = __CreateView(0);
        __AppendElement(page, view);
        __SetDataset(view, { test: 'test-value' });
        assertEqual(__GetDataByKey(view, 'test'), 'test-value', 'the first map');
        __AddDataset(view, 'test1', 'test-value1');
        assertEqual(__GetDataByKey(view, 'test1'), 'test-value1', '__AddDataset merges');
        __SetDataset(view, { only: 'one' });
        assertEqual(__GetDataByKey(view, 'only'), 'one', 'the replacement map');
        assertEqual(__GetDataByKey(view, 'test'), undefined, 'the old key is gone');
        assertEqual(__GetDataByKey(view, 'test1'), undefined, 'the merged key is gone too');
        assertEqual(__GetAttributeByName(view, 'data-test'), null, 'and its mirrored attribute');
        ",
    );
    let tree = elements.tree();
    assert!(tree.data_by_key(2, "test").is_none());
    assert!(tree.data_by_key(2, "only").is_some());
    assert_eq!(tree.attribute(2, "data-test"), None);
    assert_eq!(tree.attribute(2, "data-only"), Some("one"));
}

// --------------------------------------------------------- component identity

/// web-core `:399` (`__CreateComponent`), `__UpdateComponentID`/
/// `__GetComponentID` half. The component id is a *string* name, unrelated to
/// the numeric unique id, and web-core keeps it out of the DOM in a side
/// table; the same is true here. `__CreateComponent` itself is not
/// implemented, so an ordinary element carries the identity.
#[test]
fn update_component_id_and_get_component_id_round_trip() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const element = __CreateView(0);
        __AppendElement(page, element);
        assertEqual(__GetComponentID(element), null, 'no component id before __UpdateComponentID');
        __UpdateComponentID(element, 'id');
        assertEqual(__GetComponentID(element), 'id', 'after __UpdateComponentID');
        assertEqual(__GetComponentID(page), 'card', '__CreatePage recorded its componentID');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.component_id(2), Some("id"));
    assert_eq!(tree.component_id(PAGE), Some("card"));
}

/// web-core `:763` (`__UpdateComponentID`) — two component ids are swapped
/// between two elements, colliding transiently, and both reads afterwards must
/// report the new value.
#[test]
fn component_ids_can_be_swapped_between_two_elements() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const first = __CreateView(0);
        const second = __CreateView(0);
        __AppendElement(page, first);
        __AppendElement(page, second);
        __UpdateComponentID(first, 'id1');
        __UpdateComponentID(second, 'id2');
        __UpdateComponentID(first, 'id2');
        __UpdateComponentID(second, 'id1');
        assertEqual(__GetComponentID(first), 'id2', 'first');
        assertEqual(__GetComponentID(second), 'id1', 'second');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.component_id(2), Some("id2"));
    assert_eq!(tree.component_id(3), Some("id1"));
}

// ---------------------------------------------------------------------- flush

/// web-core builds and asserts a whole child list before its first
/// `__FlushElementTree` (`element-apis.spec.ts:952-996`, asserted before the
/// flush at `:997`): the tree is mutated immediately and the flush is the
/// style/layout commit, not the mutation.
#[test]
fn tree_mutations_are_observable_before_a_flush() {
    let elements = render(
        r"
        const page = __CreatePage('card', 0);
        const first = __CreateView(0);
        const second = __CreateImage(0);
        __AppendElement(page, first);
        __AppendElement(page, second);
        // No flush has run yet.
        assertEqual(__GetChildren(page).length, 2, 'appends are visible before the flush');
        assertEqual(tagsOf(__GetChildren(page)), 'view,image', 'and so is their order');
        __RemoveElement(page, first);
        assertEqual(__GetChildren(page).length, 1, 'so are removals');
        __FlushElementTree();
        assertEqual(__GetChildren(page).length, 1, 'the flush changes no structure');
        const third = __CreateText(0);
        __AppendElement(page, third);
        assertEqual(__GetChildren(page).length, 2, 'and mutation continues after it');
        ",
    );
    let tree = elements.tree();
    assert_eq!(tree.tag(tree.first_element(PAGE)), Some("image"));
    assert_eq!(tree.tag(tree.last_element(PAGE)), Some("text"));
}

/// The Rust side of the same rule: a batch that never reaches
/// `__FlushElementTree` has already changed the tree, and the tree reports the
/// batch as uncommitted so a frame producer keeps its retained frame.
#[test]
fn an_unflushed_batch_is_already_in_the_tree_and_reports_itself_uncommitted() {
    let (mut runtime, elements) = runtime();
    runtime
        .evaluate_main_thread_script(
            r"
            globalThis.renderPage = function () {};
            __AppendElement(__CreatePage('card', 0), __CreateView(0));
            ",
        )
        .expect("evaluate");

    let tree = elements.tree();
    assert!(
        tree.has_uncommitted_mutations(),
        "no __FlushElementTree has run"
    );
    assert_eq!(tree.page(), Some(PAGE));
    assert_eq!(tree.first_element(PAGE), 2, "the append already landed");
    assert_eq!(tree.tag(2), Some("view"));
}
