//! The `DOM` domain over a real view: node descriptions, the change events a
//! page update produces, and the geometry reads.

use std::cell::{Cell, RefCell};

use serde_json::{Value, json};

use crate::devtools::test_support::{Client, booted};
use crate::test_support::{TestEngine, TestViewSpec};

const TREE: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  const box = __CreateView(0);
  __SetAttribute(box, 'name', 'box');
  __SetAttribute(box, 'data-a', 'one');
  __AppendElement(page, box);
  const leaf = __CreateView(0);
  __SetAttribute(leaf, 'name', 'leaf');
  __AppendElement(box, leaf);
  const input = __CreateElement('input', 0);
  __SetAttribute(input, 'name', 'input');
  __SetAttribute(input, 'placeholder', 'hint');
  __AppendElement(box, input);
};
globalThis.updatePage = function () {};
";

const CHANGES: &str = r"
globalThis.make = function (parent, name) {
  const view = __CreateView(0);
  __SetAttribute(view, 'name', name);
  __AppendElement(parent, view);
  return view;
};
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetAttribute(page, 'name', 'page');
  const e = globalThis.e = { page };
  e.a = make(page, 'a');
  e.b = make(page, 'b');
  e.c = make(page, 'c');
  e.a1 = make(e.a, 'a1');
  e.a2 = make(e.a, 'a2');
  e.a3 = make(e.a, 'a3');
  e.a1x = make(e.a1, 'a1x');
  e.b1 = make(e.b, 'b1');
  e.c1 = make(e.c, 'c1');
  __SetAttribute(e.a, 'data-x', 'one');
  __SetAttribute(e.a, 'data-y', 'gone');
};
globalThis.updatePage = function (data) {
  const e = globalThis.e;
  switch (data.step) {
    case 'attributes':
      __SetAttribute(e.a, 'data-x', 'two');
      __SetAttribute(e.a, 'data-y', null);
      __SetAttribute(e.a, 'data-z', 'added');
      __SetAttribute(e.c1, 'data-x', 'unseen');
      break;
    case 'move':
      __AppendElement(e.b, e.a3);
      break;
    case 'reorder':
      __InsertElementBefore(e.a, e.a2, e.a1);
      break;
    case 'insert':
      __InsertElementBefore(e.a, __CreateView(0), e.a2);
      make(e.c, 'c2');
      break;
    case 'remove':
      __RemoveElement(e.page, e.b);
      break;
    case 'after':
      __SetAttribute(e.b1, 'data-x', 'unseen');
      __SetAttribute(e.a3, 'data-x', 'unseen');
      __SetAttribute(e.a, 'data-x', 'three');
      break;
    case 'placeholder':
      __SetAttribute(e.input, 'placeholder', 'other');
      break;
    case 'touch':
      __SetAttribute(e.a, 'data-x', String(data.value));
      break;
  }
};
";

struct Session {
    client: Client,
    last: Cell<i64>,
}

impl Session {
    fn new(engine: &mut TestEngine) -> Self {
        let session = Self {
            client: Client::attach(engine),
            last: Cell::new(0),
        };
        session.sync(engine);
        session
    }

    fn result(&self, engine: &mut TestEngine, method: &str, params: Value) -> Value {
        self.client.result(engine, None, method, params)
    }

    fn error(&self, engine: &mut TestEngine, method: &str, params: Value) -> Value {
        self.client.error(engine, None, method, params)
    }

    fn sync(&self, engine: &mut TestEngine) -> Vec<Value> {
        let from = self.last.get();
        let to = from + 1;
        self.last.set(to);
        self.client
            .session
            .send(json!({"id": to, "method": "CSS.enable"}).to_string());
        self.client
            .recorder
            .wait_pumping(Some(engine), "the sync answer", |value| value["id"] == to);
        let events = RefCell::new(Vec::new());
        let started = Cell::new(from == 0);
        self.client.recorder.wait_for("the sync answer", |value| {
            if value["id"] == to {
                return true;
            }
            if value["id"] == from {
                started.set(true);
            } else if started.get()
                && let Some(method) = value["method"].as_str()
                && method.starts_with("DOM.")
            {
                events.borrow_mut().push(json!([method, value["params"]]));
            }
            false
        });
        events.into_inner()
    }

    fn step(&self, engine: &mut TestEngine, data: &Value) -> Vec<Value> {
        engine
            .view
            .update_data(data.to_string(), String::new())
            .unwrap();
        engine
            .probe_document(|_| ())
            .expect("the update was applied");
        self.sync(engine)
    }
}

#[test]
fn a_shadow_tree_is_connected_through_its_host() {
    let mut engine = booted(TestViewSpec::new(TREE));
    let session = Session::new(&mut engine);
    let tree = session.result(
        &mut engine,
        "DOM.getDocument",
        json!({"depth": -1, "pierce": true}),
    )["root"]
        .clone();
    let input = named(&tree, "input").unwrap();
    let shadow = &input["shadowRoots"][0];
    let text = &shadow["children"][1]["children"][0]["children"][0];
    let bits = |node: &Value| node["nodeId"].as_u64().unwrap();
    let (input, shadow, text) = (bits(input), bits(shadow), bits(text));
    let (connected, parent) = engine
        .probe_document(move |document| {
            let id = |bits| terminus::link::NodeId::from_bits(bits).unwrap();
            (
                document.is_connected(id(text)),
                document
                    .get(id(shadow))
                    .and_then(terminus::link::Node::parent_id),
            )
        })
        .unwrap();
    assert!(connected);
    assert_eq!(parent.map(terminus::link::NodeId::to_bits), Some(input));
}

fn named<'v>(node: &'v Value, name: &str) -> Option<&'v Value> {
    let attributes = node["attributes"].as_array();
    if attributes.is_some_and(|attributes| {
        attributes
            .chunks(2)
            .any(|pair| pair[0] == "name" && pair[1] == name)
    }) {
        return Some(node);
    }
    ["children", "shadowRoots"]
        .iter()
        .filter_map(|field| node[field].as_array())
        .flatten()
        .find_map(|child| named(child, name))
}

fn id_of(root: &Value, name: &str) -> u64 {
    named(root, name).unwrap_or_else(|| panic!("no node named {name} in {root}"))["nodeId"]
        .as_u64()
        .unwrap()
}

#[test]
fn get_document_describes_the_tree_to_the_depth_asked() {
    let mut engine = booted(TestViewSpec::new(TREE));
    let session = Session::new(&mut engine);

    let root = &session.result(&mut engine, "DOM.getDocument", json!({}))["root"];
    assert_eq!(root["nodeType"], 9);
    assert_eq!(root["nodeName"], "#document");
    assert_eq!(root["localName"], "");
    assert_eq!(root["frameId"], "page");
    assert_eq!(root["nodeId"], root["backendNodeId"]);
    assert!(root.get("parentId").is_none());
    let page = &root["children"][0];
    assert_eq!(root["childNodeCount"], 1);
    assert_eq!(page["nodeName"], "page");
    assert_eq!(page["localName"], "page");
    assert_eq!(page["nodeType"], 1);
    assert_eq!(page["parentId"], root["nodeId"]);
    assert_eq!(page["childNodeCount"], 1);
    assert!(page.get("children").is_none(), "{page}");

    let root = &session.result(&mut engine, "DOM.getDocument", json!({"depth": 3}))["root"];
    let page = &root["children"][0];
    let box_ = &page["children"][0];
    assert_eq!(box_["attributes"], json!(["name", "box", "data-a", "one"]));
    assert_eq!(box_["childNodeCount"], 2);
    let leaf = &box_["children"][0];
    assert_eq!(leaf["childNodeCount"], 0);
    assert!(leaf.get("children").is_none(), "{leaf}");
    let input = &box_["children"][1];
    assert_eq!(input["nodeName"], "input");
    assert_eq!(input["localName"], "input");
    assert_eq!(
        input["children"],
        json!([]),
        "a host is sent with its children"
    );
    let shadow = &input["shadowRoots"][0];
    assert_eq!(shadow["nodeType"], 11);
    assert_eq!(shadow["nodeName"], "#document-fragment");
    assert_eq!(shadow["shadowRootType"], "open");
    assert_eq!(shadow["parentId"], input["nodeId"]);
    assert_eq!(shadow["childNodeCount"], 2);
    assert!(
        shadow.get("children").is_none(),
        "no pierce: the shadow root's children are not sent: {shadow}"
    );

    let root = &session.result(
        &mut engine,
        "DOM.getDocument",
        json!({"depth": 5, "pierce": true}),
    )["root"];
    let shadow = &named(root, "input").unwrap()["shadowRoots"][0];
    let block = &shadow["children"][1];
    let pseudo = &block["children"][0];
    assert_eq!(pseudo["childNodeCount"], 1);
    let text = &pseudo["children"][0];
    assert_eq!(text["nodeType"], 3, "an only text child is sent: {pseudo}");
    assert_eq!(text["nodeName"], "#text");
    assert_eq!(text["nodeValue"], "hint");
    assert_eq!(text["parentId"], pseudo["nodeId"]);
    assert!(text.get("childNodeCount").is_none());

    let whole = &session.result(&mut engine, "DOM.getDocument", json!({"depth": -1}))["root"];
    assert!(named(whole, "leaf").unwrap().get("children").is_some());

    assert_eq!(
        session.error(&mut engine, "DOM.getDocument", json!({"depth": 0}))["code"],
        -32000
    );
}

#[test]
fn request_child_nodes_sends_set_child_nodes() {
    let mut engine = booted(TestViewSpec::new(TREE));
    let session = Session::new(&mut engine);
    let root = session.result(&mut engine, "DOM.getDocument", json!({}))["root"].clone();
    let page = root["children"][0]["nodeId"].as_u64().unwrap();

    assert_eq!(
        session.result(
            &mut engine,
            "DOM.requestChildNodes",
            json!({"nodeId": page})
        ),
        json!({})
    );
    let set = session.client.event(&mut engine, None, "DOM.setChildNodes");
    assert_eq!(set["parentId"], page);
    let box_ = &set["nodes"][0];
    assert_eq!(box_["attributes"][1], "box");
    assert_eq!(box_["parentId"], page);
    assert!(box_.get("children").is_none());

    session.result(
        &mut engine,
        "DOM.requestChildNodes",
        json!({"nodeId": page, "depth": -1}),
    );
    let set = session.client.event(&mut engine, None, "DOM.setChildNodes");
    assert_eq!(set["parentId"], box_["nodeId"]);
    assert_eq!(set["nodes"][0]["children"], json!([]));

    let unknown = id_of(
        &session.result(&mut engine, "DOM.getDocument", json!({"depth": -1}))["root"],
        "leaf",
    );
    session.result(&mut engine, "DOM.getDocument", json!({}));
    let error = session.error(
        &mut engine,
        "DOM.requestChildNodes",
        json!({"nodeId": unknown}),
    );
    assert_eq!(error["code"], -32000);
    assert_eq!(error["message"], "Could not find node with given id");
}

fn changes_session(engine: &mut TestEngine) -> (Session, Value) {
    let session = Session::new(engine);
    let whole = session.result(engine, "DOM.getDocument", json!({"depth": -1}))["root"].clone();
    session.result(engine, "DOM.getDocument", json!({"depth": 2}));
    session.result(
        engine,
        "DOM.requestChildNodes",
        json!({"nodeId": id_of(&whole, "a"), "depth": -1}),
    );
    session.result(
        engine,
        "DOM.requestChildNodes",
        json!({"nodeId": id_of(&whole, "b")}),
    );
    session.sync(engine);
    (session, whole)
}

#[test]
fn each_change_under_a_sent_parent_is_reported() {
    let mut engine = booted(TestViewSpec::new(CHANGES));
    let (session, tree) = changes_session(&mut engine);
    let id = |name| id_of(&tree, name);

    assert_eq!(
        session.step(&mut engine, &json!({"step": "attributes"})),
        vec![
            json!(["DOM.attributeRemoved", {"nodeId": id("a"), "name": "data-y"}]),
            json!(["DOM.attributeModified", {"nodeId": id("a"), "name": "data-x", "value": "two"}]),
            json!(["DOM.attributeModified", {"nodeId": id("a"), "name": "data-z", "value": "added"}]),
        ]
    );

    let moved = session.step(&mut engine, &json!({"step": "move"}));
    assert_eq!(moved.len(), 2, "{moved:?}");
    assert_eq!(
        moved[0],
        json!(["DOM.childNodeRemoved", {"parentNodeId": id("a"), "nodeId": id("a3")}])
    );
    assert_eq!(moved[1][0], "DOM.childNodeInserted");
    assert_eq!(moved[1][1]["parentNodeId"], id("b"));
    assert_eq!(moved[1][1]["previousNodeId"], id("b1"));
    assert_eq!(moved[1][1]["node"]["nodeId"], id("a3"));
    assert_eq!(moved[1][1]["node"]["attributes"], json!(["name", "a3"]));

    let reordered = session.step(&mut engine, &json!({"step": "reorder"}));
    assert_eq!(reordered.len(), 2, "{reordered:?}");
    assert_eq!(
        reordered[0],
        json!(["DOM.childNodeRemoved", {"parentNodeId": id("a"), "nodeId": id("a2")}])
    );
    assert_eq!(reordered[1][0], "DOM.childNodeInserted");
    assert_eq!(reordered[1][1]["parentNodeId"], id("a"));
    assert_eq!(reordered[1][1]["previousNodeId"], 0);
    assert_eq!(reordered[1][1]["node"]["nodeId"], id("a2"));
    assert_eq!(reordered[1][1]["node"]["childNodeCount"], 0);

    let inserted = session.step(&mut engine, &json!({"step": "insert"}));
    assert_eq!(inserted.len(), 2, "{inserted:?}");
    assert_eq!(
        inserted[0],
        json!(["DOM.childNodeCountUpdated", {"nodeId": id("c"), "childNodeCount": 2}])
    );
    assert_eq!(inserted[1][0], "DOM.childNodeInserted");
    assert_eq!(inserted[1][1]["parentNodeId"], id("a"));
    assert_eq!(inserted[1][1]["previousNodeId"], 0);

    assert_eq!(
        session.step(&mut engine, &json!({"step": "remove"})),
        vec![json!(["DOM.childNodeRemoved", {"parentNodeId": id("page"), "nodeId": id("b")}])]
    );
    assert_eq!(
        session.step(&mut engine, &json!({"step": "after"})),
        vec![
            json!(["DOM.attributeModified", {"nodeId": id("a"), "name": "data-x", "value": "three"}])
        ],
        "a removed subtree, with the node moved into it, is forgotten"
    );
}

#[test]
fn a_text_change_is_reported_for_a_sent_text_node() {
    let mut engine = booted(TestViewSpec::new(&format!(
        "{CHANGES}
globalThis.renderPage = (render => function () {{
  render();
  globalThis.e.input = __CreateElement('input', 0);
  __SetAttribute(globalThis.e.input, 'placeholder', 'hint');
  __AppendElement(globalThis.e.c, globalThis.e.input);
}})(globalThis.renderPage);
"
    )));
    let session = Session::new(&mut engine);
    let root = session.result(
        &mut engine,
        "DOM.getDocument",
        json!({"depth": -1, "pierce": true}),
    )["root"]
        .clone();
    let input = &named(&root, "c").unwrap()["children"][1];
    let pseudo = &input["shadowRoots"][0]["children"][1]["children"][0];
    let text = pseudo["children"][0]["nodeId"].clone();

    assert_eq!(
        session.step(&mut engine, &json!({"step": "placeholder"})),
        vec![
            json!(["DOM.attributeModified", {"nodeId": input["nodeId"], "name": "placeholder", "value": "other"}]),
            json!(["DOM.characterDataModified", {"nodeId": text, "characterData": "other"}]),
        ]
    );
}

#[test]
fn nothing_is_reported_without_a_document_or_after_disable() {
    let mut engine = booted(TestViewSpec::new(CHANGES));
    let session = Session::new(&mut engine);
    session.result(&mut engine, "DOM.enable", json!({}));
    assert_eq!(
        session.step(&mut engine, &json!({"step": "touch", "value": 1})),
        Vec::<Value>::new()
    );

    session.result(&mut engine, "DOM.getDocument", json!({"depth": 2}));
    assert_eq!(
        session
            .step(&mut engine, &json!({"step": "touch", "value": 2}))
            .len(),
        1
    );
    session.result(&mut engine, "DOM.disable", json!({}));
    assert_eq!(
        session.step(&mut engine, &json!({"step": "touch", "value": 3})),
        Vec::<Value>::new()
    );
    assert_eq!(
        session.error(
            &mut engine,
            "DOM.pushNodesByBackendIdsToFrontend",
            json!({"backendNodeIds": [1]})
        )["message"],
        "Document needs to be requested first"
    );
}

#[test]
fn push_nodes_by_backend_ids_sends_the_path_top_down() {
    let mut engine = booted(TestViewSpec::new(CHANGES));
    let session = Session::new(&mut engine);
    let tree = session.result(&mut engine, "DOM.getDocument", json!({"depth": -1}))["root"].clone();
    let id = |name| id_of(&tree, name);
    session.result(&mut engine, "DOM.getDocument", json!({}));
    session.sync(&mut engine);

    let answer = session.result(
        &mut engine,
        "DOM.pushNodesByBackendIdsToFrontend",
        json!({"backendNodeIds": [id("a1x"), 7_777_777, id("page")]}),
    );
    assert_eq!(answer["nodeIds"], json!([id("a1x"), 0, id("page")]));
    let parents: Vec<Value> = session
        .sync(&mut engine)
        .into_iter()
        .map(|event| {
            assert_eq!(event[0], "DOM.setChildNodes");
            event[1]["parentId"].clone()
        })
        .collect();
    assert_eq!(
        parents,
        vec![json!(id("page")), json!(id("a")), json!(id("a1"))]
    );
}

const BOXES: &str = r"
globalThis.renderPage = function () {
  const page = __CreatePage('card', 0);
  __SetInlineStyles(page, 'display:flex;flex-direction:column;align-items:flex-start');
  const box = __CreateView(0);
  __SetAttribute(box, 'name', 'box');
  __SetInlineStyles(box, 'box-sizing:content-box;width:100px;height:50px;margin:5px 6px 7px 8px;border-style:solid;border-width:1px 2px 3px 4px;padding:10px 20px 30px 40px');
  __AppendElement(page, box);
  const scroller = __CreateView(0);
  __SetAttribute(scroller, 'name', 'scroller');
  __SetInlineStyles(scroller, 'display:flex;flex-direction:column;overflow:scroll;width:200px;height:100px');
  __AppendElement(page, scroller);
  for (const name of ['first', 'second']) {
    const card = __CreateView(0);
    __SetAttribute(card, 'name', name);
    __SetInlineStyles(card, 'flex-shrink:0;width:200px;height:100px');
    __AppendElement(scroller, card);
  }
  __FlushElementTree();
};
";

#[test]
fn box_model_quads_follow_the_border_padding_and_margin_edges() {
    let mut engine = booted(TestViewSpec::new(BOXES));
    let session = Session::new(&mut engine);
    let tree = session.result(&mut engine, "DOM.getDocument", json!({"depth": -1}))["root"].clone();
    let model = &session.result(
        &mut engine,
        "DOM.getBoxModel",
        json!({"nodeId": id_of(&tree, "box")}),
    )["model"];
    assert_eq!(
        model["margin"],
        json!([0.0, 0.0, 180.0, 0.0, 180.0, 106.0, 0.0, 106.0])
    );
    assert_eq!(
        model["border"],
        json!([8.0, 5.0, 174.0, 5.0, 174.0, 99.0, 8.0, 99.0])
    );
    assert_eq!(
        model["padding"],
        json!([12.0, 6.0, 172.0, 6.0, 172.0, 96.0, 12.0, 96.0])
    );
    assert_eq!(
        model["content"],
        json!([52.0, 16.0, 152.0, 16.0, 152.0, 66.0, 52.0, 66.0])
    );
    assert_eq!(model["width"], 166);
    assert_eq!(model["height"], 94);

    let second = id_of(&tree, "second");
    let scroller = id_of(&tree, "scroller");
    let border = |engine: &mut TestEngine| {
        session.result(engine, "DOM.getBoxModel", json!({"backendNodeId": second}))["model"]
            ["border"]
            .clone()
    };
    assert_eq!(border(&mut engine)[1], 206.0);
    engine
        .probe_document(move |document| {
            let scroller = terminus::link::NodeId::from_bits(scroller).unwrap();
            document.scroll_to(scroller, terminus::link::Vector2D::new(0.0, 40.0));
        })
        .unwrap();
    assert_eq!(border(&mut engine)[1], 166.0);
}

#[test]
fn node_for_location_answers_the_frontmost_element() {
    let mut engine = booted(TestViewSpec::new(BOXES));
    let session = Session::new(&mut engine);
    let tree = session.result(&mut engine, "DOM.getDocument", json!({"depth": -1}))["root"].clone();
    let answer = session.result(
        &mut engine,
        "DOM.getNodeForLocation",
        json!({"x": 60, "y": 30}),
    );
    assert_eq!(answer["backendNodeId"], id_of(&tree, "box"));
    assert_eq!(answer["nodeId"], id_of(&tree, "box"));
    assert_eq!(answer["frameId"], "page");

    session.result(&mut engine, "DOM.getDocument", json!({}));
    let answer = session.result(
        &mut engine,
        "DOM.getNodeForLocation",
        json!({"x": 60, "y": 30}),
    );
    assert_eq!(answer["backendNodeId"], id_of(&tree, "box"));
    assert!(answer.get("nodeId").is_none(), "{answer}");

    let error = session.error(
        &mut engine,
        "DOM.getNodeForLocation",
        json!({"x": -50, "y": -50}),
    );
    assert_eq!(error["message"], "No node found at given location");
}

#[test]
fn stale_ids_and_ids_of_another_kind_are_errors() {
    let mut engine = booted(TestViewSpec::new(&format!(
        "{CHANGES}
globalThis.renderPage = (render => function () {{
  render();
  const input = __CreateElement('input', 0);
  __SetAttribute(input, 'placeholder', 'hint');
  __AppendElement(globalThis.e.c, input);
}})(globalThis.renderPage);
"
    )));
    let session = Session::new(&mut engine);
    let tree = session.result(
        &mut engine,
        "DOM.getDocument",
        json!({"depth": -1, "pierce": true}),
    )["root"]
        .clone();
    let text = named(&tree, "c").unwrap()["children"][1]["shadowRoots"][0]["children"][1]["children"][0]
        ["children"][0]["nodeId"]
        .as_u64()
        .unwrap();
    let removed = id_of(&tree, "b");
    session.step(&mut engine, &json!({"step": "remove"}));
    let absent = (1_u64 << 40) | 3;

    for bad in [removed, absent, 0] {
        for (method, params) in [
            ("DOM.requestChildNodes", json!({"nodeId": bad})),
            ("DOM.getBoxModel", json!({"nodeId": bad})),
            ("CSS.getComputedStyleForNode", json!({"nodeId": bad})),
            ("CSS.getMatchedStylesForNode", json!({"nodeId": bad})),
            ("CSS.getInlineStylesForNode", json!({"nodeId": bad})),
        ] {
            let error = session.error(&mut engine, method, params);
            assert_eq!(error["code"], -32000, "{method} {bad}");
            assert_eq!(
                error["message"], "Could not find node with given id",
                "{method}"
            );
        }
        assert_eq!(
            session.error(
                &mut engine,
                "DOM.getBoxModel",
                json!({"backendNodeId": bad})
            )["message"],
            "No node found for given backend id"
        );
    }
    assert_eq!(
        session.result(
            &mut engine,
            "DOM.pushNodesByBackendIdsToFrontend",
            json!({"backendNodeIds": [removed, absent]})
        )["nodeIds"],
        json!([0, 0])
    );

    assert_eq!(
        session.error(&mut engine, "DOM.getBoxModel", json!({"nodeId": text}))["message"],
        "Could not compute box model."
    );
    assert_eq!(
        session.result(
            &mut engine,
            "DOM.requestChildNodes",
            json!({"nodeId": text})
        ),
        json!({})
    );
    for method in [
        "CSS.getComputedStyleForNode",
        "CSS.getMatchedStylesForNode",
        "CSS.getInlineStylesForNode",
    ] {
        assert_eq!(
            session.error(&mut engine, method, json!({"nodeId": text}))["message"],
            "Node is not an Element",
            "{method}"
        );
    }
    assert_eq!(
        session.error(&mut engine, "DOM.getBoxModel", json!({}))["code"],
        -32000
    );
    assert_eq!(
        session.error(&mut engine, "DOM.getBoxModel", json!({"nodeId": -1}))["code"],
        -32602
    );
    assert_eq!(
        session.error(&mut engine, "DOM.setAttributeValue", json!({}))["code"],
        -32601
    );
    assert_eq!(
        session.error(&mut engine, "CSS.setStyleTexts", json!({}))["code"],
        -32601
    );
}
