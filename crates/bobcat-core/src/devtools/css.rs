//! The `CSS` domain of the page session: computed styles and the matched
//! rules of an element, read from the document on request. Rules are
//! reported without a style sheet id or a source range, so none of them is
//! editable.

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::json;
use terminus::link::{MatchedDeclaration, MatchedRule, MatchedStyles, StylesheetOrigin};

use super::Frontend;
use super::dom::{find, no_node};
use super::protocol::{Error, Request};
use crate::main::tree::LynxDocument;

type Node = terminus::link::Node<()>;

#[derive(Default)]
pub(crate) struct CssState {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeParams {
    node_id: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Style<'a> {
    css_properties: Vec<Property<'a>>,
    shorthand_entries: [(); 0],
}

#[derive(Serialize)]
struct Property<'a> {
    name: &'a str,
    value: &'a str,
    important: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleMatch<'a> {
    rule: Rule<'a>,
    matching_selectors: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Rule<'a> {
    selector_list: SelectorList<'a>,
    origin: &'static str,
    style: Style<'a>,
}

#[derive(Serialize)]
struct SelectorList<'a> {
    selectors: Vec<Selector<'a>>,
    text: String,
}

#[derive(Serialize)]
struct Selector<'a> {
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Inline<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_style: Option<Style<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes_style: Option<Style<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Matched<'a> {
    #[serde(flatten)]
    inline: Inline<'a>,
    #[serde(rename = "matchedCSSRules")]
    rules: Vec<RuleMatch<'a>>,
    pseudo_elements: [(); 0],
    inherited: Vec<Inherited<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Inherited<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_style: Option<Style<'a>>,
    #[serde(rename = "matchedCSSRules")]
    rules: Vec<RuleMatch<'a>>,
}

pub(crate) fn dispatch(
    _state: &mut CssState,
    document: &mut LynxDocument,
    frontend: &Frontend,
    request: &Request,
) {
    let document: &LynxDocument = document;
    let id = request.id;
    let element = || {
        let params: NodeParams = request.params()?;
        let node = find(document, params.node_id).ok_or_else(no_node)?;
        node.is_element()
            .then_some(node)
            .ok_or_else(|| Error::server("Node is not an Element"))
    };
    match request.method.as_str() {
        "CSS.enable" | "CSS.disable" => frontend.result(id, &json!({})),
        "CSS.getComputedStyleForNode" => frontend.reply(
            id,
            element().map(|node| {
                let computed: Vec<_> = document
                    .computed_style_entries(node.id(), &[], true)
                    .into_iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect();
                json!({ "computedStyle": computed })
            }),
        ),
        "CSS.getInlineStylesForNode" => frontend.reply(
            id,
            element().map(|node| json!(inline(document.matched_styles(node.id()).as_ref()))),
        ),
        "CSS.getMatchedStylesForNode" => frontend.reply(
            id,
            element().map(|node| {
                let matched = document.matched_styles(node.id());
                let ancestors: Vec<Option<MatchedStyles>> = ancestor_elements(document, node)
                    .map(|ancestor| document.matched_styles(ancestor.id()))
                    .collect();
                json!(Matched {
                    inline: inline(matched.as_ref()),
                    rules: rule_matches(matched.as_ref()),
                    pseudo_elements: [],
                    inherited: ancestors
                        .iter()
                        .map(|matched| Inherited {
                            inline_style: matched
                                .as_ref()
                                .and_then(|matched| matched.inline.as_deref())
                                .map(style),
                            rules: rule_matches(matched.as_ref()),
                        })
                        .collect(),
                })
            }),
        ),
        _ => frontend.not_found(request),
    }
}

fn ancestor_elements<'d>(
    document: &'d LynxDocument,
    node: &'d Node,
) -> impl Iterator<Item = &'d Node> {
    std::iter::successors(Some(node), |node| {
        node.parent_id().and_then(|id| document.get(id))
    })
    .skip(1)
    .filter(|node| node.is_element())
}

fn inline(matched: Option<&MatchedStyles>) -> Inline<'_> {
    Inline {
        inline_style: matched
            .and_then(|matched| matched.inline.as_deref())
            .map(style),
        attributes_style: matched
            .and_then(|matched| matched.presentational.as_deref())
            .map(style),
    }
}

fn style(declarations: &[MatchedDeclaration]) -> Style<'_> {
    Style {
        css_properties: declarations
            .iter()
            .map(|declaration| Property {
                name: &declaration.name,
                value: &declaration.value,
                important: declaration.important,
            })
            .collect(),
        shorthand_entries: [],
    }
}

fn rule_matches(matched: Option<&MatchedStyles>) -> Vec<RuleMatch<'_>> {
    matched.map_or_else(Vec::new, |matched| {
        matched.rules.iter().map(rule_match).collect()
    })
}

fn rule_match(rule: &MatchedRule) -> RuleMatch<'_> {
    let texts: Vec<&str> = rule
        .selectors
        .iter()
        .map(|selector| selector.text.as_str())
        .collect();
    RuleMatch {
        rule: Rule {
            selector_list: SelectorList {
                text: texts.join(", "),
                selectors: texts.into_iter().map(|text| Selector { text }).collect(),
            },
            origin: match rule.origin {
                StylesheetOrigin::UserAgent => "user-agent",
                StylesheetOrigin::User => "injected",
                StylesheetOrigin::Author => "regular",
            },
            style: style(&rule.declarations),
        },
        matching_selectors: rule
            .selectors
            .iter()
            .enumerate()
            .filter_map(|(index, selector)| selector.matches.then_some(index))
            .collect(),
    }
}
