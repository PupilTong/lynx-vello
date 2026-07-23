//! Integration tests for the `w3c-dom` primitives an embedder's API layer
//! delegates to: the ONE-TREE [`Document`] (raw slab-index storage, structure
//! ops, queries), `&Node` navigation, invalidation-carrying DOM setters,
//! inline-style parsing, and the let-it-crash mutation contract. Internal
//! style scheduling is asserted behaviorally by the style/flush tests rather
//! than exposed here as mutable dirty state.

mod common;

use w3c_dom::{DOCUMENT_NODE_ID, Document, Node, NodeId, NodeType};

fn test_document<T>() -> Document<T> {
    Document::new(common::device(800.0, 600.0))
}

fn node(doc: &mut Document<()>, tag: &str) -> NodeId {
    doc.create_element(tag, ())
}

#[test]
fn document_is_slot_zero_and_node_ids_are_raw_slab_indices() {
    let mut doc = test_document();
    assert_eq!(doc.root_node().id(), DOCUMENT_NODE_ID);
    assert_eq!(doc.root_node().node_type(), NodeType::Document);

    let a = node(&mut doc, "div");
    assert!(doc.get(a).is_some());
    doc.append_document_element(a);
    assert_eq!(doc.root_element().map(Node::id), Some(a));
    assert_eq!(doc.root_node().first_child().map(Node::id), Some(a));
    assert_eq!(doc.get(a).unwrap().parent_id(), Some(DOCUMENT_NODE_ID));

    doc.remove_subtree(a);
    assert!(doc.get(a).is_none());

    let b = node(&mut doc, "div");
    assert_eq!(a, b, "Slab should reuse the vacant slot");
    assert!(
        doc.get(a).is_some(),
        "the raw index now resolves to its new occupant"
    );
    assert!(doc.get(b).is_some());
}

#[test]
fn node_ref_navigation() {
    let mut doc = test_document();
    let root = node(&mut doc, "html");
    let container = node(&mut doc, "div");
    doc.append_child(root, container);
    let a = node(&mut doc, "div");
    let b = node(&mut doc, "div");
    let c = node(&mut doc, "div");
    doc.append_child(container, a);
    doc.append_child(container, b);
    doc.append_child(container, c);

    let cref = doc.get(container).unwrap();
    let div = stylo::LocalName::from("div");
    assert_eq!(cref.local_name(), Some(&div));
    assert_eq!(cref.tag_name(), Some("div"));
    assert_eq!(cref.parent().unwrap().id(), root);
    let kids: Vec<_> = cref.children().map(Node::id).collect();
    assert_eq!(kids, vec![a, b, c]);
    assert_eq!(cref.first_child().unwrap().id(), a);
    assert_eq!(cref.last_child().unwrap().id(), c);

    assert!(doc.get(a).unwrap().previous_sibling().is_none());
    assert_eq!(doc.get(a).unwrap().next_sibling().unwrap().id(), b);
    assert_eq!(doc.get(b).unwrap().previous_sibling().unwrap().id(), a);
    assert!(doc.get(c).unwrap().next_sibling().is_none());
}

#[test]
fn element_and_text_nodes_share_the_document_tree() {
    let mut doc = test_document();
    let parent = doc.create_element("p", ());
    let text = doc.create_text_node("hello", ());
    doc.append_child(parent, text);

    let element = doc.get(parent).unwrap();
    assert_eq!(element.node_type(), NodeType::Element);
    assert!(element.is_element());
    assert!(!element.is_text_node());
    assert_eq!(element.tag_name(), Some("p"));
    assert_eq!(element.text(), None);

    let text_node = doc.get(text).unwrap();
    assert_eq!(text_node.node_type(), NodeType::Text);
    assert!(!text_node.is_element());
    assert!(text_node.is_text_node());
    assert_eq!(text_node.tag_name(), None);
    assert_eq!(text_node.text(), Some("hello"));
    assert_eq!(text_node.parent_id(), Some(parent));
    assert_eq!(element.first_child().unwrap().id(), text);

    doc.set_text_node_data(text, "updated");
    assert_eq!(doc.get(text).unwrap().text(), Some("updated"));
}

#[test]
fn element_navigation_and_empty_matching_handle_text_children() {
    use selectors::Element as _;

    let mut doc = test_document();
    let parent = node(&mut doc, "div");
    let leading_text = doc.create_text_node("", ());
    let first = node(&mut doc, "span");
    let middle_text = doc.create_text_node("between", ());
    let second = node(&mut doc, "span");
    doc.append_child(parent, leading_text);
    doc.append_child(parent, first);
    doc.append_child(parent, middle_text);
    doc.append_child(parent, second);

    let parent_ref = doc.get(parent).unwrap();
    assert_eq!(parent_ref.first_child().unwrap().id(), leading_text);
    assert_eq!(parent_ref.first_element_child().unwrap().id(), first);
    assert_eq!(
        doc.get(first).unwrap().next_sibling_element().unwrap().id(),
        second
    );
    assert_eq!(
        doc.get(second)
            .unwrap()
            .prev_sibling_element()
            .unwrap()
            .id(),
        first
    );
    assert!(
        !parent_ref.is_empty(),
        "a non-empty text child makes the element non-empty"
    );

    let empty_parent = node(&mut doc, "div");
    let empty_text = doc.create_text_node("", ());
    doc.append_child(empty_parent, empty_text);
    assert!(
        doc.get(empty_parent).unwrap().is_empty(),
        "an empty text child does not affect :empty"
    );
    doc.set_text_node_data(empty_text, " ");
    assert!(
        !doc.get(empty_parent).unwrap().is_empty(),
        "whitespace character data is non-empty"
    );
}

#[test]
fn insert_before_reorders_within_one_parent() {
    let mut doc = test_document();
    let parent = node(&mut doc, "div");
    let a = node(&mut doc, "div");
    let b = node(&mut doc, "div");
    let c = node(&mut doc, "div");
    doc.append_child(parent, a);
    doc.append_child(parent, b);
    doc.append_child(parent, c);

    doc.insert_before(parent, c, Some(a));
    assert_eq!(doc.get(parent).unwrap().child_ids(), &[c, a, b]);
    assert_eq!(doc.get(c).unwrap().parent_id(), Some(parent));
}

#[test]
fn remove_subtree_frees_detaches_and_returns_payloads() {
    /// A payload carrying an embedder-side id, to observe the harvest.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Payload(i32);
    let mut doc: Document<Payload> = test_document();
    let container = doc.create_element("div", Payload(10));
    let child = doc.create_element("div", Payload(11));
    doc.append_child(container, child);
    let grandchild = doc.create_text_node("payload", Payload(12));
    doc.append_child(child, grandchild);

    let mut removed = doc.remove_subtree(child);
    removed.sort_unstable();
    assert_eq!(
        removed,
        vec![Payload(11), Payload(12)],
        "every freed node's payload is returned"
    );

    assert!(doc.get(child).is_none());
    assert!(doc.get(grandchild).is_none());
    assert!(doc.get(container).is_some());
    assert!(doc.get(container).unwrap().child_ids().is_empty());
}

#[test]
fn remove_subtree_clears_the_root_element() {
    let mut doc = test_document();
    let root = node(&mut doc, "page");
    doc.append_document_element(root);
    assert_eq!(doc.root_element().map(Node::id), Some(root));
    doc.remove_subtree(root);
    assert_eq!(doc.root_element().map(Node::id), None);
}

#[test]
fn ancestor_and_child_queries() {
    let mut doc = test_document();
    let root = node(&mut doc, "html");
    let container = node(&mut doc, "div");
    doc.append_child(root, container);
    let leaf = node(&mut doc, "div");
    doc.append_child(container, leaf);

    assert!(doc.is_ancestor(root, leaf));
    assert!(doc.is_ancestor(container, leaf));
    assert!(!doc.is_ancestor(leaf, root));
    assert_eq!(doc.child_position(root, container), Some(0));
    assert_eq!(doc.child_position(container, leaf), Some(0));
    assert_eq!(doc.child_position(root, leaf), None);
    assert_eq!(doc.get(container).unwrap().child_ids().len(), 1);
}

#[test]
fn inline_style_helpers_parse_merge_and_clear() {
    let mut doc = test_document();
    let view = node(&mut doc, "div");

    doc.add_inline_style(view, "color", "red");
    doc.add_inline_style(view, "width", "10px");
    assert_eq!(doc.inline_style_declaration_count(view), 2);

    doc.add_inline_style(view, "definitely-not-a-property", "1");
    assert_eq!(doc.inline_style_declaration_count(view), 2);

    doc.set_inline_style(view, "display:flex");
    assert_eq!(doc.inline_style_declaration_count(view), 1);

    doc.set_inline_style(view, "");
    assert_eq!(doc.inline_style_declaration_count(view), 0);
}

#[test]
fn root_matching_uses_document_structure() {
    use selectors::Element as _;

    let mut doc = test_document();
    let root = node(&mut doc, "html");
    let child = node(&mut doc, "div");
    let detached = node(&mut doc, "section");
    doc.append_child(root, child);
    doc.append_document_element(root);

    assert!(doc.get(root).unwrap().is_root());
    assert!(!doc.get(child).unwrap().is_root());
    assert!(
        !doc.get(detached).unwrap().is_root(),
        "a detached parentless element is not the document element"
    );
    assert!(doc.is_connected(root));
    assert!(doc.is_connected(child));
    assert!(!doc.is_connected(detached));
}

#[test]
fn stylo_sees_a_distinct_document_node_and_real_owner_document() {
    use stylo::dom::{TDocument as _, TNode as _};

    let mut doc = test_document();
    let root = node(&mut doc, "html");
    let detached = node(&mut doc, "section");
    doc.append_document_element(root);

    let root_node = doc.get(root).unwrap();
    let document_node = root_node.owner_doc();
    assert!(document_node.as_document().is_some());
    assert_eq!(document_node.as_node(), document_node);
    assert_eq!(document_node, doc.root_node());
    assert_eq!(root_node.parent_node(), Some(document_node));
    assert_eq!(document_node.first_child(), Some(root_node));
    assert!(root_node.is_in_document());

    let detached_node = doc.get(detached).unwrap();
    assert_eq!(detached_node.owner_doc(), document_node);
    assert_eq!(detached_node.parent_node(), None);
    assert!(!detached_node.is_in_document());
}

#[test]
fn attributes_come_only_from_the_real_map() {
    use stylo::dom::TElement;

    let mut doc = test_document();
    let el = node(&mut doc, "div");
    doc.set_attribute(el, "title", "hi");

    let elem = doc.get(el).unwrap();
    let ns = stylo::Namespace::default();
    let title = stylo::LocalName::from("title");
    assert_eq!(
        elem.attribute("title"),
        Some("hi"),
        "the accessor sees the DOM attribute"
    );
    assert_eq!(elem.get_attr(&title, &ns), Some("hi".to_owned()));
    assert_eq!(elem.get_attr(&stylo::LocalName::from("data-x"), &ns), None);
}

#[test]
fn reparenting_the_root_element_detaches_it_from_the_document() {
    let mut doc = test_document();
    let root = node(&mut doc, "page");
    doc.append_document_element(root);
    let other = node(&mut doc, "view");
    doc.append_child(other, root);

    assert_eq!(doc.root_element().map(Node::id), None);
    assert_eq!(doc.get(root).unwrap().parent_id(), Some(other));
    assert!(!doc.is_connected(root));
}

#[test]
#[should_panic(expected = "parent must be a live element")]
fn text_nodes_cannot_have_children() {
    let mut doc = test_document();
    let text = doc.create_text_node("parent", ());
    let child = node(&mut doc, "span");
    doc.append_child(text, child);
}

#[test]
#[should_panic(expected = "requires a live element")]
fn text_nodes_cannot_be_the_document_root() {
    let mut doc = test_document();
    let text = doc.create_text_node("root", ());
    doc.append_document_element(text);
}

#[test]
#[should_panic(expected = "element-only Document mutation")]
fn text_nodes_reject_element_attributes() {
    let mut doc = test_document();
    let text = doc.create_text_node("hello", ());
    doc.set_attribute(text, "title", "not an element");
}

#[test]
#[should_panic(expected = "stale NodeId")]
fn mutating_through_a_stale_handle_crashes() {
    let mut doc = test_document();
    let a = node(&mut doc, "div");
    doc.remove_subtree(a);
    assert!(doc.get(a).is_none());
    doc.set_attribute(a, "title", "boom");
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "descendant")]
fn cycle_creating_insert_crashes_in_debug() {
    let mut doc = test_document();
    let outer = node(&mut doc, "div");
    let inner = node(&mut doc, "div");
    doc.append_child(outer, inner);
    doc.append_child(inner, outer);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "reference")]
fn foreign_insert_reference_crashes_in_debug() {
    let mut doc = test_document();
    let parent = node(&mut doc, "div");
    let child = node(&mut doc, "div");
    let stranger = node(&mut doc, "div");
    doc.insert_before(parent, child, Some(stranger));
}
