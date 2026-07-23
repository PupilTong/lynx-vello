//! `@keyframes` and `@font-face` rule semantics — ported from
//! `lynx/core/renderer/css/css_keyframes_token_unittest.cc`,
//! `css_font_face_token_unittest.cc`, `ng/font_face/font_face_rule_test.cc`,
//! and `ng/parser/font_face_parser_test.cc`.

mod common;

use common::{Doc, device, url_data};
use stylo::context::QuirksMode;
use stylo::media_queries::MediaList;
use stylo::properties::font_face::DescriptorId;
use stylo::shared_lock::SharedRwLock;
use stylo::stylesheets::{AllowImportRules, CssRule, Origin, Stylesheet};
use stylo_traits::ToCss;
use w3c_dom::{Document, StylesheetOrigin};

fn parse_sheet(css: &str) -> (SharedRwLock, Stylesheet) {
    let lock = SharedRwLock::new();
    let media = stylo::servo_arc::Arc::new(lock.wrap(MediaList::empty()));
    let sheet = Stylesheet::from_str(
        css,
        url_data(),
        Origin::Author,
        media,
        lock.clone(),
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::Yes,
    );
    (lock, sheet)
}

fn keyframe_selectors(css: &str) -> Vec<String> {
    let (lock, sheet) = parse_sheet(css);
    let guard = lock.read();
    for rule in sheet.contents.read_with(&guard).rules(&guard) {
        if let CssRule::Keyframes(keyframes) = rule {
            return keyframes
                .read_with(&guard)
                .keyframes
                .iter()
                .map(|keyframe| keyframe.read_with(&guard).selector.to_css_string())
                .collect();
        }
    }
    panic!("no @keyframes rule parsed from `{css}`");
}

fn font_face_descriptor(body: &str, id: DescriptorId) -> Option<String> {
    let (lock, sheet) = parse_sheet(&format!("@font-face {{ {body} }}"));
    let guard = lock.read();
    for rule in sheet.contents.read_with(&guard).rules(&guard) {
        if let CssRule::FontFace(font_face) = rule {
            let mut css = String::new();
            font_face
                .read_with(&guard)
                .descriptors
                .get(id, &mut css)
                .expect("serialization succeeds");
            return (!css.is_empty()).then_some(css);
        }
    }
    panic!("no @font-face rule parsed");
}

#[test]
fn keyframe_selectors_normalize_and_reject() {
    assert_eq!(
        keyframe_selectors(
            "@keyframes k { from { opacity: 0 } to { opacity: 1 } 99% { opacity: 0.5 } }"
        ),
        vec!["0%", "100%", "99%"],
        "from/to normalize to 0%/100%; bare percentages keep their value"
    );
    assert_eq!(
        keyframe_selectors("@keyframes k { from { opacity: 0 } -1% { opacity: 1 } }"),
        vec!["0%"],
        "an out-of-range selector drops its keyframe, not clamps it"
    );
    assert_eq!(
        keyframe_selectors("@keyframes k { 0%, 50%, to { opacity: 1 } }"),
        vec!["0%, 50%, 100%"],
        "selector lists stay grouped on one keyframe"
    );
}

#[test]
fn keyframes_rules_register_and_resolve_by_name() {
    let mut doc: Document<()> = Document::new(device(800.0, 600.0));
    let rule = doc
        .build_keyframes_rule("slide", "from { opacity: 0 } to { opacity: 1 }")
        .expect("named rule builds");
    doc.append_rules(vec![rule], StylesheetOrigin::Author);
    assert!(
        doc.build_keyframes_rule("", "from { opacity: 0 }")
            .is_none(),
        "an empty animation name is invalid"
    );

    let root = doc.create_element("page", ());
    let root_ref = doc.get(root).expect("root is live");
    assert!(doc.has_keyframes_animation("slide", root_ref));
    assert!(!doc.has_keyframes_animation("missing", root_ref));
}

#[test]
fn font_face_full_descriptor_set() {
    let body = r#"
        font-family: "Bitstream Vera Serif Bold";
        src: local("PingFang SC"), url("https://example.com/font.woff2") format("woff2");
        font-weight: 100 900;
        font-stretch: 75% 125%;
        font-style: oblique 10deg 20deg;
        font-variation-settings: "wght" 700, "wdth" 80.5;
        unicode-range: U+0025-00FF, U+4??;
    "#;
    assert_eq!(
        font_face_descriptor(body, DescriptorId::FontFamily).as_deref(),
        Some("\"Bitstream Vera Serif Bold\"")
    );
    let src = font_face_descriptor(body, DescriptorId::Src).expect("src parses");
    assert!(src.contains("local(\"PingFang SC\")"), "src: {src}");
    assert!(
        src.contains("url(\"https://example.com/font.woff2\") format(woff2)")
            || src.contains("url(\"https://example.com/font.woff2\") format(\"woff2\")"),
        "format hint attaches to the url source: {src}"
    );
    assert_eq!(
        font_face_descriptor(body, DescriptorId::FontWeight).as_deref(),
        Some("100 900")
    );
    assert_eq!(
        font_face_descriptor(body, DescriptorId::FontStretch).as_deref(),
        Some("75% 125%")
    );
    assert_eq!(
        font_face_descriptor(body, DescriptorId::FontStyle).as_deref(),
        Some("oblique 10deg 20deg")
    );
    assert_eq!(
        font_face_descriptor(body, DescriptorId::FontVariationSettings).as_deref(),
        Some("\"wght\" 700, \"wdth\" 80.5")
    );
    assert_eq!(
        font_face_descriptor(body, DescriptorId::UnicodeRange).as_deref(),
        Some("U+25-FF, U+400-4FF"),
        "wildcard ranges canonicalize to explicit bounds"
    );
}

#[test]
fn font_face_defaults_stay_unset() {
    let body = "font-family: My Font; src: url(https://example.com/font.ttf);";
    assert_eq!(
        font_face_descriptor(body, DescriptorId::FontFamily).as_deref(),
        Some("My Font"),
        "identifier-sequence family names keep their unquoted form"
    );
    assert!(font_face_descriptor(body, DescriptorId::Src).is_some());
    for id in [
        DescriptorId::FontWeight,
        DescriptorId::FontStretch,
        DescriptorId::FontStyle,
        DescriptorId::FontVariationSettings,
        DescriptorId::UnicodeRange,
    ] {
        assert!(
            font_face_descriptor(body, id).is_none(),
            "descriptor {id:?} stays unset"
        );
    }
}

#[test]
fn font_face_weight_and_style_forms() {
    let base = "font-family: F; src: url(font.woff2);";
    assert_eq!(
        font_face_descriptor(
            &format!("{base} font-weight: bold normal;"),
            DescriptorId::FontWeight
        )
        .as_deref(),
        Some("bold normal"),
        "keyword endpoints parse (computed range sorts to 400..700 later)"
    );
    assert_eq!(
        font_face_descriptor(
            &format!("{base} font-weight: 900 100;"),
            DescriptorId::FontWeight
        )
        .as_deref(),
        Some("900 100")
    );
    assert_eq!(
        font_face_descriptor(
            &format!("{base} font-style: italic;"),
            DescriptorId::FontStyle
        )
        .as_deref(),
        Some("italic")
    );
    assert_eq!(
        font_face_descriptor(
            &format!("{base} font-style: oblique;"),
            DescriptorId::FontStyle
        )
        .as_deref(),
        Some("oblique"),
        "default oblique angle (14deg) is implicit"
    );
    let converted = font_face_descriptor(
        &format!("{base} font-style: oblique -0.25turn 1.5707963267948966rad;"),
        DescriptorId::FontStyle,
    )
    .expect("boundary angles accepted");
    assert!(
        converted.starts_with("oblique -0.25turn"),
        "authored units preserved at the descriptor level: {converted}"
    );
    assert_eq!(
        font_face_descriptor(
            &format!("{base} font-stretch: 0.4%;"),
            DescriptorId::FontStretch
        )
        .as_deref(),
        Some("0.4%")
    );
}

#[test]
fn font_face_invalid_optional_descriptors_dropped() {
    let base = "font-family: MyFont; src: url(font.woff2);";
    let body = format!(
        "{base} font-weight: 100 200 300; font-stretch: bogus; \
         font-style: italic oblique; font-variation-settings: \"bad\"; \
         unicode-range: not-a-range;"
    );
    assert!(font_face_descriptor(&body, DescriptorId::FontFamily).is_some());
    assert!(font_face_descriptor(&body, DescriptorId::Src).is_some());
    for id in [
        DescriptorId::FontWeight,
        DescriptorId::FontStretch,
        DescriptorId::FontStyle,
        DescriptorId::FontVariationSettings,
        DescriptorId::UnicodeRange,
    ] {
        assert!(
            font_face_descriptor(&body, id).is_none(),
            "invalid descriptor {id:?} is dropped"
        );
    }
    for style in ["oblique 91deg", "oblique 1e100turn"] {
        assert!(
            font_face_descriptor(
                &format!("{base} font-style: {style};"),
                DescriptorId::FontStyle
            )
            .is_none(),
            "`{style}` is out of the ±90deg oblique range"
        );
    }
}

#[test]
fn font_face_missing_required_descriptors_stay_unset() {
    assert!(
        font_face_descriptor("font-family: MyFont;", DescriptorId::Src).is_none(),
        "no src descriptor"
    );
    assert!(
        font_face_descriptor("src: url(font.woff2);", DescriptorId::FontFamily).is_none(),
        "no family descriptor"
    );
}

#[test]
fn font_face_src_list_is_forgiving_per_entry() {
    let src = font_face_descriptor(
        "font-family: MyFont; src: local(\"A\"), invalid-fn(\"font.woff2\");",
        DescriptorId::Src,
    );
    match src {
        Some(list) => {
            assert!(list.contains("local(\"A\")"), "good entry survives: {list}");
            assert!(!list.contains("invalid-fn"), "bad entry dropped: {list}");
        }
        None => panic!("src list with one valid entry must survive"),
    }
}

#[test]
fn font_face_rules_register_in_the_stylist() {
    let mut doc = Doc::new();
    assert_eq!(doc.dom.font_face_count(), 0);
    doc.add_css("@font-face { font-family: A; src: url(a.woff2); }");
    doc.add_css("@font-face { font-family: B; src: url(b.woff2); }");
    assert_eq!(doc.dom.font_face_count(), 2);
}
