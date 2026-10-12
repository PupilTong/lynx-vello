//! rkyv 0.7 wire model for the pre-parsed CSS sections — `StyleInfo` (label
//! 2) and `CustomSectionsStyleInfo` (label 7); field and enum ordering is
//! serialized ABI.

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

/// Root of the `CustomSectionsStyleInfo` section (label 7). Mirrors
/// web-core's `RawStyleInfoMap` (lynx-stack
/// `packages/web-platform/web-core/src/template/template_sections/style_info/raw_style_info_map.
/// rs`, added by lynx-stack#4138): every custom section the compiler tagged
/// `encoding: 'CSS'`, keyed by its section name, each one an ordinary
/// [`StyleInfo`] holding the section's rule list under css id `0`
/// (`encodeCSSMap` in `ts/encode/encodeCSS.ts`).
///
/// web-core keys an `FnvHashMap`; rkyv 0.7 archives a hash map with its own
/// hash index whatever the source map's hasher was, so a `HashMap` reads the
/// same bytes.
#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleInfoMap {
    pub name_to_style_info: HashMap<String, StyleInfo>,
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
            component.write_css_string(buf);
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

impl SimpleSelector {
    /// Appends this one component's CSS text to `buf`. A selector's text is
    /// its components' texts in order, with nothing between them, so a
    /// component can be spliced into a selector by writing it between two
    /// runs of the selector's own components.
    pub fn write_css_string(&self, buf: &mut String) {
        match self.kind {
            SimpleSelectorKind::Type | SimpleSelectorKind::UnknownText => {
                buf.push_str(&self.value);
            }
            SimpleSelectorKind::Class => {
                buf.push('.');
                buf.push_str(&self.value);
            }
            SimpleSelectorKind::Id => {
                buf.push('#');
                buf.push_str(&self.value);
            }
            SimpleSelectorKind::Attribute => {
                // The encoder stores the generated attribute-selector text
                // with its brackets already included (`[type=submit]`).
                buf.push_str(&self.value);
            }
            SimpleSelectorKind::PseudoClass => {
                buf.push(':');
                buf.push_str(&self.value);
            }
            SimpleSelectorKind::PseudoElement => {
                buf.push_str("::");
                buf.push_str(&self.value);
            }
            SimpleSelectorKind::Universal => {
                buf.push('*');
            }
            SimpleSelectorKind::Combinator => {
                // The descendant combinator is encoded as a single space.
                if self.value.trim().is_empty() {
                    buf.push(' ');
                } else {
                    buf.push(' ');
                    buf.push_str(&self.value);
                    buf.push(' ');
                }
            }
        }
    }
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
    let style_info: StyleInfo =
        decode_archive(bytes, MAX_VALIDATION_DEPTH).map_err(DecodeError::StyleInfo)?;
    check_rule_depth(&style_info).map_err(DecodeError::StyleInfo)?;
    Ok(style_info)
}

/// Decodes the `CustomSectionsStyleInfo` section under the same bounds as
/// [`decode_style_info`]: the one section-length cap for the whole map, one
/// validation level more for the map around the `StyleInfo`s, and the rule
/// depth limit for every entry.
pub(crate) fn decode_style_info_map(bytes: &[u8]) -> Result<StyleInfoMap, DecodeError> {
    let map: StyleInfoMap = decode_archive(bytes, MAX_VALIDATION_DEPTH + 1)
        .map_err(DecodeError::CustomSectionsStyleInfo)?;
    for (name, style_info) in &map.name_to_style_info {
        check_rule_depth(style_info).map_err(|reason| {
            DecodeError::CustomSectionsStyleInfo(format!("{name:?}: {reason}"))
        })?;
    }
    Ok(map)
}

/// Validates and deserializes one rkyv root, refusing a section over
/// [`MAX_SECTION_LEN`] before copying it and bounding validation recursion by
/// `max_depth`.
fn decode_archive<T>(bytes: &[u8], max_depth: usize) -> Result<T, String>
where
    T: Archive,
    for<'a> T::Archived: bytecheck::CheckBytes<ArchiveValidator<'a>>
        + RkyvDeserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
    for<'a> <T::Archived as bytecheck::CheckBytes<ArchiveValidator<'a>>>::Error: std::fmt::Display,
{
    if bytes.len() > MAX_SECTION_LEN {
        return Err(format!(
            "section is {} bytes, over the {MAX_SECTION_LEN}-byte limit",
            bytes.len()
        ));
    }

    let mut aligned = rkyv::AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    // This model contains no shared pointers, so ArchiveValidator supplies
    // every check it needs. No platform-specific threads or unsafe access:
    // excessive nesting fails during validation on the calling thread.
    let mut validator = ArchiveValidator::with_max_depth(&aligned, max_depth);
    let archived = check_archived_root_with_context::<T, _>(&aligned, &mut validator)
        .map_err(|e| e.to_string())?;
    archived
        .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
        .map_err(|e| format!("{e:?}"))
}

/// Refuses a rule tree deeper than [`MAX_RULE_DEPTH`]. Validation already
/// bounded the tree; rejecting and dropping it here cannot recurse to an
/// input-controlled depth.
fn check_rule_depth(style_info: &StyleInfo) -> Result<(), String> {
    let depth = nesting_depth(style_info);
    if depth > MAX_RULE_DEPTH {
        return Err(format!(
            "rule nesting is {depth} levels deep, over the {MAX_RULE_DEPTH}-level limit"
        ));
    }
    Ok(())
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

    fn class_sheet(class: &str, value: &str) -> StyleInfo {
        let mut rule = empty_rule();
        rule.prelude.selectors = vec![Selector {
            components: vec![SimpleSelector {
                kind: SimpleSelectorKind::Class,
                value: class.to_owned(),
            }],
        }];
        rule.declaration_block.declarations = vec![ParsedDeclaration {
            property: CssProperty {
                id: CssPropertyId::BackgroundColor,
                unknown_name: None,
            },
            value_tokens: vec![ValueToken {
                token_type: token_types::IDENT_TOKEN,
                value: value.to_owned(),
            }],
            is_important: false,
        }];
        StyleInfo {
            css_id_to_style_sheet: HashMap::from([(
                0,
                StyleSheet {
                    imports: vec![],
                    rules: vec![rule],
                },
            )]),
            style_text_size_hint: 32,
        }
    }

    /// The label-7 shape web-core's `RawStyleInfoMap::encode` writes: one
    /// `StyleInfo` per section name, each holding css id 0. Encoded here the
    /// way the native converter encodes it.
    #[test]
    fn style_info_map_round_trips_two_sections() {
        let original = StyleInfoMap {
            name_to_style_info: HashMap::from([
                ("CSS".to_owned(), class_sheet("foo", "blue")),
                ("A &/中".to_owned(), class_sheet("bar", "green")),
            ]),
        };
        let bytes = rkyv::to_bytes::<_, 1024>(&original).unwrap();
        let decoded = decode_style_info_map(&bytes).unwrap();

        assert_eq!(decoded.name_to_style_info.len(), 2);
        for (name, class, value) in [("CSS", ".foo", "blue"), ("A &/中", ".bar", "green")] {
            let style_info = &decoded.name_to_style_info[name];
            assert_eq!(style_info.style_text_size_hint, 32);
            assert_eq!(style_info.css_id_to_style_sheet.len(), 1);
            let rule = &style_info.css_id_to_style_sheet[&0].rules[0];
            assert_eq!(rule.prelude.selectors[0].to_css_string(), class);
            assert_eq!(rule.declaration_block.declarations[0].value_text(), value);
        }
    }

    #[test]
    fn rejects_garbage_and_oversized_style_info_maps() {
        assert!(matches!(
            decode_style_info_map(&[0xFF; 64]),
            Err(DecodeError::CustomSectionsStyleInfo(_))
        ));
        // A valid `StyleInfo` root is not a map root.
        let style_info = rkyv::to_bytes::<_, 1024>(&class_sheet("foo", "red")).unwrap();
        assert!(matches!(
            decode_style_info_map(&style_info),
            Err(DecodeError::CustomSectionsStyleInfo(_))
        ));
        let error = decode_style_info_map(&vec![0u8; MAX_SECTION_LEN + 1])
            .expect_err("over the length cap");
        assert!(error.to_string().contains("over the"), "{error}");
    }

    /// The rule depth limit holds per entry, and the map's own level leaves
    /// a populated tree at the limit decodable.
    #[test]
    fn style_info_map_entries_keep_the_rule_depth_limit() {
        let deep = |levels: usize| {
            let mut rule = class_sheet("leaf", "red").css_id_to_style_sheet[&0].rules[0].clone();
            for _ in 1..levels {
                let mut parent = empty_rule();
                parent.children.push(rule);
                rule = parent;
            }
            StyleInfo {
                css_id_to_style_sheet: HashMap::from([(
                    0,
                    StyleSheet {
                        imports: vec![],
                        rules: vec![rule],
                    },
                )]),
                style_text_size_hint: 0,
            }
        };
        let encode = |depth| {
            rkyv::to_bytes::<_, 4096>(&StyleInfoMap {
                name_to_style_info: HashMap::from([
                    ("shallow".to_owned(), class_sheet("a", "red")),
                    ("deep".to_owned(), deep(depth)),
                ]),
            })
            .unwrap()
        };
        let at_limit = encode(MAX_RULE_DEPTH);
        let decoded = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || decode_style_info_map(&at_limit).expect("at the limit"))
            .expect("spawn small-stack caller")
            .join()
            .expect("valid input decodes on the caller's stack");
        assert_eq!(
            nesting_depth(&decoded.name_to_style_info["deep"]),
            MAX_RULE_DEPTH
        );
        let error = decode_style_info_map(&encode(MAX_RULE_DEPTH + 1)).expect_err("over the limit");
        assert!(error.to_string().contains("levels deep"), "{error}");
        assert!(error.to_string().contains("\"deep\""), "{error}");
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
