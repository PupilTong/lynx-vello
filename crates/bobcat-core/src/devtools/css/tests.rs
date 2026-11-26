//! The `CSS` domain over a real view: computed styles, matched rules and
//! inline styles of an element.

use serde_json::{Value, json};

use crate::devtools::test_support::{Client, booted};
use crate::test_support::{TestEngine, TestViewSpec};

const PAGE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const parent = __CreateView(0);
  __SetClasses(parent, 'parent');
  __SetInlineStyles(parent, 'opacity:0.5');
  __AppendElement(page, parent);
  const box = __CreateView(0);
  __SetClasses(box, 'a b');
  __SetInlineStyles(box, 'width:10px');
  __AppendElement(parent, box);
  __FlushElementTree();
};
";

const SHEET: &str = "
.a { color: red; }
.x, .b { margin-left: 3px; }
.a.b { color: blue !important; }
.parent { font-size: 20px; }
";

fn node(engine: &mut TestEngine, client: &Client, depth: usize) -> u64 {
    let root = client.result(engine, None, "DOM.getDocument", json!({"depth": -1}));
    let mut node = &root["root"];
    for _ in 0..depth {
        node = &node["children"][0];
    }
    node["nodeId"].as_u64().unwrap()
}

fn texts(rules: &Value) -> Vec<&str> {
    rules
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["rule"]["selectorList"]["text"].as_str().unwrap())
        .collect()
}

#[test]
fn computed_style_is_the_resolved_value_list() {
    let mut engine = booted(TestViewSpec::new(PAGE).with_style_sheet(SHEET));
    let client = Client::attach(&engine);
    let box_ = node(&mut engine, &client, 3);
    let computed = client.result(
        &mut engine,
        None,
        "CSS.getComputedStyleForNode",
        json!({"nodeId": box_}),
    );
    let value = |name: &str| {
        computed["computedStyle"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .map(|entry| entry["value"].clone())
    };
    assert_eq!(value("color"), Some(json!("rgb(0, 0, 255)")));
    assert_eq!(value("width"), Some(json!("10px")));
    assert_eq!(value("margin-left"), Some(json!("3px")));
    assert_eq!(value("font-size"), Some(json!("20px")));
}

#[test]
fn matched_styles_list_rules_lowest_priority_first_with_inherited_entries() {
    let mut engine = booted(TestViewSpec::new(PAGE).with_style_sheet(SHEET));
    let client = Client::attach(&engine);
    let box_ = node(&mut engine, &client, 3);
    let matched = client.result(
        &mut engine,
        None,
        "CSS.getMatchedStylesForNode",
        json!({"nodeId": box_}),
    );

    assert_eq!(
        matched["inlineStyle"],
        json!({"cssProperties": [{"name": "width", "value": "10px", "important": false}], "shorthandEntries": []})
    );
    assert_eq!(matched["pseudoElements"], json!([]));
    let rules = &matched["matchedCSSRules"];
    let all = rules.as_array().unwrap();
    assert!(
        all.iter()
            .any(|rule| rule["rule"]["origin"] == "user-agent"),
        "{rules}"
    );
    let author: Vec<&Value> = all
        .iter()
        .filter(|rule| rule["rule"]["origin"] == "regular")
        .collect();
    assert_eq!(
        author
            .iter()
            .map(|rule| rule["rule"]["selectorList"]["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![".a", ".x, .b", ".a.b"]
    );
    assert_eq!(
        author[1]["rule"]["selectorList"]["selectors"],
        json!([{"text": ".x"}, {"text": ".b"}])
    );
    assert_eq!(author[1]["matchingSelectors"], json!([1]));
    assert_eq!(author[0]["matchingSelectors"], json!([0]));
    assert_eq!(
        author[2]["rule"]["style"]["cssProperties"],
        json!([{"name": "color", "value": "blue", "important": true}])
    );
    assert!(author[0]["rule"].get("styleSheetId").is_none());
    assert!(author[0]["rule"]["style"].get("range").is_none());
    let user_agent_first = all
        .iter()
        .position(|rule| rule["rule"]["origin"] == "regular")
        .unwrap();
    assert!(
        all[..user_agent_first]
            .iter()
            .all(|rule| rule["rule"]["origin"] == "user-agent"),
        "{rules}"
    );

    let inherited = matched["inherited"].as_array().unwrap();
    assert_eq!(inherited.len(), 2, "the parent and the page: {inherited:?}");
    assert_eq!(
        inherited[0]["inlineStyle"]["cssProperties"],
        json!([{"name": "opacity", "value": "0.5", "important": false}])
    );
    assert!(texts(&inherited[0]["matchedCSSRules"]).contains(&".parent"));
    assert!(inherited[1].get("inlineStyle").is_none());
}

#[test]
fn inline_styles_are_the_style_attribute_declarations() {
    let mut engine = booted(TestViewSpec::new(PAGE).with_style_sheet(SHEET));
    let client = Client::attach(&engine);
    let box_ = node(&mut engine, &client, 3);
    let inline = client.result(
        &mut engine,
        None,
        "CSS.getInlineStylesForNode",
        json!({"nodeId": box_}),
    );
    assert_eq!(
        inline["inlineStyle"]["cssProperties"],
        json!([{"name": "width", "value": "10px", "important": false}])
    );
    assert!(inline.get("attributesStyle").is_none(), "{inline}");
    assert_eq!(
        client.result(&mut engine, None, "CSS.enable", json!({})),
        json!({})
    );
}
