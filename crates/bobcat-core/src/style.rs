//! The pre-parsed author-stylesheet contract and its lowering into the
//! document's cascade.
//!
//! A `.web.bundle` ships CSS that a build step already tokenized: selector
//! components and per-declaration value tokens, not stylesheet text. Decoding
//! that container is embedder work ([`crate::resource::ResourceFetcher`]
//! answers a stylesheet request with either form), so this module owns the
//! engine-side vocabulary an embedder lowers into — [`PreparsedStyleSheet`] —
//! and the conversion from it into the document's rules.
//!
//! The conversion never produces stylesheet text. Each rule contributes one
//! selector-list parse and one value parse per declaration, which is the floor:
//! the wire format keeps attribute selectors (`[type="submit"]`) and functional
//! pseudo-classes (`:nth-child(4n+1)`) as raw text rather than decomposing
//! them, and stylo builds specified values only through its value parsers.
//! Everything above that — sheet tokenization, at-rule and declaration-block
//! parsing, and property-name resolution for the shorthand-expanding
//! properties — is skipped.

use dom::{CssDeclaration, CssKeyframe, CssRule, StylesheetOrigin};

use crate::main::tree::LynxDocument;

/// An author stylesheet whose CSS the host parsed before the engine saw it.
///
/// Rules are in cascade order: later rules win ties, exactly as in a
/// stylesheet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparsedStyleSheet {
    /// The sheet's rules, in source order.
    pub rules: Vec<PreparsedRule>,
}

impl PreparsedStyleSheet {
    /// Whether this sheet would contribute nothing to the cascade.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// One pre-parsed rule.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreparsedRule {
    /// A style rule: a selector list plus its declaration block.
    Style {
        /// The selector list, as CSS selector text.
        ///
        /// Selector text is the boundary because the pre-parsed forms this
        /// contract accepts keep attribute selectors and functional
        /// pseudo-classes as text; there is nothing finer to hand over.
        selectors: String,
        /// The rule's declarations, in source order.
        declarations: Vec<PreparsedDeclaration>,
    },
    /// An `@keyframes` rule. Names are global to the view and are not scoped.
    Keyframes {
        /// The animation name.
        name: String,
        /// The keyframe blocks, in source order.
        keyframes: Vec<PreparsedKeyframe>,
    },
    /// An `@font-face` rule.
    ///
    /// Font-face descriptors are not CSS properties, so this variant carries
    /// the descriptor block text rather than parsed declarations.
    FontFace {
        /// The descriptor block body, without the surrounding braces.
        descriptors: String,
    },
}

/// One `@keyframes` child block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparsedKeyframe {
    /// The keyframe selector text (`from`, `to`, `50%`, or a comma list).
    pub selector: String,
    /// The block's declarations, in source order.
    pub declarations: Vec<PreparsedDeclaration>,
}

/// One `property: value` declaration whose value text the host already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparsedDeclaration {
    /// The property name.
    pub property: String,
    /// The value text, without a trailing `!important`.
    pub value: String,
    /// Whether the declaration carries `!important`.
    pub important: bool,
}

fn declarations(source: &[PreparsedDeclaration]) -> impl Iterator<Item = CssDeclaration<'_>> {
    source.iter().map(|declaration| CssDeclaration {
        property: declaration.property.as_str(),
        value: declaration.value.as_str().into(),
        important: declaration.important,
    })
}

/// Lowers a pre-parsed sheet into rules the document owns.
///
/// Rules that do not parse are dropped, which is what a browser does with a
/// stylesheet: an unsupported selector or value invalidates its own rule or
/// declaration and nothing else.
fn lower(document: &LynxDocument, sheet: &PreparsedStyleSheet) -> Vec<CssRule> {
    sheet
        .rules
        .iter()
        .filter_map(|rule| match rule {
            PreparsedRule::Style {
                selectors,
                declarations: block,
            } => document.build_style_rule(selectors, declarations(block)),
            PreparsedRule::Keyframes { name, keyframes } => document.build_keyframes_rule(
                name,
                keyframes.iter().map(|keyframe| CssKeyframe {
                    selector: keyframe.selector.as_str(),
                    declarations: declarations(&keyframe.declarations).collect(),
                }),
            ),
            PreparsedRule::FontFace { descriptors } => {
                Some(document.build_font_face_rule(descriptors))
            }
        })
        .collect()
}

/// Mounts a pre-parsed author stylesheet on the document.
pub(crate) fn add_preparsed_style_sheet(document: &mut LynxDocument, sheet: &PreparsedStyleSheet) {
    let rules = lower(document, sheet);
    document.append_rules(rules);
}

/// Mounts an author stylesheet supplied as CSS text.
///
/// The CSS Syntax §3.2 decode step is applied here rather than at the fetch
/// boundary, so every caller gets it: a leading BOM is decoding metadata, and
/// left in place U+FEFF is an ident code point that fuses with the first
/// selector, costing the sheet its first rule.
pub(crate) fn add_style_sheet_text(document: &mut LynxDocument, css: &str) {
    let css = css.strip_prefix('\u{feff}').unwrap_or(css);
    document.add_stylesheet(css, StylesheetOrigin::Author);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::main::tree::{PageConfig, Viewport, new_document};

    fn document() -> LynxDocument {
        new_document(Viewport::new(393.0, 727.0), PageConfig::default())
    }

    fn declaration(property: &str, value: &str) -> PreparsedDeclaration {
        PreparsedDeclaration {
            property: property.to_owned(),
            value: value.to_owned(),
            important: false,
        }
    }

    fn important(property: &str, value: &str) -> PreparsedDeclaration {
        PreparsedDeclaration {
            important: true,
            ..declaration(property, value)
        }
    }

    fn style(selectors: &str, declarations: Vec<PreparsedDeclaration>) -> PreparsedRule {
        PreparsedRule::Style {
            selectors: selectors.to_owned(),
            declarations,
        }
    }

    #[test]
    fn a_preparsed_class_rule_reaches_computed_style() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![style(
                    ".basic",
                    vec![
                        declaration("width", "100px"),
                        declaration("height", "100px"),
                    ],
                )],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "basic");
        document.layout();

        let layout = document
            .rounded_layout(view)
            .expect("the styled element is laid out");
        assert!((layout.size.width - 100.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_selector_the_wire_format_keeps_as_text_still_matches() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![style(
                    "view[data-status=\"complete\"]:nth-child(2n)",
                    vec![declaration("width", "42px")],
                )],
            },
        );
        let page = document.document_element().id();
        for _ in 0..2 {
            let view = document.create_element("view", ());
            document.set_attribute(view, "data-status", "complete");
            document.insert_before(page, view, None);
        }
        document.layout();

        let second = document
            .get(page)
            .expect("the page is live")
            .child_ids()
            .get(1)
            .copied()
            .expect("two children were inserted");
        let layout = document.rounded_layout(second).expect("laid out");
        assert!((layout.size.width - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn important_and_source_order_follow_the_cascade() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![
                    style(".a", vec![important("width", "10px")]),
                    style(".a", vec![declaration("width", "20px")]),
                ],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.layout();

        let layout = document.rounded_layout(view).expect("laid out");
        assert!(
            (layout.size.width - 10.0).abs() < f32::EPSILON,
            "an important declaration outranks a later normal one"
        );
    }

    #[test]
    fn an_unparsable_rule_does_not_reject_the_other_rules() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![
                    style("!!! not a selector", vec![declaration("width", "1px")]),
                    style(
                        ".a",
                        vec![
                            declaration("width", "not-a-length"),
                            declaration("height", "33px"),
                        ],
                    ),
                ],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.layout();

        let layout = document.rounded_layout(view).expect("laid out");
        assert!((layout.size.height - 33.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_class_change_restyles_against_preparsed_author_rules() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![
                    style(".wide", vec![declaration("width", "300px")]),
                    style(".narrow", vec![declaration("width", "30px")]),
                ],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "wide");
        document.layout();
        assert!((document.rounded_layout(view).unwrap().size.width - 300.0).abs() < f32::EPSILON);

        document.set_classes(view, "narrow");
        document.layout();
        assert!(
            (document.rounded_layout(view).unwrap().size.width - 30.0).abs() < f32::EPSILON,
            "a class change must invalidate against author rules"
        );
    }

    #[test]
    fn an_inline_style_outranks_a_preparsed_author_rule() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![style(".a", vec![declaration("width", "10px")])],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.set_inline_style(view, "width: 55px");
        document.layout();

        let layout = document.rounded_layout(view).expect("laid out");
        assert!((layout.size.width - 55.0).abs() < f32::EPSILON);
    }

    #[test]
    fn an_id_change_restyles_against_preparsed_author_rules() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![style("#tall", vec![declaration("height", "77px")])],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.layout();
        let before = document.rounded_layout(view).unwrap().size.height;
        assert!(
            before.abs() < f32::EPSILON,
            "unmatched before the id is set"
        );

        document.set_id_attribute(view, Some("tall"));
        document.layout();
        assert!((document.rounded_layout(view).unwrap().size.height - 77.0).abs() < f32::EPSILON);
    }

    /// A BOM is decoding metadata, not the first character of a selector.
    #[test]
    fn a_byte_order_mark_does_not_cost_the_sheet_its_first_rule() {
        let mut document = document();
        add_style_sheet_text(&mut document, "\u{feff}.a { width: 44px; }");
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.layout();

        let layout = document.rounded_layout(view).expect("laid out");
        assert!((layout.size.width - 44.0).abs() < f32::EPSILON);
    }

    #[test]
    fn author_text_and_preparsed_sheets_cascade_in_mount_order() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![style(".a", vec![declaration("width", "10px")])],
            },
        );
        add_style_sheet_text(&mut document, ".a { width: 20px; }");
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.layout();

        let layout = document.rounded_layout(view).expect("laid out");
        assert!(
            (layout.size.width - 20.0).abs() < f32::EPSILON,
            "the later sheet wins the tie regardless of how it was supplied"
        );
    }

    /// No fixture bundle carries `@font-face`, so this is the only thing
    /// exercising its lowering: the rule must build, mount, and leave the rest
    /// of the sheet intact.
    #[test]
    fn a_font_face_rule_mounts_without_disturbing_the_sheet() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![
                    PreparsedRule::FontFace {
                        descriptors: "font-family:Custom;src:url(custom.ttf);".to_owned(),
                    },
                    style(".a", vec![declaration("width", "21px")]),
                ],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.layout();

        let layout = document.rounded_layout(view).expect("laid out");
        assert!((layout.size.width - 21.0).abs() < f32::EPSILON);
    }

    /// A descriptor block that parses to nothing must not cause the rule, or
    /// the whole sheet, to be rejected.
    #[test]
    fn an_empty_font_face_block_is_harmless() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![
                    PreparsedRule::FontFace {
                        descriptors: String::new(),
                    },
                    style(".a", vec![declaration("width", "22px")]),
                ],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "a");
        document.layout();

        assert!((document.rounded_layout(view).unwrap().size.width - 22.0).abs() < f32::EPSILON);
    }

    /// That the rule itself carries its name, offsets and blocks is asserted
    /// in `dom`, where the built rule is still readable. This covers the part
    /// visible from here: a keyframes rule mounts alongside style rules
    /// without disturbing them, and `animation-name` cascades.
    #[test]
    fn a_keyframes_rule_mounts_alongside_the_style_rules_that_use_it() {
        let mut document = document();
        add_preparsed_style_sheet(
            &mut document,
            &PreparsedStyleSheet {
                rules: vec![
                    PreparsedRule::Keyframes {
                        name: "spin".to_owned(),
                        keyframes: vec![
                            PreparsedKeyframe {
                                selector: "from".to_owned(),
                                declarations: vec![declaration("transform", "rotate(0deg)")],
                            },
                            PreparsedKeyframe {
                                selector: "to".to_owned(),
                                declarations: vec![declaration("transform", "rotate(360deg)")],
                            },
                        ],
                    },
                    style(".spinner", vec![declaration("animation-name", "spin")]),
                ],
            },
        );
        let page = document.document_element().id();
        let view = document.create_element("view", ());
        document.insert_before(page, view, None);
        document.set_classes(view, "spinner");
        document.layout();

        let node = document.get(view).expect("live");
        let style = node.computed_style().expect("styled");
        assert_eq!(
            style
                .get_ui()
                .animation_name_at(0)
                .as_atom()
                .map(ToString::to_string),
            Some("spin".to_owned())
        );
    }
}
