//! rkyv 0.7 wire model for the pre-parsed CSS `StyleInfo` section; field and
//! enum ordering is serialized ABI.

use std::collections::HashMap;

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

pub use crate::css_property::{
    CssProperty, CssPropertyId, ParsedDeclaration, STYLE_PROPERTY_MAP, ValueToken, token_types,
};
use crate::error::DecodeError;

/// Root of the `StyleInfo` section. Mirrors web-core's `RawStyleInfo`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleInfo {
    pub css_id_to_style_sheet: HashMap<i32, StyleSheet>,
    pub style_text_size_hint: usize,
}

/// One CSS fragment. Mirrors web-core's `StyleSheet`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleSheet {
    pub imports: Vec<i32>,
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
    pub kind: RuleKind,
    pub prelude: RulePrelude,
    pub declaration_block: DeclarationBlock,
    #[omit_bounds]
    #[archive_attr(omit_bounds)]
    pub children: Vec<Rule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
#[repr(i32)]
pub enum RuleKind {
    Style = 1,
    FontFace = 2,
    Keyframes = 3,
}

/// Rule prelude. Mirrors web-core's `RulePrelude`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct RulePrelude {
    pub selectors: Vec<Selector>,
}

/// One complex selector. Mirrors web-core's `Selector`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct Selector {
    pub components: Vec<SimpleSelector>,
}

impl Selector {
    #[must_use]
    pub fn to_css_string(&self) -> String {
        let mut buf = String::new();
        for component in &self.components {
            match component.kind {
                SimpleSelectorKind::Type | SimpleSelectorKind::UnknownText => {
                    buf.push_str(&component.value);
                }
                SimpleSelectorKind::Class => {
                    buf.push('.');
                    buf.push_str(&component.value);
                }
                SimpleSelectorKind::Id => {
                    buf.push('#');
                    buf.push_str(&component.value);
                }
                SimpleSelectorKind::Attribute => {
                    buf.push('[');
                    buf.push_str(&component.value);
                    buf.push(']');
                }
                SimpleSelectorKind::PseudoClass => {
                    buf.push(':');
                    buf.push_str(&component.value);
                }
                SimpleSelectorKind::PseudoElement => {
                    buf.push_str("::");
                    buf.push_str(&component.value);
                }
                SimpleSelectorKind::Universal => {
                    buf.push('*');
                }
                SimpleSelectorKind::Combinator => {
                    buf.push(' ');
                    buf.push_str(&component.value);
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
    pub kind: SimpleSelectorKind,
    pub value: String,
}

#[expect(missing_docs, reason = "self-describing selector kinds")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
#[repr(i32)]
pub enum SimpleSelectorKind {
    Class = 1,
    Id = 2,
    Attribute = 3,
    Type = 4,
    Combinator = 5,
    PseudoClass = 6,
    PseudoElement = 7,
    Universal = 8,
    UnknownText = 9,
}

/// A rule's declarations. Mirrors web-core's `DeclarationBlock`.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct DeclarationBlock {
    pub declarations: Vec<ParsedDeclaration>,
}

pub(crate) fn decode_style_info(bytes: &[u8]) -> Result<StyleInfo, DecodeError> {
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

    #[test]
    fn style_info_round_trips() {
        let rule = Rule {
            kind: RuleKind::Keyframes,
            prelude: RulePrelude {
                selectors: vec![Selector {
                    components: vec![SimpleSelector {
                        kind: SimpleSelectorKind::UnknownText,
                        value: "spin".to_owned(),
                    }],
                }],
            },
            declaration_block: DeclarationBlock {
                declarations: vec![],
            },
            children: vec![Rule {
                kind: RuleKind::Style,
                prelude: RulePrelude {
                    selectors: vec![Selector {
                        components: vec![SimpleSelector {
                            kind: SimpleSelectorKind::UnknownText,
                            value: "to".to_owned(),
                        }],
                    }],
                },
                declaration_block: DeclarationBlock {
                    declarations: vec![ParsedDeclaration {
                        property: CssProperty {
                            id: CssPropertyId::Transform,
                            unknown_name: None,
                        },
                        value_tokens: vec![
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
                children: vec![],
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
            style_text_size_hint: 123,
        };

        let bytes = rkyv::to_bytes::<_, 1024>(&original).unwrap();
        let decoded = decode_style_info(&bytes).unwrap();

        assert_eq!(decoded.style_text_size_hint, 123);
        let sheet = &decoded.css_id_to_style_sheet[&42];
        assert_eq!(sheet.imports, [7]);
        let rule = &sheet.rules[0];
        assert_eq!(rule.kind, RuleKind::Keyframes);
        assert_eq!(rule.prelude.selectors[0].to_css_string(), "spin");
        let keyframe = &rule.children[0];
        assert_eq!(keyframe.prelude.selectors[0].to_css_string(), "to");
        let declaration = &keyframe.declaration_block.declarations[0];
        assert_eq!(declaration.property.name(), "transform");
        assert_eq!(declaration.value_text(), "rotate(360deg)");
    }

    #[test]
    fn rejects_garbage_style_info() {
        let garbage = vec![0xFF; 64];
        assert!(matches!(
            decode_style_info(&garbage),
            Err(DecodeError::StyleInfo(_))
        ));
    }
}
