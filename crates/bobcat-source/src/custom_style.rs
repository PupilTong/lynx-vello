//! Preserve native CSS section names using the compiler's web custom-section
//! descriptors. The aggregate rkyv `StyleInfo` representation stays unchanged.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::web::style_info::{DeclarationBlock, Rule, RuleKind, StyleInfo};

/// The native fragment has already been decoded. Preserve its name with the
/// same CSS descriptor the compiler supplies to the web encoder.
pub(crate) fn named_descriptors(
    style_info: &StyleInfo,
    names: BTreeMap<String, i32>,
) -> impl Iterator<Item = (String, Value)> + '_ {
    names.into_iter().map(|(name, id)| {
        let rules = crate::lower_style::fragment_rules(style_info, id);
        (name, json!({"encoding":"CSS", "content":{"ruleList": rules.map(rule_json).collect::<Vec<_>>()}}))
    })
}

fn text(value: String) -> Value {
    let mut text = json!({"loc":{"line":0,"column":0}});
    text["value"] = Value::String(value);
    text
}

fn style_json(block: &DeclarationBlock) -> Vec<Value> {
    block
        .declarations
        .iter()
        .map(|decl| {
            let (value, important) = decl.value_and_importance();
            let value = if important {
                format!("{value} !important")
            } else {
                value
            };
            json!({"name":decl.property.name(),"value":value,
            "keyLoc":{"line":0,"column":0},"valLoc":{"line":0,"column":0}})
        })
        .collect()
}

fn rule_json(rule: &Rule) -> Value {
    let prelude = || {
        rule.prelude
            .selectors
            .iter()
            .map(crate::web::style_info::Selector::to_css_string)
            .collect::<Vec<_>>()
            .join(",")
    };
    match rule.kind {
        RuleKind::Style => {
            json!({"type":"StyleRule", "selectorText":text(prelude()), "variables":{}, "style":style_json(&rule.declaration_block)})
        }
        RuleKind::FontFace => {
            json!({"type":"FontFaceRule", "style":style_json(&rule.declaration_block)})
        }
        RuleKind::Keyframes => {
            json!({"type":"KeyframesRule", "name":text(prelude()), "styles":rule.children.iter().map(|child| {
            let key = child.prelude.selectors.iter().map(crate::web::style_info::Selector::to_css_string).collect::<Vec<_>>().join(",");
            json!({"keyText":text(key), "variables":{}, "style":style_json(&child.declaration_block)})
        }).collect::<Vec<_>>()})
        }
    }
}
