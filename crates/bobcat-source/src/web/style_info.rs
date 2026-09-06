//! rkyv 0.7 wire model for the pre-parsed CSS `StyleInfo` section; field and
//! enum ordering is serialized ABI.

use std::collections::HashMap;

use rkyv::validation::check_archived_root_with_context;
use rkyv::validation::validators::ArchiveValidator;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

pub use super::css_property::{
    CssProperty, CssPropertyId, ParsedDeclaration, STYLE_PROPERTY_MAP, ValueToken, token_types,
};
use super::error::DecodeError;

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
        self.write_css_string(&mut buf);
        buf
    }

    /// Appends [`Self::to_css_string`] to `buf`, without allocating a
    /// temporary for each selector of a list.
    pub fn write_css_string(&self, buf: &mut String) {
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
                    // The encoder stores the generated attribute-selector text
                    // with its brackets already included (`[type=submit]`).
                    buf.push_str(&component.value);
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
                    // The descendant combinator is encoded as a single space.
                    if component.value.trim().is_empty() {
                        buf.push(' ');
                    } else {
                        buf.push(' ');
                        buf.push_str(&component.value);
                        buf.push(' ');
                    }
                }
            }
        }
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

/// Bounds allocation for the copied archive and validation work.
const MAX_SECTION_LEN: usize = 1 << 20;

/// The deepest rule tree a caller may receive, walk, or drop.
const MAX_RULE_DEPTH: usize = 64;

/// Bound recursive byte validation *before* constructing an owned rule tree.
/// Each children vector consumes a subtree level. The remaining allowance
/// covers the root, map entries, rules vector and leaf vectors/strings; it
/// does not grow with input size. Tests exercise populated rules at the limit.
const MAX_VALIDATION_DEPTH: usize = MAX_RULE_DEPTH + 8;

pub(crate) fn decode_style_info(bytes: &[u8]) -> Result<StyleInfo, DecodeError> {
    if bytes.len() > MAX_SECTION_LEN {
        return Err(DecodeError::StyleInfo(format!(
            "section is {} bytes, over the {MAX_SECTION_LEN}-byte limit",
            bytes.len()
        )));
    }

    let mut aligned = rkyv::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    // This model contains no shared pointers, so ArchiveValidator supplies
    // every check it needs. No platform-specific threads or unsafe access:
    // excessive nesting fails during validation on the calling thread.
    let mut validator = ArchiveValidator::with_max_depth(&aligned, MAX_VALIDATION_DEPTH);
    let archived = check_archived_root_with_context::<StyleInfo, _>(&aligned, &mut validator)
        .map_err(|e| DecodeError::StyleInfo(e.to_string()))?;
    let style_info: StyleInfo = archived
        .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
        .map_err(|e| DecodeError::StyleInfo(format!("{e:?}")))?;

    let depth = nesting_depth(&style_info);
    if depth > MAX_RULE_DEPTH {
        // Validation already bounded this tree; rejecting and dropping it
        // here cannot recurse to an input-controlled depth.
        return Err(DecodeError::StyleInfo(format!(
            "rule nesting is {depth} levels deep, over the {MAX_RULE_DEPTH}-level limit"
        )));
    }

    Ok(style_info)
}

/// Deepest `children` chain in the section, counted without recursion.
fn nesting_depth(style_info: &StyleInfo) -> usize {
    let mut deepest = 0;
    let mut pending: Vec<(&Rule, usize)> = style_info
        .css_id_to_style_sheet
        .values()
        .flat_map(|sheet| sheet.rules.iter().map(|rule| (rule, 1)))
        .collect();
    while let Some((rule, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        pending.extend(rule.children.iter().map(|child| (child, depth + 1)));
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_rule() -> Rule {
        Rule {
            kind: RuleKind::Style,
            prelude: RulePrelude::default(),
            declaration_block: DeclarationBlock {
                declarations: vec![],
            },
            children: vec![],
        }
    }

    /// Serializes a `Rule` chain `depth` levels deep.
    ///
    /// On a stack of its own because encoding recurses too. That side is ours
    /// and is never fed by an attacker, so an overflow there would say nothing
    /// about the decoder — but it would look identical in the test output.
    fn deep_section(depth: usize) -> Vec<u8> {
        std::thread::Builder::new()
            .stack_size(512 * 1024 * 1024)
            .spawn(move || {
                let mut rule = empty_rule();
                for _ in 0..depth {
                    let mut parent = empty_rule();
                    parent.children = vec![rule];
                    rule = parent;
                }
                let info = StyleInfo {
                    css_id_to_style_sheet: HashMap::from([(
                        0,
                        StyleSheet {
                            imports: vec![],
                            rules: vec![rule],
                        },
                    )]),
                    style_text_size_hint: 0,
                };
                rkyv::to_bytes::<_, 4096>(&info)
                    .expect("serialize")
                    .to_vec()
            })
            .expect("spawn")
            .join()
            .expect("the encoder survived")
    }

    /// The regression this whole bound exists for.
    ///
    /// `Rule` is self-referential with no depth bound and rkyv 0.7's derived
    /// validator recurses per level, so a *well-formed* section — nothing for
    /// validation to reject — used to abort the process inside
    /// `check_archived_root` with `fatal runtime error: stack overflow`. That
    /// is `SIGABRT`, not a panic, so this test could not have caught it with
    /// `should_panic`: the whole test binary died.
    ///
    /// Decode directly on a 256 KiB caller. The decoder must reject the input
    /// before following its 8000 levels (224 KB of archive), without spawning
    /// a helper thread or requiring a stack sized from the input length.
    #[test]
    fn deep_nesting_is_refused_instead_of_aborting() {
        let bytes = deep_section(8000);
        assert!(
            bytes.len() < MAX_SECTION_LEN,
            "the length cap is not what is under test here"
        );

        let decoded = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || decode_style_info(&bytes))
            .expect("spawn")
            .join()
            .expect("decoding did not abort the process");

        let message = decoded
            .expect_err("8000 levels is over the limit")
            .to_string();
        assert!(
            message.contains("maximum subtree depth"),
            "expected the depth limit to reject this, got: {message}"
        );
    }

    /// The limit has to leave the shape the format actually uses alone.
    #[test]
    fn nesting_within_the_limit_still_decodes() {
        let bytes = deep_section(MAX_RULE_DEPTH - 1);
        let decoded = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || decode_style_info(&bytes).expect("within the limit"))
            .expect("spawn small-stack caller")
            .join()
            .expect("valid input decodes on the caller's stack");
        assert_eq!(nesting_depth(&decoded), MAX_RULE_DEPTH);
    }

    #[test]
    fn a_rule_tree_one_level_over_the_limit_is_rejected() {
        let bytes = deep_section(MAX_RULE_DEPTH);
        let error = decode_style_info(&bytes).expect_err("65 rule levels");
        assert!(error.to_string().contains("levels deep"), "{error}");
    }

    #[test]
    fn a_populated_leaf_at_the_rule_limit_still_decodes() {
        let mut rule = empty_rule();
        rule.prelude.selectors = vec![Selector {
            components: vec![SimpleSelector {
                kind: SimpleSelectorKind::Class,
                value: "a-selector-long-enough-to-be-out-of-line".to_owned(),
            }],
        }];
        rule.declaration_block.declarations = vec![ParsedDeclaration {
            property: CssProperty::from_name("--a-long-custom-property-name"),
            value_tokens: vec![ValueToken {
                token_type: token_types::IDENT_TOKEN,
                value: "a-value-long-enough-to-be-out-of-line".to_owned(),
            }],
            is_important: true,
        }];
        for _ in 1..MAX_RULE_DEPTH {
            let mut parent = empty_rule();
            parent.children.push(rule);
            rule = parent;
        }
        let info = StyleInfo {
            css_id_to_style_sheet: HashMap::from([(
                0,
                StyleSheet {
                    imports: vec![1, 2],
                    rules: vec![rule],
                },
            )]),
            style_text_size_hint: 0,
        };
        let bytes = rkyv::to_bytes::<_, 4096>(&info).expect("encode populated leaf");
        let decoded = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || decode_style_info(&bytes).expect("decode populated leaf"))
            .expect("spawn small-stack caller")
            .join()
            .expect("populated input decodes on the caller's stack");
        assert_eq!(nesting_depth(&decoded), MAX_RULE_DEPTH);
        let mut leaf = &decoded.css_id_to_style_sheet[&0].rules[0];
        while let Some(child) = leaf.children.first() {
            leaf = child;
        }
        assert_eq!(
            leaf.prelude.selectors[0].components[0].value,
            "a-selector-long-enough-to-be-out-of-line"
        );
        assert!(leaf.declaration_block.declarations[0].is_important);
    }

    #[test]
    fn an_oversized_section_is_refused_before_validation() {
        let error =
            decode_style_info(&vec![0u8; MAX_SECTION_LEN + 1]).expect_err("over the length cap");
        assert!(error.to_string().contains("over the"), "{error}");
    }

    #[test]
    fn nesting_depth_counts_the_deepest_chain() {
        let mut shallow = empty_rule();
        shallow.children = vec![empty_rule()];
        let mut deep = empty_rule();
        deep.children = vec![{
            let mut middle = empty_rule();
            middle.children = vec![empty_rule()];
            middle
        }];
        let info = StyleInfo {
            css_id_to_style_sheet: HashMap::from([(
                0,
                StyleSheet {
                    imports: vec![],
                    rules: vec![shallow, deep],
                },
            )]),
            style_text_size_hint: 0,
        };
        assert_eq!(nesting_depth(&info), 3);
        assert_eq!(nesting_depth(&StyleInfo::default()), 0);
    }

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
    fn selector_text_round_trips_the_encoder_shapes() {
        let selector = Selector {
            components: vec![
                SimpleSelector {
                    kind: SimpleSelectorKind::Class,
                    value: "card".to_owned(),
                },
                SimpleSelector {
                    kind: SimpleSelectorKind::Combinator,
                    value: " ".to_owned(),
                },
                SimpleSelector {
                    kind: SimpleSelectorKind::Type,
                    value: "view".to_owned(),
                },
                SimpleSelector {
                    kind: SimpleSelectorKind::Attribute,
                    value: "[type=submit]".to_owned(),
                },
                SimpleSelector {
                    kind: SimpleSelectorKind::PseudoClass,
                    value: "nth-child(4n+1)".to_owned(),
                },
            ],
        };
        assert_eq!(
            selector.to_css_string(),
            ".card view[type=submit]:nth-child(4n+1)"
        );

        let child = Selector {
            components: vec![
                SimpleSelector {
                    kind: SimpleSelectorKind::Class,
                    value: "a".to_owned(),
                },
                SimpleSelector {
                    kind: SimpleSelectorKind::Combinator,
                    value: ">".to_owned(),
                },
                SimpleSelector {
                    kind: SimpleSelectorKind::PseudoElement,
                    value: "placeholder".to_owned(),
                },
            ],
        };
        assert_eq!(child.to_css_string(), ".a > ::placeholder");
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
