//! `resources/check-layout-th.js`, over the engine's geometry.
//!
//! Each element `checkLayout(selector)` selects is one subtest, checking the
//! `data-expected-*`, `data-offset-*` and `data-total-*` attributes on that
//! element's parent and on its whole subtree with the script's own ±1px
//! tolerance. The CSSOM-View numbers come from the layout the way a browser
//! derives them:
//!
//! - `offsetWidth`/`offsetHeight`: the border box.
//! - `offsetLeft`/`offsetTop`: the border box's position against the *offset parent*'s padding
//!   edge, where the offset parent is the nearest ancestor whose `position` is not `static`, else
//!   `<body>`. With `<body>` (when static) or no offset parent the position is against the initial
//!   containing block, as every engine reports it.
//! - `clientWidth`/`clientHeight`: the padding box (the engine's scrollbars take no layout space).
//! - `scrollWidth`/`scrollHeight`: the scrollable overflow, at least the padding box.
//! - `clientLeft`/`clientTop`: the start border.
//! - padding, margin and display: the resolved `getComputedStyle` value.

use terminus::link::{Document, NodeId};

use super::loader::Page;

/// Per-subtest outcome: `Ok` or the first failed assertion.
pub(crate) type Subtest = Result<(), String>;

const NUMERIC: &[&str] = &[
    "data-expected-width",
    "data-expected-height",
    "data-offset-x",
    "data-offset-y",
    "data-expected-client-width",
    "data-expected-client-height",
    "data-expected-scroll-width",
    "data-expected-scroll-height",
    "data-expected-bounding-client-rect-width",
    "data-expected-bounding-client-rect-height",
    "data-total-x",
    "data-total-y",
];

const STYLE: &[(&str, &str)] = &[
    ("data-expected-padding-top", "padding-top"),
    ("data-expected-padding-bottom", "padding-bottom"),
    ("data-expected-padding-left", "padding-left"),
    ("data-expected-padding-right", "padding-right"),
    ("data-expected-margin-top", "margin-top"),
    ("data-expected-margin-bottom", "margin-bottom"),
    ("data-expected-margin-left", "margin-left"),
    ("data-expected-margin-right", "margin-right"),
];

pub(crate) fn run(page: &mut Page, selectors: &[String]) -> Vec<Subtest> {
    page.dom.layout();
    let root = page.dom.document_element().id();
    let mut results = Vec::new();
    for selector in selectors {
        let Ok(nodes) = page.dom.query_selector_all(root, selector) else {
            results.push(Err(format!("selector `{selector}` does not parse")));
            continue;
        };
        for node in nodes {
            let mut outcome = Ok(());
            if let Some(parent) = parent_element(&page.dom, node) {
                outcome = check_node(page, parent);
            }
            if outcome.is_ok() {
                outcome = check_subtree(page, node);
            }
            results.push(outcome);
        }
    }
    results
}

fn parent_element(dom: &Document<()>, node: NodeId) -> Option<NodeId> {
    let parent = dom.get(node)?.parent_id()?;
    dom.get(parent)?.is_element().then_some(parent)
}

fn check_subtree(page: &Page, node: NodeId) -> Subtest {
    check_node(page, node)?;
    let children: Vec<NodeId> = page
        .dom
        .get(node)
        .map(|n| n.child_ids().to_vec())
        .unwrap_or_default();
    for child in children {
        if page
            .dom
            .get(child)
            .is_some_and(terminus::link::Node::is_element)
        {
            check_subtree(page, child)?;
        }
    }
    Ok(())
}

fn label(dom: &Document<()>, node: NodeId) -> String {
    let Some(element) = dom.get(node) else {
        return "?".into();
    };
    let mut out = element.tag_name().unwrap_or("?").to_owned();
    if let Some(id) = element.id_attribute() {
        out.push('#');
        out.push_str(id);
    }
    for class in element.classes() {
        out.push('.');
        out.push_str(class);
    }
    out
}

fn check_node(page: &Page, node: NodeId) -> Subtest {
    let dom = &page.dom;
    let Some(element) = dom.get(node) else {
        return Ok(());
    };
    for &name in NUMERIC {
        let Some(expected) = element.attribute(name) else {
            continue;
        };
        let expected_text = expected.trim().to_owned();
        let actual = metric(page, node, name);
        let Ok(expected) = expected_text.parse::<f32>() else {
            return Err(format!(
                "{}: {name}={expected_text} is not a number",
                label(dom, node)
            ));
        };
        match actual {
            Some(actual) if (actual - expected).abs() < 1.0 => {}
            Some(actual) => {
                return Err(format!(
                    "{}: {name} expected {expected} got {actual}",
                    label(dom, node)
                ));
            }
            None => return Err(format!("{}: {name}: element has no box", label(dom, node))),
        }
    }
    for &(name, property) in STYLE {
        let Some(expected) = element.attribute(name) else {
            continue;
        };
        let actual = dom
            .computed_style_text(node, property, true)
            .unwrap_or_default();
        let actual = actual.strip_suffix("px").unwrap_or(&actual);
        if actual != expected.as_ref() {
            return Err(format!(
                "{}: {property} expected {expected} got {actual}",
                label(dom, node)
            ));
        }
    }
    if let Some(expected) = element.attribute("data-expected-display") {
        let actual = dom
            .computed_style_text(node, "display", true)
            .unwrap_or_default();
        if actual != expected.as_ref() {
            return Err(format!(
                "{}: display expected {expected} got {actual}",
                label(dom, node)
            ));
        }
    }
    Ok(())
}

fn position(dom: &Document<()>, node: NodeId) -> String {
    dom.computed_style_text(node, "position", false)
        .unwrap_or_default()
}

/// CSSOM-View's offset parent, simplified: the nearest non-static
/// ancestor, else `<body>`; `None` for a fixed box and for the root.
fn offset_parent(page: &Page, node: NodeId) -> Option<NodeId> {
    if position(&page.dom, node) == "fixed" {
        return None;
    }
    let mut current = parent_element(&page.dom, node);
    while let Some(ancestor) = current {
        if Some(ancestor) == page.body {
            return Some(ancestor);
        }
        if position(&page.dom, ancestor) != "static" {
            return Some(ancestor);
        }
        current = parent_element(&page.dom, ancestor);
    }
    None
}

fn metric(page: &Page, node: NodeId, name: &str) -> Option<f32> {
    let dom = &page.dom;
    let layout = dom.rounded_layout(node)?;
    let rect = dom.bounding_client_rect(node)?;
    let client_width = layout.size.width - layout.border.left - layout.border.right;
    let client_height = layout.size.height - layout.border.top - layout.border.bottom;
    let offset = || -> Option<(f32, f32)> {
        if Some(node) == page.body {
            return Some((0.0, 0.0));
        }
        match offset_parent(page, node) {
            Some(parent) if Some(parent) != page.body || position(dom, parent) != "static" => {
                let parent_rect = dom.bounding_client_rect(parent)?;
                let parent_layout = dom.rounded_layout(parent)?;
                Some((
                    rect.origin.x - parent_rect.origin.x - parent_layout.border.left,
                    rect.origin.y - parent_rect.origin.y - parent_layout.border.top,
                ))
            }
            _ => Some((rect.origin.x, rect.origin.y)),
        }
    };
    Some(match name {
        "data-expected-width" => layout.size.width,
        "data-expected-height" => layout.size.height,
        "data-offset-x" => offset()?.0,
        "data-offset-y" => offset()?.1,
        "data-expected-client-width" => client_width,
        "data-expected-client-height" => client_height,
        "data-expected-scroll-width" => {
            (layout.content_size.width - layout.border.left).max(client_width)
        }
        "data-expected-scroll-height" => {
            (layout.content_size.height - layout.border.top).max(client_height)
        }
        "data-expected-bounding-client-rect-width" => rect.size.width,
        "data-expected-bounding-client-rect-height" => rect.size.height,
        "data-total-x" => layout.border.left + offset()?.0,
        "data-total-y" => layout.border.top + offset()?.1,
        _ => return None,
    })
}
