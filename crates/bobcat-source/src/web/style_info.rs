//! rkyv 0.7 wire model for the pre-parsed CSS sections — `StyleInfo` (label
//! 2) and `CustomSectionsStyleInfo` (label 7); field and enum ordering is
//! serialized ABI.

use std::collections::HashMap;

use rkyv::validation::check_archived_root_with_context;
use rkyv::validation::validators::ArchiveValidator;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};

use super::css_property::{ArchivedParsedDeclaration, DeclarationRead};
pub use super::css_property::{
    CssProperty, CssPropertyId, ParsedDeclaration, STYLE_PROPERTY_MAP, ValueToken, token_types,
};
use super::error::DecodeError;

#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleInfo {
    pub css_id_to_style_sheet: HashMap<i32, StyleSheet>,
    pub style_text_size_hint: usize,
}

#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleInfoMap {
    pub name_to_style_info: HashMap<String, StyleInfo>,
}

#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct StyleSheet {
    pub imports: Vec<i32>,
    pub rules: Vec<Rule>,
}

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

#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct RulePrelude {
    pub selectors: Vec<Selector>,
}

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

    pub fn write_css_string(&self, buf: &mut String) {
        SelectorRead::write_css_string(self, buf);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct SimpleSelector {
    pub kind: SimpleSelectorKind,
    pub value: String,
}

impl SimpleSelector {
    pub fn write_css_string(&self, buf: &mut String) {
        ComponentRead::write_css_string(self, buf);
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

#[derive(Debug, Clone, Default, Archive, RkyvDeserialize, RkyvSerialize)]
#[archive(check_bytes)]
pub struct DeclarationBlock {
    pub declarations: Vec<ParsedDeclaration>,
}

const MAX_SECTION_LEN: usize = 1 << 20;

const MAX_RULE_DEPTH: usize = 64;

const MAX_VALIDATION_DEPTH: usize = MAX_RULE_DEPTH + 8;

pub(crate) fn decode_style_info(bytes: &[u8]) -> Result<StyleInfo, DecodeError> {
    let section = AlignedSection::new(bytes).map_err(DecodeError::StyleInfo)?;
    deserialize(check_style_info(&section)?).map_err(DecodeError::StyleInfo)
}

pub(crate) fn decode_style_info_map(bytes: &[u8]) -> Result<StyleInfoMap, DecodeError> {
    let section = AlignedSection::new(bytes).map_err(DecodeError::CustomSectionsStyleInfo)?;
    deserialize(check_style_info_map(&section)?).map_err(DecodeError::CustomSectionsStyleInfo)
}

pub(crate) struct AlignedSection(rkyv::AlignedVec);

impl AlignedSection {
    pub(crate) fn new(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SECTION_LEN {
            return Err(format!(
                "section is {} bytes, over the {MAX_SECTION_LEN}-byte limit",
                bytes.len()
            ));
        }
        let mut aligned = rkyv::AlignedVec::with_capacity(bytes.len());
        aligned.extend_from_slice(bytes);
        Ok(Self(aligned))
    }
}

pub(crate) fn check_style_info(
    section: &AlignedSection,
) -> Result<&ArchivedStyleInfo, DecodeError> {
    let style_info = check_archive::<StyleInfo>(section, MAX_VALIDATION_DEPTH)
        .map_err(DecodeError::StyleInfo)?;
    check_rule_depth(style_info).map_err(DecodeError::StyleInfo)?;
    Ok(style_info)
}

pub(crate) fn check_style_info_map(
    section: &AlignedSection,
) -> Result<&ArchivedStyleInfoMap, DecodeError> {
    let map = check_archive::<StyleInfoMap>(section, MAX_VALIDATION_DEPTH + 1)
        .map_err(DecodeError::CustomSectionsStyleInfo)?;
    for (name, style_info) in map.name_to_style_info.iter() {
        check_rule_depth(style_info).map_err(|reason| {
            DecodeError::CustomSectionsStyleInfo(format!("{:?}: {reason}", name.as_str()))
        })?;
    }
    Ok(map)
}

fn check_archive<T>(section: &AlignedSection, max_depth: usize) -> Result<&T::Archived, String>
where
    T: Archive,
    for<'a> T::Archived: bytecheck::CheckBytes<ArchiveValidator<'a>>,
    for<'a> <T::Archived as bytecheck::CheckBytes<ArchiveValidator<'a>>>::Error: std::fmt::Display,
{
    let mut validator = ArchiveValidator::with_max_depth(&section.0, max_depth);
    check_archived_root_with_context::<T, _>(&section.0, &mut validator).map_err(|e| e.to_string())
}

fn deserialize<T, A>(archived: &A) -> Result<T, String>
where
    A: RkyvDeserialize<T, rkyv::de::deserializers::SharedDeserializeMap>,
{
    archived
        .deserialize(&mut rkyv::de::deserializers::SharedDeserializeMap::new())
        .map_err(|e| format!("{e:?}"))
}

fn check_rule_depth(style_info: &impl StyleInfoRead) -> Result<(), String> {
    let depth = nesting_depth(style_info);
    if depth > MAX_RULE_DEPTH {
        return Err(format!(
            "rule nesting is {depth} levels deep, over the {MAX_RULE_DEPTH}-level limit"
        ));
    }
    Ok(())
}

fn nesting_depth<S: StyleInfoRead>(style_info: &S) -> usize {
    let mut deepest = 0;
    let mut pending: Vec<(&<S::Sheet as StyleSheetRead>::Rule, usize)> = style_info
        .sheets()
        .flat_map(|(_, sheet)| sheet.rules().iter().map(|rule| (rule, 1)))
        .collect();
    while let Some((rule, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        pending.extend(rule.children().iter().map(|child| (child, depth + 1)));
    }
    deepest
}

pub(crate) trait StyleInfoRead {
    type Sheet: StyleSheetRead;
    fn sheet(&self, css_id: i32) -> Option<&Self::Sheet>;
    fn sheets(&self) -> impl Iterator<Item = (i32, &Self::Sheet)>;
}

pub(crate) trait StyleSheetRead {
    type Rule: RuleRead;
    fn imports(&self) -> &[i32];
    fn rules(&self) -> &[Self::Rule];
}

pub(crate) trait RuleRead: Sized {
    type Selector: SelectorRead;
    type Declaration: DeclarationRead;
    fn kind(&self) -> RuleKind;
    fn selectors(&self) -> &[Self::Selector];
    fn declarations(&self) -> &[Self::Declaration];
    fn children(&self) -> &[Self];
}

pub(crate) trait SelectorRead {
    type Component: ComponentRead;
    fn components(&self) -> &[Self::Component];

    fn write_css_string(&self, buf: &mut String) {
        for component in self.components() {
            component.write_css_string(buf);
        }
    }
}

pub(crate) trait ComponentRead {
    fn kind(&self) -> SimpleSelectorKind;
    fn value(&self) -> &str;

    fn write_css_string(&self, buf: &mut String) {
        let value = self.value();
        match self.kind() {
            SimpleSelectorKind::Type
            | SimpleSelectorKind::UnknownText
            | SimpleSelectorKind::Attribute => {
                buf.push_str(value);
            }
            SimpleSelectorKind::Class => {
                buf.push('.');
                buf.push_str(value);
            }
            SimpleSelectorKind::Id => {
                buf.push('#');
                buf.push_str(value);
            }
            SimpleSelectorKind::PseudoClass => {
                buf.push(':');
                buf.push_str(value);
            }
            SimpleSelectorKind::PseudoElement => {
                buf.push_str("::");
                buf.push_str(value);
            }
            SimpleSelectorKind::Universal => {
                buf.push('*');
            }
            SimpleSelectorKind::Combinator => {
                if value.trim().is_empty() {
                    buf.push(' ');
                } else {
                    buf.push(' ');
                    buf.push_str(value);
                    buf.push(' ');
                }
            }
        }
    }
}

impl StyleInfoRead for StyleInfo {
    type Sheet = StyleSheet;
    fn sheet(&self, css_id: i32) -> Option<&StyleSheet> {
        self.css_id_to_style_sheet.get(&css_id)
    }
    fn sheets(&self) -> impl Iterator<Item = (i32, &StyleSheet)> {
        self.css_id_to_style_sheet
            .iter()
            .map(|(id, sheet)| (*id, sheet))
    }
}

impl StyleInfoRead for ArchivedStyleInfo {
    type Sheet = ArchivedStyleSheet;
    fn sheet(&self, css_id: i32) -> Option<&ArchivedStyleSheet> {
        self.css_id_to_style_sheet.get(&css_id)
    }
    fn sheets(&self) -> impl Iterator<Item = (i32, &ArchivedStyleSheet)> {
        self.css_id_to_style_sheet
            .iter()
            .map(|(id, sheet)| (*id, sheet))
    }
}

impl StyleSheetRead for StyleSheet {
    type Rule = Rule;
    fn imports(&self) -> &[i32] {
        &self.imports
    }
    fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

impl StyleSheetRead for ArchivedStyleSheet {
    type Rule = ArchivedRule;
    fn imports(&self) -> &[i32] {
        &self.imports
    }
    fn rules(&self) -> &[ArchivedRule] {
        &self.rules
    }
}

impl RuleRead for Rule {
    type Selector = Selector;
    type Declaration = ParsedDeclaration;
    fn kind(&self) -> RuleKind {
        self.kind
    }
    fn selectors(&self) -> &[Selector] {
        &self.prelude.selectors
    }
    fn declarations(&self) -> &[ParsedDeclaration] {
        &self.declaration_block.declarations
    }
    fn children(&self) -> &[Self] {
        &self.children
    }
}

impl RuleRead for ArchivedRule {
    type Selector = ArchivedSelector;
    type Declaration = ArchivedParsedDeclaration;
    fn kind(&self) -> RuleKind {
        unarchive(&self.kind)
    }
    fn selectors(&self) -> &[ArchivedSelector] {
        &self.prelude.selectors
    }
    fn declarations(&self) -> &[ArchivedParsedDeclaration] {
        &self.declaration_block.declarations
    }
    fn children(&self) -> &[Self] {
        &self.children
    }
}

impl SelectorRead for Selector {
    type Component = SimpleSelector;
    fn components(&self) -> &[SimpleSelector] {
        &self.components
    }
}

impl SelectorRead for ArchivedSelector {
    type Component = ArchivedSimpleSelector;
    fn components(&self) -> &[ArchivedSimpleSelector] {
        &self.components
    }
}

impl ComponentRead for SimpleSelector {
    fn kind(&self) -> SimpleSelectorKind {
        self.kind
    }
    fn value(&self) -> &str {
        &self.value
    }
}

impl ComponentRead for ArchivedSimpleSelector {
    fn kind(&self) -> SimpleSelectorKind {
        unarchive(&self.kind)
    }
    fn value(&self) -> &str {
        &self.value
    }
}

fn unarchive<T, A: RkyvDeserialize<T, rkyv::Infallible>>(archived: &A) -> T {
    let Ok(value) = archived.deserialize(&mut rkyv::Infallible);
    value
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
        let style_info = rkyv::to_bytes::<_, 1024>(&class_sheet("foo", "red")).unwrap();
        assert!(matches!(
            decode_style_info_map(&style_info),
            Err(DecodeError::CustomSectionsStyleInfo(_))
        ));
        let error = decode_style_info_map(&vec![0u8; MAX_SECTION_LEN + 1])
            .expect_err("over the length cap");
        assert!(error.to_string().contains("over the"), "{error}");
    }

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
