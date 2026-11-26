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
//!   `Selector::write_css_string` for a prelude (`lower_style.rs:237-269`),
//!   `ParsedDeclaration::value_and_importance` for a declaration (`lower_style.rs:302-320`),
//!   `ParsedDeclaration::write_value_text` for an `@font-face` descriptor
//!   (`lower_style.rs:322-338`). No test in this file re-serializes CSS itself: every asserted
//!   string is the return value of one production call on one decoded item.
//! * the engine's stylesheet hand-off, through [`registered_style_sheet`] —
//!   `PageSource::from_bytes` runs the whole lowering (`ContainerSources::from_template`) and
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
//! output channel. The entry-name guard separates a lazy bundle's own
//! `StyleInfo` from its host card's, and no lazy bundle's `StyleInfo` is
//! mounted here; the lowering keeps every rule kind in one sheet. The
//! `:where([l-css-id="N"])` guard web-core adds to a scoped fragment's rules
//! *is* synthesized, by `lower_style`, and is asserted there: the lowered
//! sheet is not readable from this crate (see above).
//!
//! # Halves that live in other crates
//!
//! Two assertions the assignment asked for are not expressible from this
//! crate and are **not** claimed here:
//!
//! * the computed value of `color: var(--X, rgba(22,24,35,.6))` with `--X` unset belongs in
//!   `crates/terminus/tests/custom_properties.rs`;
//! * what stylo does with a `var()` inside an `@font-face` descriptor, and the
//!   `DescriptorId::FontFamily` / `DescriptorId::Src` readback, belong in
//!   `crates/terminus/tests/at_rules.rs` — which today has no `var()` case at all.
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
//! stylo `FontFaceRule` (`crates/terminus/src/style/engine.rs:417-426`) and enters
//! the cascade, but no reader of that variant exists, no `src:` URL is ever
//! fetched, and a face reaches shaping only as embedder-supplied bytes through
//! `Document::register_fonts` (`crates/terminus/src/layout/mod.rs:192`). A card
//! that ships its own face and names it therefore renders in the default
//! family. That gap is carried by
//! `a_font_face_declared_family_shapes_the_text_that_names_it` in
//! `crates/terminus/tests/web_text_replication.rs`, which asserts the shaped
//! advance a browser gives web-core and is `#[ignore]`d on the gap. Read the
//! green count below as "the wire and the lowering", never as "`@font-face`
//! works".
// These tests build the native resource system, which wasm32 has no default
// platform for; the wasm32 run covers `decode_web_bundle` and `zip_source`.
#![cfg(not(target_arch = "wasm32"))]

use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::PageSource;
use bobcat_source::web::style_info::{
    CssProperty, DeclarationBlock, ParsedDeclaration, Rule, RuleKind, RulePrelude, Selector,
    SimpleSelector, SimpleSelectorKind, StyleInfo, StyleSheet, ValueToken, token_types,
};
use bobcat_source::web::{SectionLabel, decode};
use url::Url;

const SCREEN: bobcat_core::ScreenMetrics =
    bobcat_core::ScreenMetrics::for_viewport(32.0, 24.0, 1.0);

fn one_token(property: &str, token_type: u8, value: &str) -> ParsedDeclaration {
    tokenized(property, &[(token_type, value)])
}

fn ident(property: &str, value: &str) -> ParsedDeclaration {
    one_token(property, token_types::IDENT_TOKEN, value)
}

fn dimension(property: &str, value: &str) -> ParsedDeclaration {
    one_token(property, token_types::DIMENSION_TOKEN, value)
}

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

fn push_section(bytes: &mut Vec<u8>, label: SectionLabel, content: &[u8]) {
    bytes.extend_from_slice(&(label as u32).to_le_bytes());
    bytes
        .extend_from_slice(&(u32::try_from(content.len()).expect("a small section")).to_le_bytes());
    bytes.extend_from_slice(content);
}

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

fn decoded(style_info: &StyleInfo) -> StyleInfo {
    decode(&bundle(style_info))
        .expect("the bundle decodes")
        .style_info
        .expect("the StyleInfo section is present")
}

fn fragment(style_info: &StyleInfo, css_id: i32) -> &[Rule] {
    &style_info.css_id_to_style_sheet[&css_id].rules
}

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

fn selectors(prelude: &RulePrelude) -> Vec<String> {
    prelude
        .selectors
        .iter()
        .map(Selector::to_css_string)
        .collect()
}

fn page_source(style_info: &StyleInfo) -> PageSource {
    let url = Url::parse("bobcat-test://card/app.web.bundle").expect("a valid URL");
    PageSource::from_bytes(&url, &bundle(style_info)).expect("the card decodes")
}

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

    let url = card
        .view_sources(SCREEN, "en-US")
        .style_sheets
        .first()
        .cloned();
    if let Some(url) = url.as_deref() {
        assert!(
            resources.unregister(url),
            "the lowered sheet is registered under the URL the view asks for"
        );
    }
    url
}

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
}

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

    let important = decoded(&corpus[1].1);
    let width = &fragment(&important, 0)[0].declaration_block.declarations[0];
    assert!(!width.is_important, "the wire flag stays false");
    assert_eq!(
        width.value_and_importance(),
        ("100vw".to_owned(), true),
        "the marker is split back out of the value tokens"
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "one literal entry per corpus stylesheet; splitting the table hides the corpus"
)]
fn text_and_font_corpus() -> Vec<(&'static str, StyleInfo, Vec<Lowerable>)> {
    vec![
        (
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
}

#[test]
fn a_range_keyframe_selector_survives_the_bundle() {
    let info = style_info(vec![keyframes_rule(
        "reveal",
        vec![
            (
                "entry 0%",
                vec![one_token("opacity", token_types::NUMBER_TOKEN, "0")],
            ),
            (
                "entry 100%",
                vec![one_token("opacity", token_types::NUMBER_TOKEN, "1")],
            ),
        ],
    )]);

    assert_eq!(
        lowerable_rules(&info, 0),
        vec![expect_keyframes(
            "reveal",
            vec![
                ("entry 0%", vec![declaration("opacity", "0")]),
                ("entry 100%", vec![declaration("opacity", "1")]),
            ],
        )]
    );
    assert!(registered_style_sheet(&info).is_some());
}

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
        card.view_sources(SCREEN, "en-US").style_sheets.len(),
        1,
        "the card registers its author stylesheet"
    );
}
