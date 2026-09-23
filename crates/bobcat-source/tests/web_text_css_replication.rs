//! Replications of web-core's CSS encoder tests, run against this crate's
//! decoder and its lowering into the engine's pre-parsed stylesheet contract.
//!
//! Upstream these are *encoder* tests: they build a `StyleInfo` section out of
//! CSS text and assert that the emitted bytes exist, hash to a fixed digest, or
//! decode back to a fixed string. lynx-vello never produces that section — it
//! only consumes it — so every case inverts. What upstream proves the encoder
//! emits becomes the shape this decoder must accept and carry to the engine
//! without rewriting it.
//!
//! # What each case observes
//!
//! Two seams, both real:
//!
//! * the wire, through `bobcat_source::web::decode`, read by [`lowerable_rules`] through exactly
//!   the accessors `crates/bobcat-source/src/lower_style.rs` reads it through —
//!   `Selector::write_css_string` for a prelude (`lower_style.rs:140-152`),
//!   `ParsedDeclaration::value_and_importance` for a declaration (`lower_style.rs:154-172`),
//!   `ParsedDeclaration::write_value_text` for an `@font-face` descriptor
//!   (`lower_style.rs:174-190`). No test in this file re-serializes CSS itself: every asserted
//!   string is the return value of one production call on one decoded item.
//! * the engine's stylesheet hand-off, through [`registered_style_sheet`] —
//!   `PageSource::from_bytes` runs the whole lowering (`page.rs:262-270`) and
//!   `PageSource::register_with` installs the result in an embedder-owned `Resources` under the URL
//!   the card's `ViewSources` name, which is the URL `bobcat-core` asks for at view startup.
//!
//! # What is no longer covered here
//!
//! An earlier revision of this file read the lowered `PreparsedStyleSheet`
//! back and asserted concrete `PreparsedRule` values, by calling
//! `ViewResources::fetch_style_sheet` — the byte-fetch half of the resource
//! protocol. PR #221 removed that half. The registered sheet is now delivered
//! only through `ResourceFetcher::request_source`, whose `SourceCompletion` is
//! the sole channel and has no public constructor
//! (`bobcat-core/src/resource.rs:167-186`, both `new` and `over` are
//! `pub(crate)`), and `lower_style::to_preparsed_style_sheet` is `pub(crate)`
//! too. A test outside `bobcat-core` therefore cannot read the lowered rules at
//! all. What that costs this file, concretely, is every *block-level* step of
//! the lowering: `selector_text`'s `,` join across a selector list,
//! `descriptor_text`'s `:` / `;` joining and its empty-name skip,
//! `convert_rule`'s keyframe-child walk, and `fragment_order`'s import
//! ordering. Those are exercised only by `lower_style.rs`'s own `mod tests`.
//! Everything asserted below is the per-item input those steps consume.
//!
//! # Fixture provenance
//!
//! Every wire fixture below was derived by running the upstream authored CSS
//! through `@lynx-js/css-serializer` (`packages/tools/css-serializer`) and
//! `encodeCSS` (`packages/web-platform/web-core/ts/encode/encodeCSS.ts`), then
//! tokenizing the resulting value string the way web-core's own tokenizer does
//! (`packages/web-platform/web-core/src/css_tokenizer/tokenize.rs`,
//! `ParsedDeclaration::new` at
//! `.../template_sections/style_info/css_property.rs:587-597`). Three
//! consequences are not obvious from the authored CSS and are relied on here:
//!
//! * `@lynx-js/css-serializer` re-emits every `url()` **single-quoted**, so `src:
//!   url("myfont.woff")` reaches the wire as `url('myfont.woff')` and `src: url(a.woff2)` as
//!   `url('a.woff2')`. A quoted `url()` is a `<function-token>` in CSS syntax, never a
//!   `<url-token>` (`tokenize.rs:56-83`), so it rides as FUNCTION + STRING + `)`.
//! * `restoreCSSVarValue` runs on the *encoder* side (`encodeCSS.ts:26`), so a `var()` and its
//!   fallback reach the wire as ordinary value tokens, with the `, ` between name and fallback
//!   inserted by that function's template literal (`encodeCSS.ts:19-21`). Nothing here has to
//!   restore a `{{--name}}` interchange placeholder; the web path's job is to leave the tokens
//!   alone.
//! * `encodeCSS` pushes a rule's declarations **before** its custom properties
//!   (`encodeCSS.ts:153-162`), so `:root{--accent:#f00;font-size:14px}` arrives with `font-size`
//!   first.
//!
//! # Upstream artifacts deliberately dropped
//!
//! Three belong to the browser decoder rather than to the format: the
//! `:not([l-e-name])` suffix web-core appends to every rule, its
//! double-bracketed re-emission of an attribute selector
//! (`style_info_decoder.rs:96`, `:205-215`), and the separate `fontFace`
//! output channel. This DOM has no shared browser document to isolate one card
//! inside, and its lowering keeps every rule kind in one sheet.
//!
//! # Halves that live in other crates
//!
//! Two assertions the assignment asked for are not expressible from this
//! crate and are **not** claimed here:
//!
//! * the computed value of `color: var(--X, rgba(22,24,35,.6))` with `--X` unset belongs in
//!   `crates/dom/tests/custom_properties.rs`;
//! * what stylo does with a `var()` inside an `@font-face` descriptor, and the
//!   `DescriptorId::FontFamily` / `DescriptorId::Src` readback, belong in
//!   `crates/dom/tests/at_rules.rs` — which today has no `var()` case at all.
//!
//! Both are stated in the relevant case's doc comment rather than asserted.
//!
//! # `@font-face`: nine green cases that stop at the wire
//!
//! All nine cases in this file pass, six of them touching `@font-face` — the
//! rule kind, the `src: url()` round trip, descriptor order and form, the
//! `var()` fallback, the encoder corpus, and the group-at-rule survival case.
//! Every one of them is correct about what it observes: the decoder accepts
//! the section web-core's
//! encoder emits, and the lowering hands each descriptor to the engine's
//! stylesheet contract unrewritten. None of them observes anything downstream
//! of that hand-off, and downstream the path ends. The rule parses into a real
//! stylo `FontFaceRule` (`crates/dom/src/style/engine.rs:417-426`) and enters
//! the cascade, but no reader of that variant exists, no `src:` URL is ever
//! fetched, and a face reaches shaping only as embedder-supplied bytes through
//! `Document::register_fonts` (`crates/dom/src/layout/mod.rs:192`). A card
//! that ships its own face and names it therefore renders in the default
//! family. That gap is carried by
//! `a_font_face_declared_family_shapes_the_text_that_names_it` in
//! `crates/dom/tests/web_text_replication.rs`, which asserts the shaped
//! advance a browser gives web-core and is `#[ignore]`d on the gap. Read the
//! green count below as "the wire and the lowering", never as "`@font-face`
//! works".

use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use bobcat_source::web::style_info::{
    CssProperty, DeclarationBlock, ParsedDeclaration, Rule, RuleKind, RulePrelude, Selector,
    SimpleSelector, SimpleSelectorKind, StyleInfo, StyleSheet, ValueToken, token_types,
};
use bobcat_source::web::{SectionLabel, decode};
use url::Url;

/// The screen these tests' views report, as a host with no screen to measure
/// names it. None of them reads `SystemInfo`.
const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

// ---------------------------------------------------------------------------
// Building a `StyleInfo` the way the lynx-stack encoder would
// ---------------------------------------------------------------------------

/// A declaration whose value arrived as a single token of `token_type`.
fn one_token(property: &str, token_type: u8, value: &str) -> ParsedDeclaration {
    tokenized(property, &[(token_type, value)])
}

/// A declaration whose value is a bare identifier (`red`, `X`, `important`).
fn ident(property: &str, value: &str) -> ParsedDeclaration {
    one_token(property, token_types::IDENT_TOKEN, value)
}

/// A declaration whose value is a dimension (`14px`, `10rpx`, `50vh`).
fn dimension(property: &str, value: &str) -> ParsedDeclaration {
    one_token(property, token_types::DIMENSION_TOKEN, value)
}

/// A declaration whose value is a bare `url()` with a quoted argument.
///
/// `@lynx-js/css-serializer` single-quotes every `url()` it re-emits, and a
/// quoted `url()` tokenizes as `<function-token>` `<string-token>` `)` — never
/// as a `<url-token>` (`css_tokenizer/tokenize.rs:56-83`).
fn quoted_url(property: &str, path: &str) -> ParsedDeclaration {
    tokenized(
        property,
        &[
            (token_types::FUNCTION_TOKEN, "url("),
            (token_types::STRING_TOKEN, &format!("'{path}'")),
            (token_types::RIGHT_PARENTHESES_TOKEN, ")"),
        ],
    )
}

/// A declaration whose value is `var(--name)` with no fallback, the form
/// `restoreCSSVarPlaceholders` produces from `{{--name}}` when the encoder
/// recorded no default (`encodeCSS.ts:16-22`).
fn var_no_fallback(property: &str, name: &str) -> ParsedDeclaration {
    tokenized(
        property,
        &[
            (token_types::FUNCTION_TOKEN, "var("),
            (token_types::IDENT_TOKEN, name),
            (token_types::RIGHT_PARENTHESES_TOKEN, ")"),
        ],
    )
}

/// A declaration that keeps the value's original token split.
///
/// The split is load-bearing: `!important` rides the wire as ordinary value
/// tokens rather than as the `is_important` flag, and it is the token *types*
/// — a `!` delim followed by the `important` ident — that let the decoder tell
/// the marker apart from a value that merely ends in that word.
fn tokenized(property: &str, tokens: &[(u8, &str)]) -> ParsedDeclaration {
    ParsedDeclaration {
        property: CssProperty::from_name(property),
        value_tokens: tokens
            .iter()
            .map(|(token_type, value)| ValueToken {
                token_type: *token_type,
                value: (*value).to_owned(),
            })
            .collect(),
        is_important: false,
    }
}

fn selector(components: &[(SimpleSelectorKind, &str)]) -> Selector {
    Selector {
        components: components
            .iter()
            .map(|(kind, value)| SimpleSelector {
                kind: *kind,
                value: (*value).to_owned(),
            })
            .collect(),
    }
}

fn style_rule(
    components: &[(SimpleSelectorKind, &str)],
    declarations: Vec<ParsedDeclaration>,
) -> Rule {
    Rule {
        kind: RuleKind::Style,
        prelude: RulePrelude {
            selectors: vec![selector(components)],
        },
        declaration_block: DeclarationBlock { declarations },
        children: Vec::new(),
    }
}

/// A font-face rule: an empty prelude by construction, descriptors in the
/// declaration block.
fn font_face_rule(declarations: Vec<ParsedDeclaration>) -> Rule {
    Rule {
        kind: RuleKind::FontFace,
        prelude: RulePrelude {
            selectors: Vec::new(),
        },
        declaration_block: DeclarationBlock { declarations },
        children: Vec::new(),
    }
}

/// A keyframes rule: the animation name is a lone `UnknownText` prelude
/// component and each block is a child rule keyed by its own selector text.
fn keyframes_rule(name: &str, blocks: Vec<(&str, Vec<ParsedDeclaration>)>) -> Rule {
    Rule {
        kind: RuleKind::Keyframes,
        prelude: RulePrelude {
            selectors: vec![selector(&[(SimpleSelectorKind::UnknownText, name)])],
        },
        declaration_block: DeclarationBlock {
            declarations: Vec::new(),
        },
        children: blocks
            .into_iter()
            .map(|(key, declarations)| {
                style_rule(&[(SimpleSelectorKind::UnknownText, key)], declarations)
            })
            .collect(),
    }
}

/// A one-fragment `StyleInfo` under `css_id` 0, the shape every bundle built
/// with `enableRemoveCSSScope` has.
fn style_info(rules: Vec<Rule>) -> StyleInfo {
    style_info_at(0, rules)
}

fn style_info_at(css_id: i32, rules: Vec<Rule>) -> StyleInfo {
    StyleInfo {
        css_id_to_style_sheet: [(
            css_id,
            StyleSheet {
                imports: Vec::new(),
                rules,
            },
        )]
        .into_iter()
        .collect(),
        style_text_size_hint: 0,
    }
}

// ---------------------------------------------------------------------------
// Expected rules, in the terms `lower_style` reads a decoded rule in
// ---------------------------------------------------------------------------

/// One decoded rule, read through the production accessors
/// `lower_style::convert_rule` reads it through.
///
/// The shape mirrors `PreparsedRule`, one variant per `RuleKind`, with two
/// deliberate differences: a selector list stays a list rather than a joined
/// string, and a descriptor block stays a list of descriptors rather than
/// joined text, because those two joins are what this file can no longer
/// observe (see the module doc). Nothing here formats CSS — each string is the
/// return value of one production call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lowerable {
    Style {
        selectors: Vec<String>,
        declarations: Vec<Declaration>,
    },
    FontFace {
        descriptors: Vec<(String, String)>,
    },
    Keyframes {
        name: Vec<String>,
        blocks: Vec<(Vec<String>, Vec<Declaration>)>,
    },
}

/// A declaration as `lower_style::declarations` builds a `PreparsedDeclaration`
/// from it: the property name, and the `(value, important)` pair
/// `ParsedDeclaration::value_and_importance` recovers from the value tokens.
type Declaration = (String, String, bool);

fn declaration(property: &str, value: &str) -> Declaration {
    (property.to_owned(), value.to_owned(), false)
}

fn important(property: &str, value: &str) -> Declaration {
    (property.to_owned(), value.to_owned(), true)
}

fn expect_style(selectors: &[&str], declarations: Vec<Declaration>) -> Lowerable {
    Lowerable::Style {
        selectors: selectors.iter().map(|text| (*text).to_owned()).collect(),
        declarations,
    }
}

fn expect_font_face(descriptors: &[(&str, &str)]) -> Lowerable {
    Lowerable::FontFace {
        descriptors: descriptors
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn expect_keyframes(name: &str, blocks: Vec<(&str, Vec<Declaration>)>) -> Lowerable {
    Lowerable::Keyframes {
        name: vec![name.to_owned()],
        blocks: blocks
            .into_iter()
            .map(|(selector, declarations)| (vec![selector.to_owned()], declarations))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Wrapping it in a bundle and reading it back out
// ---------------------------------------------------------------------------

fn push_section(bytes: &mut Vec<u8>, label: SectionLabel, content: &[u8]) {
    bytes.extend_from_slice(&(label as u32).to_le_bytes());
    bytes
        .extend_from_slice(&(u32::try_from(content.len()).expect("a small section")).to_le_bytes());
    bytes.extend_from_slice(content);
}

/// A minimal `.web.bundle` carrying `style_info`.
///
/// The `LepusCode` section is present only because a card without a
/// `lepusCode.root` entry is not a page; the tests here never run it.
fn bundle(style_info: &StyleInfo) -> Vec<u8> {
    let mut lepus = Vec::new();
    lepus.extend_from_slice(&1u32.to_le_bytes());
    for field in ["root", "// no card script\n"] {
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

/// Round-trips `style_info` through the real bundle decoder, rkyv validation
/// included.
fn decoded(style_info: &StyleInfo) -> StyleInfo {
    decode(&bundle(style_info))
        .expect("the bundle decodes")
        .style_info
        .expect("the StyleInfo section is present")
}

fn fragment(style_info: &StyleInfo, css_id: i32) -> &[Rule] {
    &style_info.css_id_to_style_sheet[&css_id].rules
}

/// A rule's declaration block, in source order, read the way
/// `lower_style::declarations` reads it (`lower_style.rs:154-172`).
///
/// `value_and_importance` rather than `value_text` on purpose: the wire leaves
/// `is_important` false and appends the marker to the value tokens, and
/// splitting it back out is the only computation the lowering performs per
/// declaration.
fn declarations(rule: &Rule) -> Vec<Declaration> {
    rule.declaration_block
        .declarations
        .iter()
        .map(|declaration| {
            let (value, important) = declaration.value_and_importance();
            (declaration.property.name().to_owned(), value, important)
        })
        .collect()
}

/// A font-face rule's descriptors, read the way `lower_style::descriptor_text`
/// reads them (`lower_style.rs:174-190`).
///
/// A descriptor keeps a trailing `!important` in its text — that is what makes
/// stylo reject the descriptor, as a browser does — so this is
/// `write_value_text`, not `value_and_importance`.
fn descriptors(rule: &Rule) -> Vec<(String, String)> {
    assert_eq!(rule.kind, RuleKind::FontFace, "descriptors of a font-face");
    rule.declaration_block
        .declarations
        .iter()
        .map(|declaration| {
            let mut value = String::new();
            declaration.write_value_text(&mut value);
            (declaration.property.name().to_owned(), value)
        })
        .collect()
}

/// A prelude's selectors, each through `Selector::write_css_string` — the
/// production call `lower_style::selector_text` makes once per selector
/// (`lower_style.rs:140-152`).
fn selectors(prelude: &RulePrelude) -> Vec<String> {
    prelude
        .selectors
        .iter()
        .map(Selector::to_css_string)
        .collect()
}

/// The card a bundle carrying `style_info` becomes.
fn page_source(style_info: &StyleInfo) -> PageSource {
    let url = Url::parse("bobcat-test://card/app.web.bundle").expect("a valid URL");
    PageSource::from_bytes(&url, &bundle(style_info)).expect("the card decodes")
}

/// Every rule of fragment `css_id`, after a real bundle round trip, read
/// through the accessors `lower_style::convert_rule` reads them through.
fn lowerable_rules(style_info: &StyleInfo, css_id: i32) -> Vec<Lowerable> {
    fragment(&decoded(style_info), css_id)
        .iter()
        .map(lowerable)
        .collect()
}

fn lowerable(rule: &Rule) -> Lowerable {
    match rule.kind {
        RuleKind::Style => Lowerable::Style {
            selectors: selectors(&rule.prelude),
            declarations: declarations(rule),
        },
        RuleKind::FontFace => {
            assert!(
                rule.prelude.selectors.is_empty(),
                "a font-face rule has no prelude"
            );
            Lowerable::FontFace {
                descriptors: descriptors(rule),
            }
        }
        RuleKind::Keyframes => Lowerable::Keyframes {
            name: selectors(&rule.prelude),
            blocks: rule
                .children
                .iter()
                .map(|block| (selectors(&block.prelude), declarations(block)))
                .collect(),
        },
    }
}

// ---------------------------------------------------------------------------
// The engine-side seam
// ---------------------------------------------------------------------------

/// The author stylesheet URL a card carrying `style_info` hands its view, and
/// `None` when the lowering produced no rules at all.
///
/// `PageSource::from_bytes` runs the production lowering — `web::decode` ->
/// `lower_style::to_preparsed_style_sheet` -> the `!sheet.is_empty()` filter at
/// `page.rs:266` — and `register_with` installs the result in an
/// embedder-owned `Resources` under this URL, which is what `bobcat-core` asks
/// for at view startup. The `unregister` below is the proof the registration
/// actually landed there.
///
/// The sheet's *contents* are not readable from this crate; see the module
/// doc. What this reports is that the card lowered to a non-empty sheet and
/// that the sheet reached the resource system under the advertised URL.
fn registered_style_sheet(style_info: &StyleInfo) -> Option<String> {
    let card = page_source(style_info);
    let resources = Resources::new(
        ResourcesConfig {
            worker_threads: 1,
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || {},
    );
    card.register_with(&resources);

    let url = card.view_sources(SCREEN).style_sheets.first().cloned();
    if let Some(url) = url.as_deref() {
        assert!(
            resources.unregister(url),
            "the lowered sheet is registered under the URL the view asks for"
        );
    }
    url
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// Replicates `web-core/encode/raw-style-info-font-face-rule` (inline fixture,
/// `packages/web-platform/web-core/tests/encode.spec.ts:42`): the `StyleInfo`
/// format has a dedicated font-face rule kind, and a `font-family` descriptor
/// pushed under it survives the container.
///
/// Upstream stops at "the encoded buffer is non-empty". The decoder's
/// equivalent claim is that the rule reaches the engine, so this follows it to
/// the descriptor `lower_style::descriptor_text` reads, and on to the sheet
/// registration the view is handed.
///
/// The fixture's `cssId` is 1, not 0, which upstream is incidental and here is
/// not: a non-zero fragment id is component-scoped CSS, which this engine
/// mounts globally and reports as a compatibility warning.
#[test]
fn raw_style_info_carries_a_font_face_rule_kind() {
    let info = style_info_at(
        1,
        vec![font_face_rule(vec![ident("font-family", "MyFont")])],
    );

    assert_eq!(
        lowerable_rules(&info, 1),
        vec![expect_font_face(&[("font-family", "MyFont")])]
    );
    assert!(
        registered_style_sheet(&info).is_some(),
        "the rule reaches the engine as a registered sheet"
    );

    let warnings = page_source(&info).compatibility_warnings().to_vec();
    assert_eq!(
        warnings.len(),
        1,
        "fragment 1 is component-scoped and is mounted globally: {warnings:?}"
    );
}

/// Replicates `web-core/encode/encode-css-font-face-rule` (inline fixture,
/// `packages/web-platform/web-core/tests/encode.spec.ts:80`): `@font-face`
/// survives the CSS-string path into `StyleInfo`, rather than being dropped the
/// way `@media`, `@supports` and `@layer` are.
///
/// That last half is structural rather than assertable: `RuleKind` has exactly
/// three variants — `Style`, `Keyframes`, `FontFace` — so a conditional group
/// rule has no representation on this wire to begin with, and `encodeCSS`
/// simply skips those node types (`encodeCSS.ts:47-165`).
///
/// The `src` descriptor rides in the token split web-core's tokenizer actually
/// produces for a quoted `url()`, because a descriptor value is only faithful
/// if reassembling those tokens reproduces the authored text.
#[test]
fn font_face_with_a_src_url_survives_the_bundle() {
    let info = style_info(vec![font_face_rule(vec![
        one_token("font-family", token_types::STRING_TOKEN, "\"MyFont\""),
        quoted_url("src", "myfont.woff"),
    ])]);

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![expect_font_face(&[
            ("font-family", "\"MyFont\""),
            ("src", "url('myfont.woff')"),
        ])]
    );
    assert!(registered_style_sheet(&info).is_some());
}

/// Replicates `web-core/encode/font-family-at-rule-snapshot` (inline fixture,
/// `packages/web-platform/web-core/tests/encode.spec.ts:329`): the exact
/// content of a decoded `@font-face` rule — both descriptors, in source order,
/// with their quoting untouched.
///
/// The upstream snapshot is `@font-face{font-family:"MyFont";src:url('myfont.woff');}`.
/// Its asymmetric quoting is not a decoder normalisation: `@lynx-js/css-serializer`
/// re-emits the `url()` single-quoted before the value ever reaches the wire
/// (verified by running the authored CSS through
/// `packages/tools/css-serializer`), so the decoder's job is to leave both
/// forms exactly as they arrived. `lower_style::descriptor_text` is this
/// engine's `get_font_face_content`; its per-descriptor value text is what is
/// asserted, descriptor by descriptor. The `name:value;` joining that turns
/// those into the snapshot's inner text is not observable from this crate (see
/// the module doc), so what this pins is the two halves that joining reads,
/// and their order. Upstream's separate output channel collapses into the one
/// sheet here.
///
/// The stylo half of the assignment — `DescriptorId::FontFamily == "MyFont"`
/// and `DescriptorId::Src` naming `myfont.woff`, through the `font_face_descriptor`
/// helper at `crates/dom/tests/at_rules.rs:49` — is *not* asserted here and has
/// no case in that file today.
#[test]
fn font_face_descriptors_decode_in_authored_order_and_form() {
    let info = style_info(vec![font_face_rule(vec![
        one_token("font-family", token_types::STRING_TOKEN, "\"MyFont\""),
        quoted_url("src", "myfont.woff"),
    ])]);

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![expect_font_face(&[
            ("font-family", "\"MyFont\""),
            ("src", "url('myfont.woff')"),
        ])],
        "the descriptors are the snapshot's, in the snapshot's order"
    );
    assert!(registered_style_sheet(&info).is_some());
}

/// Replicates `web-core/encode/font-face-preserves-css-var-fallback` (inline
/// fixture, `packages/web-platform/web-core/tests/encode.spec.ts:345`): a
/// `var()` with a fallback used as an `@font-face` descriptor value reaches the
/// engine verbatim — neither flattened to its fallback nor left as the
/// `{{--font-family}}` interchange placeholder — and the space after the
/// top-level comma survives with it.
///
/// Upstream this is one of exactly two fixtures that catch a `restoreCSSVarValue`
/// regression in the `FontFaceRule` branch; the equivalent obligation on this side
/// is that neither the decoder nor `descriptor_text` renormalises the text.
///
/// What stylo then makes of that descriptor is the other end of the same case,
/// pinned by `font_face_descriptors_do_not_substitute_var` in
/// `crates/dom/tests/at_rules.rs`: css-variables-1 §3 limits `var()`
/// substitution to property values inside a declaration block, so the
/// descriptor carrying one is dropped while its siblings parse. That is the
/// browser's behaviour too, so it is the reference — not a gap.
#[test]
fn font_face_var_fallback_decodes_verbatim() {
    let info = style_info(vec![font_face_rule(vec![
        tokenized(
            "font-family",
            &[
                (token_types::FUNCTION_TOKEN, "var("),
                (token_types::IDENT_TOKEN, "--font-family"),
                (token_types::COMMA_TOKEN, ","),
                (token_types::WHITESPACE_TOKEN, " "),
                (token_types::STRING_TOKEN, "\"MyFont\""),
                (token_types::RIGHT_PARENTHESES_TOKEN, ")"),
            ],
        ),
        quoted_url("src", "myfont.woff"),
    ])]);

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![expect_font_face(&[
            ("font-family", "var(--font-family, \"MyFont\")"),
            ("src", "url('myfont.woff')"),
        ])]
    );
    assert!(registered_style_sheet(&info).is_some());
}

/// Replicates `web-core/encode/preserve-css-var-fallback-for-color` (inline
/// fixture, `packages/web-platform/web-core/tests/encode.spec.ts:202`): a
/// `var()` fallback on `color` reaches the engine with its number forms and
/// whitespace exactly as authored — the alpha stays `.6` rather than being
/// rewritten to `0.6`, the fallback's inner spaces stay absent, and the one
/// space after the top-level comma stays present.
///
/// The upstream assertion is on the decoded style content; the counterpart here
/// is the declaration value `lower_style` hands the engine, which is what stylo
/// parses.
///
/// The assignment's primary assertion for this row — that with
/// `--Text-TextQuaternary` unset the computed colour *is* `rgba(22,24,35,0.6)`
/// — belongs in `crates/dom/tests/custom_properties.rs`; `bobcat-source` cannot
/// depend on `dom`, and no computed value is asserted in this file.
#[test]
fn color_var_fallback_decodes_verbatim() {
    let info = style_info(vec![style_rule(
        &[(SimpleSelectorKind::Class, "foo")],
        vec![tokenized(
            "color",
            &[
                (token_types::FUNCTION_TOKEN, "var("),
                (token_types::IDENT_TOKEN, "--Text-TextQuaternary"),
                (token_types::COMMA_TOKEN, ","),
                (token_types::WHITESPACE_TOKEN, " "),
                (token_types::FUNCTION_TOKEN, "rgba("),
                (token_types::NUMBER_TOKEN, "22"),
                (token_types::COMMA_TOKEN, ","),
                (token_types::NUMBER_TOKEN, "24"),
                (token_types::COMMA_TOKEN, ","),
                (token_types::NUMBER_TOKEN, "35"),
                (token_types::COMMA_TOKEN, ","),
                (token_types::NUMBER_TOKEN, ".6"),
                (token_types::RIGHT_PARENTHESES_TOKEN, ")"),
                (token_types::RIGHT_PARENTHESES_TOKEN, ")"),
            ],
        )],
    )]);

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![expect_style(
            &[".foo"],
            vec![declaration(
                "color",
                "var(--Text-TextQuaternary, rgba(22,24,35,.6))"
            )]
        )]
    );
    assert!(registered_style_sheet(&info).is_some());
}

/// Replicates `web-core/encode/non-ascii-css-content-string` (inline fixture,
/// `packages/web-platform/web-core/tests/encode.spec.ts:465`): UTF-8 text
/// inside a CSS string survives the container byte for byte, and the fixture's
/// whole selector — class, attribute selector and `:before` — comes back
/// unchanged.
///
/// `:before` really is a pseudo-*class* component on this wire: `encodeCSS`
/// takes csstree's node type verbatim, and csstree parses the single-colon form
/// as `PseudoClassSelector` (`encodeCSS.ts:126-133`), which is why the upstream
/// snapshot shows one colon while `::before` in the `selectors` corpus entry
/// keeps two.
///
/// The upstream snapshot also pins `[[data-status="complete"]]` and a
/// `:not([l-e-name])` suffix; both are rewrites web-core's browser decoder adds
/// and neither exists here. `content:` is itself inert in this engine — no
/// generated-content boxes are produced — so what this pins is value fidelity,
/// not rendering.
#[test]
fn non_ascii_content_string_decodes_byte_exactly() {
    let info = style_info(vec![style_rule(
        &[
            (SimpleSelectorKind::Class, "class145"),
            (SimpleSelectorKind::Attribute, "[data-status=\"complete\"]"),
            (SimpleSelectorKind::PseudoClass, "before"),
        ],
        vec![one_token(
            "content",
            token_types::STRING_TOKEN,
            "\"\u{2713} \"",
        )],
    )]);
    let decoded = decoded(&info);
    let value = &declarations(&fragment(&decoded, 0)[0])[0].1;
    assert_eq!(value.as_bytes(), "\"\u{2713} \"".as_bytes());

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![expect_style(
            &[".class145[data-status=\"complete\"]:before"],
            vec![declaration("content", "\"\u{2713} \"")]
        )]
    );
    assert!(registered_style_sheet(&info).is_some());
}

/// Replicates `web-core/encode-style-path/byte-for-byte-fingerprints` (inline
/// corpus, `packages/web-platform/web-core/tests/encode-style-path.spec.ts:62`):
/// the seven text- and font-relevant members of the encoder's stability corpus.
///
/// Upstream the assertion is a sha256 pin on the bytes a JavaScript build step
/// emits, which has no counterpart in an engine that never emits them. What
/// ports is the corpus itself, read as decode inputs: each stylesheet must reach
/// the engine as the rules it went in as. `fontFace` and `fontFaceVar` are the
/// two entries that exist upstream only to catch a dropped `restoreCSSVarValue`
/// in the font-face branch, so a `var()` inside a descriptor is what they check
/// here too.
///
/// Three shapes below are the encoder's rather than the author's, and were
/// derived by running each stylesheet through `@lynx-js/css-serializer`: the
/// corpus writes `{{--fw}}` and `{{--c}}` and the encoder rewrites both to
/// `var(...)`; `url(a.woff2)` is re-emitted single-quoted; and `:root`'s custom
/// property is pushed *after* its ordinary declarations. Combinators come back
/// spaced (`.b > .c`) because the wire stores each one as its own component
/// with no surrounding text; the selector is the same selector.
#[test]
fn encoder_corpus_stylesheets_decode_to_their_rules() {
    let corpus = text_and_font_corpus();
    for (name, info, expected) in &corpus {
        assert_eq!(&lowerable_rules(info, 0), expected, "corpus entry `{name}`");
        assert!(
            registered_style_sheet(info).is_some(),
            "corpus entry `{name}` reaches the engine as a registered sheet"
        );
    }

    // The corpus `important` entry only earns its place if the marker is
    // recovered rather than carried into a value parser: the wire leaves
    // `is_important` false and appends the marker to the value tokens instead.
    let important = decoded(&corpus[1].1);
    let width = &fragment(&important, 0)[0].declaration_block.declarations[0];
    assert!(!width.is_important, "the wire flag stays false");
    assert_eq!(
        width.value_and_importance(),
        ("100vw".to_owned(), true),
        "the marker is split back out of the value tokens"
    );
}

/// Each corpus stylesheet as the wire carries it, paired with the rules it must
/// reach the engine as.
#[expect(
    clippy::too_many_lines,
    reason = "one literal entry per corpus stylesheet; splitting the table hides the corpus"
)]
fn text_and_font_corpus() -> Vec<(&'static str, StyleInfo, Vec<Lowerable>)> {
    vec![
        (
            // ':root{--accent:#f00;font-size:14px}.t{color:var(--accent)}'
            "variables",
            style_info(vec![
                style_rule(
                    &[(SimpleSelectorKind::PseudoClass, "root")],
                    vec![
                        dimension("font-size", "14px"),
                        one_token("--accent", token_types::HASH_TOKEN, "#f00"),
                    ],
                ),
                style_rule(
                    &[(SimpleSelectorKind::Class, "t")],
                    vec![var_no_fallback("color", "--accent")],
                ),
            ]),
            vec![
                expect_style(
                    &[":root"],
                    vec![
                        declaration("font-size", "14px"),
                        declaration("--accent", "#f00"),
                    ],
                ),
                expect_style(&[".t"], vec![declaration("color", "var(--accent)")]),
            ],
        ),
        (
            // '.a{width:100vw !important;height:50vh;font-size:10rpx}'
            "important",
            style_info(vec![style_rule(
                &[(SimpleSelectorKind::Class, "a")],
                vec![
                    tokenized(
                        "width",
                        &[
                            (token_types::DIMENSION_TOKEN, "100vw"),
                            (token_types::WHITESPACE_TOKEN, " "),
                            (token_types::DELIM_TOKEN, "!"),
                            (token_types::IDENT_TOKEN, "important"),
                        ],
                    ),
                    dimension("height", "50vh"),
                    dimension("font-size", "10rpx"),
                ],
            )]),
            vec![expect_style(
                &[".a"],
                vec![
                    important("width", "100vw"),
                    declaration("height", "50vh"),
                    declaration("font-size", "10rpx"),
                ],
            )],
        ),
        (
            // '@font-face{font-family:X;src:url(a.woff2)}'
            "fontFace",
            style_info(vec![font_face_rule(vec![
                ident("font-family", "X"),
                quoted_url("src", "a.woff2"),
            ])]),
            vec![expect_font_face(&[
                ("font-family", "X"),
                ("src", "url('a.woff2')"),
            ])],
        ),
        (
            // '@font-face{font-family:X;src:url(a.woff2);font-weight:{{--fw}}}'
            "fontFaceVar",
            style_info(vec![font_face_rule(vec![
                ident("font-family", "X"),
                quoted_url("src", "a.woff2"),
                var_no_fallback("font-weight", "--fw"),
            ])]),
            vec![expect_font_face(&[
                ("font-family", "X"),
                ("src", "url('a.woff2')"),
                ("font-weight", "var(--fw)"),
            ])],
        ),
        (
            // '@keyframes k{from{color:{{--c}}}to{color:blue}}'
            "keyframesVar",
            style_info(vec![keyframes_rule(
                "k",
                vec![
                    ("from", vec![var_no_fallback("color", "--c")]),
                    ("to", vec![ident("color", "blue")]),
                ],
            )]),
            vec![expect_keyframes(
                "k",
                vec![
                    ("from", vec![declaration("color", "var(--c)")]),
                    ("to", vec![declaration("color", "blue")]),
                ],
            )],
        ),
        (
            // '.a{color:red;padding:1rem}'
            "plain",
            style_info(vec![style_rule(
                &[(SimpleSelectorKind::Class, "a")],
                vec![ident("color", "red"), dimension("padding", "1rem")],
            )]),
            vec![expect_style(
                &[".a"],
                vec![declaration("color", "red"), declaration("padding", "1rem")],
            )],
        ),
        (
            // '.a .b>.c+.d~.e{color:red}#id[data-x="1"]:hover::before{color:blue}
            //  *{margin:0}div{padding:0}'
            //
            // `*` arrives as a Type component whose value is `*`: csstree
            // parses it as a TypeSelector, and `encodeCSS` forwards that node
            // type verbatim, so the wire's UniversalSelector kind is never
            // produced by this encoder.
            "selectors",
            style_info(vec![
                style_rule(
                    &[
                        (SimpleSelectorKind::Class, "a"),
                        (SimpleSelectorKind::Combinator, " "),
                        (SimpleSelectorKind::Class, "b"),
                        (SimpleSelectorKind::Combinator, ">"),
                        (SimpleSelectorKind::Class, "c"),
                        (SimpleSelectorKind::Combinator, "+"),
                        (SimpleSelectorKind::Class, "d"),
                        (SimpleSelectorKind::Combinator, "~"),
                        (SimpleSelectorKind::Class, "e"),
                    ],
                    vec![ident("color", "red")],
                ),
                style_rule(
                    &[
                        (SimpleSelectorKind::Id, "id"),
                        (SimpleSelectorKind::Attribute, "[data-x=\"1\"]"),
                        (SimpleSelectorKind::PseudoClass, "hover"),
                        (SimpleSelectorKind::PseudoElement, "before"),
                    ],
                    vec![ident("color", "blue")],
                ),
                style_rule(
                    &[(SimpleSelectorKind::Type, "*")],
                    vec![one_token("margin", token_types::NUMBER_TOKEN, "0")],
                ),
                style_rule(
                    &[(SimpleSelectorKind::Type, "div")],
                    vec![one_token("padding", token_types::NUMBER_TOKEN, "0")],
                ),
            ]),
            vec![
                expect_style(&[".a .b > .c + .d ~ .e"], vec![declaration("color", "red")]),
                expect_style(
                    &["#id[data-x=\"1\"]:hover::before"],
                    vec![declaration("color", "blue")],
                ),
                expect_style(&["*"], vec![declaration("margin", "0")]),
                expect_style(&["div"], vec![declaration("padding", "0")]),
            ],
        ),
    ]
}

/// Replicates `web-core/xml-to-web-bundle/keyframes-and-font-face-survive`
/// (inline fixture,
/// `packages/web-platform/web-core/tests/xml-to-web-bundle.spec.ts:373`):
/// `@keyframes` and `@font-face` are the two group at-rule kinds a Lynx card can
/// carry, and a card declaring both loses nothing.
///
/// Upstream the card is a Vanilla-Lynx-XML envelope *compiled to a bundle*, and
/// the assertion reads the decoded `StyleInfo` back out of that bundle — so the
/// faithful port is the bundle path, with both group kinds side by side in one
/// fragment. Taking the raw Lynx-XML path instead would test nothing: there a
/// `<style>` body is stored unparsed as `PageStyleSheet::Text`
/// (`crates/bobcat-source/src/page.rs:299`) and never becomes rules, and
/// `from_lynx_xml` hard-codes an empty warning list (`page.rs:307`), so
/// upstream's `expect(discarded).toStrictEqual([])` has nothing to bite on.
/// On the bundle path that field is live (`page.rs:271-278`).
///
/// The web's two decoded channels — `css` and `fontFace` — collapse into one
/// sheet here, so the "separate channel" half of the upstream assertion is
/// dropped; what remains is that both rules survive with their contents.
#[test]
fn a_card_carries_keyframes_and_font_face() {
    let info = style_info(vec![
        keyframes_rule(
            "spin",
            vec![
                (
                    "from",
                    vec![one_token("opacity", token_types::NUMBER_TOKEN, "0")],
                ),
                (
                    "to",
                    vec![one_token("opacity", token_types::NUMBER_TOKEN, "1")],
                ),
            ],
        ),
        font_face_rule(vec![
            ident("font-family", "CardFont"),
            quoted_url("src", "a.woff2"),
        ]),
    ]);

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![
            expect_keyframes(
                "spin",
                vec![
                    ("from", vec![declaration("opacity", "0")]),
                    ("to", vec![declaration("opacity", "1")]),
                ],
            ),
            expect_font_face(&[("font-family", "CardFont"), ("src", "url('a.woff2')"),]),
        ]
    );
    assert!(registered_style_sheet(&info).is_some());

    assert!(
        page_source(&info).compatibility_warnings().is_empty(),
        "nothing is reported discarded: {:?}",
        page_source(&info).compatibility_warnings()
    );
}

/// The Lynx XML envelope the upstream fixture is written in, kept as its own
/// case so that dropping it from
/// [`a_card_carries_keyframes_and_font_face`] loses no coverage.
///
/// Here a `<style>` body is author CSS text handed to the engine's own CSS
/// parser rather than pre-tokenized, so the only claim available at this layer
/// is that the section is carried across the envelope byte for byte and
/// registered as a sheet. It says nothing about either at-rule surviving
/// lowering — that is the bundle case's job.
#[test]
fn a_lynx_xml_envelope_carries_its_style_section_verbatim() {
    const STYLE: &str = "@keyframes spin{from{opacity:0}to{opacity:1}}\
                         @font-face{font-family:CardFont;src:url(a.woff2)}";
    let source = format!(
        "<!doctype lynx>\n\
         <lynx engine-version=\"4.2\">\n\
         <style>{STYLE}</style>\n\
         <script thread=\"main\">// main</script>\n\
         <script thread=\"background\">// background</script>\n\
         </lynx>\n"
    );

    let parsed = bobcat_source::xml::parse(&source).expect("the card parses");
    assert_eq!(
        parsed.style,
        Some(STYLE),
        "the style section is carried verbatim"
    );

    let url = Url::parse("bobcat-test://card/app.lynx.xml").expect("a valid URL");
    let card = PageSource::from_bytes(&url, source.as_bytes()).expect("the card decodes");
    assert_eq!(
        card.view_sources(SCREEN).style_sheets.len(),
        1,
        "the card registers its author stylesheet"
    );
}
