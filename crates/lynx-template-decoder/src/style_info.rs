//! The `StyleInfo` section: CSS pre-parsed into rules/selectors/declarations
//! and serialized with **rkyv 0.7** (default `size_32` layout, root at the end
//! of the buffer).
//!
//! These types mirror
//! `packages/web-platform/web-core/src/template/template_sections/style_info/raw_style_info.rs`
//! in lynx-stack **exactly** — field order and enum variant order define the
//! rkyv wire format. Do not reorder anything here.

use std::collections::HashMap;

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

pub use crate::css_property::{
    CssProperty, CssPropertyId, ParsedDeclaration, STYLE_PROPERTY_MAP, ValueToken, token_types,
};
use crate::error::DecodeError;

/// Root of the `StyleInfo` section. Mirrors web-core's `RawStyleInfo`.
///
/// The source type uses an Fnv hasher for the map; the hasher does not affect
/// the archived layout, so a std `HashMap` is used here.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleInfo {
    /// CSS fragment id (`cssId`, one per entry/component) → its stylesheet.
    pub css_id_to_style_sheet: HashMap<i32, StyleSheet>,
    /// Length hint for the flattened CSS text, filled in by the encoder.
    pub style_content_str_size_hint: usize,
}

/// One CSS fragment. Mirrors web-core's `StyleSheet`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleSheet {
    /// `@import`ed fragment ids (flattened via topological sort at runtime).
    pub imports: Vec<i32>,
    /// The fragment's rules, in source order.
    pub rules: Vec<Rule>,
}

/// A style / `@font-face` / `@keyframes` rule. Mirrors web-core's `Rule`.
#[derive(Debug, Clone, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(bound(
    serialize = "__S: rkyv::ser::Serializer + rkyv::ser::ScratchSpace",
    deserialize = "__D: rkyv::de::SharedDeserializeRegistry"
))]
#[archive_attr(
    derive(bytecheck::CheckBytes),
    check_bytes(
        bound = "__C: rkyv::validation::ArchiveContext, <__C as rkyv::Fallible>::Error: std::error::Error"
    )
)]
pub struct Rule {
    /// Which kind of rule this is.
    pub rule_type: RuleType,
    /// Selector list (style rules) or prelude text (keyframes); empty for
    /// `@font-face`.
    pub prelude: RulePrelude,
    /// The rule's declarations.
    pub declaration_block: DeclarationBlock,
    /// Child rules (e.g. the keyframe blocks of an `@keyframes` rule).
    #[omit_bounds]
    #[archive_attr(omit_bounds)]
    pub nested_rules: Vec<Rule>,
}

/// Rule kind. Mirrors web-core's `RuleType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
#[repr(i32)]
pub enum RuleType {
    /// A plain style rule (`selector { … }`).
    Declaration = 1,
    /// An `@font-face` rule.
    FontFace = 2,
    /// An `@keyframes` rule.
    KeyFrames = 3,
}

/// Rule prelude. Mirrors web-core's `RulePrelude`.
///
/// For [`RuleType::Declaration`] this is the selector list. For
/// [`RuleType::KeyFrames`] it holds a single selector whose one
/// [`ValueToken`]-less simple selector carries the prelude text. For
/// [`RuleType::FontFace`] it is empty.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct RulePrelude {
    /// The comma-separated selectors of the rule.
    pub selector_list: Vec<Selector>,
}

/// One complex selector. Mirrors web-core's `Selector`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct Selector {
    /// The sequence of simple selectors / combinators.
    pub simple_selectors: Vec<SimpleSelector>,
}

impl Selector {
    /// Reassembles the selector source text, mirroring web-core's
    /// `Selector::generate_to_string_buf`.
    #[must_use]
    pub fn to_css_string(&self) -> String {
        let mut buf = String::new();
        for simple in &self.simple_selectors {
            match simple.selector_type {
                SimpleSelectorType::TypeSelector | SimpleSelectorType::UnknownText => {
                    buf.push_str(&simple.value);
                }
                SimpleSelectorType::ClassSelector => {
                    buf.push('.');
                    buf.push_str(&simple.value);
                }
                SimpleSelectorType::IdSelector => {
                    buf.push('#');
                    buf.push_str(&simple.value);
                }
                SimpleSelectorType::AttributeSelector => {
                    buf.push('[');
                    buf.push_str(&simple.value);
                    buf.push(']');
                }
                SimpleSelectorType::PseudoClassSelector => {
                    buf.push(':');
                    buf.push_str(&simple.value);
                }
                SimpleSelectorType::PseudoElementSelector => {
                    buf.push_str("::");
                    buf.push_str(&simple.value);
                }
                SimpleSelectorType::UniversalSelector => {
                    buf.push('*');
                }
                SimpleSelectorType::Combinator => {
                    buf.push(' ');
                    buf.push_str(&simple.value);
                    buf.push(' ');
                }
            }
        }
        buf
    }
}

/// One simple selector or combinator. Mirrors web-core's `OneSimpleSelector`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct SimpleSelector {
    /// What kind of simple selector this is.
    pub selector_type: SimpleSelectorType,
    /// The selector text without its sigil (class/id name, tag name,
    /// attribute body, combinator char, …).
    pub value: String,
}

/// Simple selector kind. Mirrors web-core's `OneSimpleSelectorType`.
#[expect(missing_docs, reason = "self-describing selector kinds")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
#[repr(i32)]
pub enum SimpleSelectorType {
    ClassSelector = 1,
    IdSelector = 2,
    AttributeSelector = 3,
    TypeSelector = 4,
    Combinator = 5,
    PseudoClassSelector = 6,
    PseudoElementSelector = 7,
    UniversalSelector = 8,
    UnknownText = 9,
}

/// A rule's declarations. Mirrors web-core's `DeclarationBlock`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct DeclarationBlock {
    /// The declarations, in source order.
    pub declarations: Vec<ParsedDeclaration>,
}

/// Decodes the rkyv-serialized `StyleInfo` section payload.
pub(crate) fn decode_style_info(bytes: &[u8]) -> Result<StyleInfo, DecodeError> {
    // rkyv reads the archive through relative pointers; the buffer must be
    // properly aligned, which a plain `&[u8]` section slice does not
    // guarantee. AlignedVec aligns to 16 bytes.
    let mut aligned = rkyv::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    let archived = rkyv::check_archived_root::<StyleInfo>(&aligned)
        .map_err(|e| DecodeError::StyleInfo(e.to_string()))?;
    let style_info: StyleInfo = archived
        .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
        .map_err(|e| DecodeError::StyleInfo(format!("{e:?}")))?;
    Ok(style_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips a nested style tree through rkyv using the same
    /// serializer shape as web-core (`rkyv::to_bytes::<_, 1024>`).
    #[test]
    fn style_info_round_trips() {
        let rule = Rule {
            rule_type: RuleType::KeyFrames,
            prelude: RulePrelude {
                selector_list: vec![Selector {
                    simple_selectors: vec![SimpleSelector {
                        selector_type: SimpleSelectorType::UnknownText,
                        value: "spin".to_owned(),
                    }],
                }],
            },
            declaration_block: DeclarationBlock {
                declarations: vec![],
            },
            nested_rules: vec![Rule {
                rule_type: RuleType::Declaration,
                prelude: RulePrelude {
                    selector_list: vec![Selector {
                        simple_selectors: vec![SimpleSelector {
                            selector_type: SimpleSelectorType::UnknownText,
                            value: "to".to_owned(),
                        }],
                    }],
                },
                declaration_block: DeclarationBlock {
                    declarations: vec![ParsedDeclaration {
                        property_id: CssProperty {
                            id: CssPropertyId::Transform,
                            unknown_name: None,
                        },
                        value_token_list: vec![
                            ValueToken {
                                token_type: token_types::FUNCTION_TOKEN,
                                value: "rotate(".to_owned(),
                            },
                            ValueToken {
                                token_type: token_types::DIMENSION_TOKEN,
                                value: "360deg".to_owned(),
                            },
                            ValueToken {
                                token_type: token_types::RIGHT_PARENTHESES_TOKEN,
                                value: ")".to_owned(),
                            },
                        ],
                        is_important: false,
                    }],
                },
                nested_rules: vec![],
            }],
        };
        let original = StyleInfo {
            css_id_to_style_sheet: HashMap::from([(
                42,
                StyleSheet {
                    imports: vec![7],
                    rules: vec![rule],
                },
            )]),
            style_content_str_size_hint: 123,
        };

        let bytes = rkyv::to_bytes::<_, 1024>(&original).unwrap();
        let decoded = decode_style_info(&bytes).unwrap();

        assert_eq!(decoded.style_content_str_size_hint, 123);
        let sheet = &decoded.css_id_to_style_sheet[&42];
        assert_eq!(sheet.imports, [7]);
        let rule = &sheet.rules[0];
        assert_eq!(rule.rule_type, RuleType::KeyFrames);
        assert_eq!(rule.prelude.selector_list[0].to_css_string(), "spin");
        let keyframe = &rule.nested_rules[0];
        assert_eq!(keyframe.prelude.selector_list[0].to_css_string(), "to");
        let declaration = &keyframe.declaration_block.declarations[0];
        assert_eq!(declaration.property_id.name(), "transform");
        assert_eq!(declaration.value_text(), "rotate(360deg)");
    }

    #[test]
    fn rejects_garbage_style_info() {
        // check_archived_root must reject bytes that are not a valid archive
        // instead of invoking UB.
        let garbage = vec![0xFF; 64];
        assert!(matches!(
            decode_style_info(&garbage),
            Err(DecodeError::StyleInfo(_))
        ));
    }
}
