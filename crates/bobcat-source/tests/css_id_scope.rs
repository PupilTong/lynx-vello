//! Per-component CSS scoping by `css_id`, end to end.
//!
//! A hand-built `StyleInfo` with three fragments — 0 global, 1 and 2 scoped,
//! each importing 0 the way the compiler emits them
//! (lynx-stack `packages/tools/css-serializer/src/css/debundle.ts:64-146`) —
//! travels the real path: the `.web.bundle` decoder, `lower_style`'s guard
//! synthesis, `PageSource::register_with`, the view's first flush, stylo.
//! The card's main-thread script marks elements with `__SetCSSId` and reads
//! their computed style back through `__GetComputedStyleByKey`; any mismatch
//! throws, and a thrown script fails the boot this test pumps.
//!
//! What it pins is web-core's behavior (`style_info_decoder.rs:193-221`,
//! `element_apis/style_apis.rs:16-52`): a fragment's rules match only an
//! element carrying its id or an importer's, a global rule matches every
//! element, only the subject element's id matters for a complex selector, and
//! re-marking an element restyles it.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobcat_core::{DrawTarget, EngineEvent, LynxGroup, NoWakeup, Painter, StyleThreads};
use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use bobcat_source::web::SectionLabel;
use bobcat_source::web::style_info::{
    CssProperty, DeclarationBlock, ParsedDeclaration, Rule, RuleKind, RulePrelude, Selector,
    SimpleSelector, SimpleSelectorKind, StyleInfo, StyleSheet, ValueToken, token_types,
};
use url::Url;

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(100.0, 100.0, 1.0);

/// `property: <the value's tokens>`, as web-core's tokenizer splits
/// `rgb(…)`: a function token, its arguments, and the closing parenthesis.
fn rgb(property: &str, arguments: &str) -> ParsedDeclaration {
    let token = |token_type, value: &str| ValueToken {
        token_type,
        value: value.to_owned(),
    };
    ParsedDeclaration {
        property: CssProperty::from_name(property),
        value_tokens: vec![
            token(token_types::FUNCTION_TOKEN, "rgb("),
            token(token_types::IDENT_TOKEN, arguments),
            token(token_types::RIGHT_PARENTHESES_TOKEN, ")"),
        ],
        is_important: false,
    }
}

fn rule(components: &[(SimpleSelectorKind, &str)], declaration: ParsedDeclaration) -> Rule {
    Rule {
        kind: RuleKind::Style,
        prelude: RulePrelude {
            selectors: vec![Selector {
                components: components
                    .iter()
                    .map(|(kind, value)| SimpleSelector {
                        kind: *kind,
                        value: (*value).to_owned(),
                    })
                    .collect(),
            }],
        },
        declaration_block: DeclarationBlock {
            declarations: vec![declaration],
        },
        children: Vec::new(),
    }
}

/// Fragment 0: `.g { background-color: rgb(0,0,255) }`.
/// Fragment 1: `.basic { color: rgb(255,0,0) }`, `.a .b { color: rgb(1,1,1) }`.
/// Fragment 2: `.basic { color: rgb(0,128,0) }`, `.a .b { color: rgb(2,2,2) }`.
fn style_info() -> StyleInfo {
    use SimpleSelectorKind::{Class, Combinator};
    let scoped = |basic: &str, descendant: &str| StyleSheet {
        imports: vec![0],
        rules: vec![
            rule(&[(Class, "basic")], rgb("color", basic)),
            rule(
                &[(Class, "a"), (Combinator, " "), (Class, "b")],
                rgb("color", descendant),
            ),
        ],
    };
    StyleInfo {
        css_id_to_style_sheet: [
            (
                0,
                StyleSheet {
                    imports: Vec::new(),
                    rules: vec![rule(&[(Class, "g")], rgb("background-color", "0,0,255"))],
                },
            ),
            (1, scoped("255,0,0", "1,1,1")),
            (2, scoped("0,128,0", "2,2,2")),
        ]
        .into_iter()
        .collect(),
        style_text_size_hint: 0,
    }
}

/// The card: every check runs inside `renderPage`, between explicit flushes,
/// and throws on the first wrong value.
const CARD: &str = r"
globalThis.processData = (data) => data;
globalThis.updatePage = () => {};
globalThis.renderPage = () => {
  const RED = 'rgb(255, 0, 0)';
  const GREEN = 'rgb(0, 128, 0)';
  const BLUE = 'rgb(0, 0, 255)';
  const page = __CreatePage('0', 0);
  const make = (parent, classes, cssId) => {
    const element = __CreateView(0);
    __SetClasses(element, classes);
    if (cssId !== undefined) __SetCSSId([element], cssId);
    __AppendElement(parent, element);
    return element;
  };
  const color = (element) => __GetComputedStyleByKey(element, 'color');
  const background = (element) => __GetComputedStyleByKey(element, 'background-color');
  const expect = (what, actual, expected) => {
    if (actual !== expected) {
      throw new Error(what + ': expected ' + expected + ', got ' + actual);
    }
  };
  const expectNeither = (what, actual) => {
    if (actual === RED || actual === GREEN) {
      throw new Error(what + ': a scoped rule matched an unscoped element: ' + actual);
    }
  };

  const one = make(page, 'basic g', 1);
  const two = make(page, 'basic g', 2);
  const none = make(page, 'basic g');
  const outer = make(page, 'a', 1);
  const innerTwo = make(outer, 'b', 2);
  const innerOne = make(outer, 'b', 1);
  const innerNone = make(outer, 'b');
  __FlushElementTree();

  expect('fragment 1 on css id 1', color(one), RED);
  expect('fragment 2 on css id 2', color(two), GREEN);
  expectNeither('no css id', color(none));
  for (const [what, element] of [['id 1', one], ['id 2', two], ['no id', none]]) {
    expect('the global rule on ' + what, background(element), BLUE);
  }
  // Only the subject's id selects the fragment: the `.a` ancestor is id 1.
  expect('.a .b on a css id 2 subject', color(innerTwo), 'rgb(2, 2, 2)');
  expect('.a .b on a css id 1 subject', color(innerOne), 'rgb(1, 1, 1)');
  expectNeither('.a .b on an unscoped subject', color(innerNone));

  // Re-marking a styled element restyles it.
  __SetCSSId([one], 2);
  __FlushElementTree();
  expect('css id 1 re-marked 2', color(one), GREEN);

  // Css id 0 removes the mark, and with it the scoped rules.
  __SetCSSId([two], 0);
  __FlushElementTree();
  expectNeither('css id 2 cleared', color(two));
  expect('the global rule after clearing', background(two), BLUE);
  if (__GetAttributeByName(two, 'l-css-id') !== null) {
    throw new Error('css id 0 must remove l-css-id');
  }
};
";

fn push_section(bytes: &mut Vec<u8>, label: SectionLabel, content: &[u8]) {
    bytes.extend_from_slice(&(label as u32).to_le_bytes());
    bytes
        .extend_from_slice(&(u32::try_from(content.len()).expect("a small section")).to_le_bytes());
    bytes.extend_from_slice(content);
}

/// A `.web.bundle` whose `lepusCode.root` is `script` and whose `StyleInfo`
/// is `style_info`.
fn bundle(style_info: &StyleInfo, script: &str) -> Vec<u8> {
    let mut lepus = Vec::new();
    lepus.extend_from_slice(&1u32.to_le_bytes());
    for field in ["root", script] {
        lepus.extend_from_slice(
            &u32::try_from(field.len())
                .expect("a small field")
                .to_le_bytes(),
        );
        lepus.extend_from_slice(field.as_bytes());
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&bobcat_source::web::MAGIC_0.to_le_bytes());
    bytes.extend_from_slice(&bobcat_source::web::MAGIC_1.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    push_section(&mut bytes, SectionLabel::LepusCode, &lepus);
    push_section(
        &mut bytes,
        SectionLabel::StyleInfo,
        &rkyv::to_bytes::<_, 4096>(style_info).expect("the wire model serializes"),
    );
    bytes
}

#[tokio::test]
async fn css_id_selects_the_fragment_an_element_matches() {
    let input = Url::parse("app:///scoped.web.bundle").expect("a valid URL");
    let page =
        PageSource::from_bytes(&input, &bundle(&style_info(), CARD)).expect("the card decodes");
    let resources = Resources::new(ResourcesConfig::default(), || {});
    page.register_with(&resources);

    let group = LynxGroup::new(Arc::new(NoWakeup), StyleThreads::Sequential)
        .await
        .expect("a group");
    let mut view = group
        .create_lynx_view(
            100.0,
            100.0,
            1.0,
            resources.builder(),
            Vec::new(),
            page.view_sources(SCREEN),
        )
        .expect("a view");
    // Boot's first flush waits for a painter to bind the view.
    let mut painter = Painter::new(DrawTarget::Offscreen, 100.0, 100.0, 1.0)
        .await
        .expect("an offscreen painter");
    painter.attach(&view).expect("attach the painter");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut finished = false;
        for event in view.pump() {
            match event {
                EngineEvent::ScriptFinished => finished = true,
                EngineEvent::ConsoleMessage { level, message, .. } => {
                    eprintln!("[{level}] {message}");
                }
                other => panic!("the scoped card failed: {other:?}"),
            }
        }
        if finished {
            break;
        }
        assert!(Instant::now() < deadline, "the card did not finish booting");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
