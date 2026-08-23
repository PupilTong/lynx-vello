//! Integration tests for the `dom` primitives an embedder's API layer
//! delegates to: the ONE-TREE [`Document`] (raw slab-index storage, structure
//! ops, queries), `&Node` navigation, invalidation-carrying DOM setters,
//! inline-style parsing, and the let-it-crash mutation contract. Internal
//! style scheduling is asserted behaviorally by the style/flush tests rather
//! than exposed here as mutable dirty state.

mod common;

use dom::{Document, Node, NodeId, StylesheetOrigin};

fn test_document() -> Document<()> {
    Document::new(common::device(800.0, 600.0), "page", ())
}

fn node(doc: &mut Document<()>, tag: &str) -> NodeId {
    doc.create_element(tag, ())
}

#[test]
fn a_removed_node_id_is_retired_rather_than_handed_to_the_next_node() {
    let mut doc = test_document();
    assert!(doc.root_node().is_document());
    let document_id = doc.root_node().id();
    let root = doc.document_element().id();
    assert_eq!(doc.root_node().first_child().map(Node::id), Some(root));
    assert_eq!(doc.get(root).unwrap().parent_id(), Some(document_id));

    let a = node(&mut doc, "div");
    doc.append_child(root, a);
    assert_eq!(doc.get(a).unwrap().parent_id(), Some(root));

    doc.drop_subtree(a);
    assert!(doc.get(a).is_none());

    let b = node(&mut doc, "div");
    assert_ne!(a, b, "a retired id is never issued again");
    assert!(
        doc.get(a).is_none(),
        "so the old id keeps resolving to nothing, however many nodes come after it"
    );
    assert!(doc.get(b).is_some());
    assert_eq!(doc.document_element().id(), root);
}

#[test]
fn node_ref_navigation() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let container = node(&mut doc, "div");
    doc.append_child(root, container);
    let a = node(&mut doc, "div");
    let b = node(&mut doc, "div");
    let c = node(&mut doc, "div");
    doc.append_child(container, a);
    doc.append_child(container, b);
    doc.append_child(container, c);

    let cref = doc.get(container).unwrap();
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
    assert!(element.is_element());
    assert!(!element.is_text_node());
    assert_eq!(element.tag_name(), Some("p"));
    assert_eq!(element.text(), None);

    let text_node = doc.get(text).unwrap();
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
fn drop_subtree_frees_detaches_and_returns_payloads() {
    /// A payload carrying an embedder-side id, to observe the harvest.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Payload(i32);
    let mut doc: Document<Payload> =
        Document::new(common::device(800.0, 600.0), "page", Payload(1));
    let container = doc.create_element("div", Payload(10));
    let child = doc.create_element("div", Payload(11));
    doc.append_child(container, child);
    let grandchild = doc.create_text_node("payload", Payload(12));
    doc.append_child(child, grandchild);

    let mut removed = doc.drop_subtree(child);
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
fn drop_element_frees_one_node_and_leaves_its_element_children_allocated() {
    /// A payload carrying an embedder-side id, to observe the harvest.
    #[derive(Debug, PartialEq, Eq)]
    struct Payload(i32);
    let mut doc: Document<Payload> =
        Document::new(common::device(800.0, 600.0), "page", Payload(1));
    let container = doc.create_element("div", Payload(10));
    let child = doc.create_element("div", Payload(11));
    doc.append_child(container, child);
    let grandchild = doc.create_element("div", Payload(12));
    doc.append_child(child, grandchild);

    assert_eq!(
        doc.drop_element(child),
        Payload(11),
        "only the dropped element's own payload comes back"
    );

    assert!(doc.get(child).is_none());
    assert!(
        doc.get(container).unwrap().child_ids().is_empty(),
        "the dropped element is gone from its parent's child list"
    );
    let orphan = doc.get(grandchild).expect("the child outlives its parent");
    assert_eq!(orphan.payload(), &Payload(12));
    assert_eq!(
        orphan.parent_id(),
        None,
        "and is left parentless rather than naming a freed node"
    );

    doc.append_child(container, grandchild);
    assert_eq!(doc.get(container).unwrap().child_ids(), &[grandchild]);
}

/// The other half of the rule: an element is what an embedder names, so it
/// survives its parent as a detached root — a text node is not, so leaving it
/// behind would leak it, and it goes with the node that owns it.
#[test]
fn drop_element_frees_the_text_nodes_under_it() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let container = node(&mut doc, "div");
    doc.append_child(root, container);
    let run = doc.create_text_node("payload", ());
    doc.append_child(container, run);
    let element = node(&mut doc, "div");
    doc.append_child(container, element);

    doc.drop_element(container);

    assert!(doc.get(container).is_none());
    assert!(doc.get(run).is_none(), "the run goes with its owner");
    assert_eq!(
        doc.get(element).map(dom::Node::parent_id),
        Some(None),
        "the element child stays, detached"
    );
}

#[test]
fn remove_element_unlinks_without_freeing() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let container = node(&mut doc, "div");
    doc.append_child(root, container);
    let leaf = node(&mut doc, "div");
    doc.append_child(container, leaf);

    doc.remove_element(container);

    assert!(
        doc.get(container)
            .is_some_and(|node| node.parent_id().is_none()),
        "the removed node stays allocated, just parentless"
    );
    assert_eq!(
        doc.get(container).unwrap().child_ids(),
        &[leaf],
        "and keeps its own subtree"
    );
    assert!(!doc.is_connected(leaf));
    assert!(doc.get(root).unwrap().child_ids().is_empty());

    doc.append_child(root, container);
    assert!(doc.is_connected(leaf), "the same ids re-insert intact");
}

#[test]
#[should_panic(expected = "cannot remove the permanent document element")]
fn the_document_element_cannot_be_removed() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    doc.drop_subtree(root);
}

#[test]
#[should_panic(expected = "cannot drop the permanent document element")]
fn the_document_element_cannot_be_dropped_on_its_own() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    doc.drop_element(root);
}

#[test]
fn ancestor_and_child_queries() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let container = node(&mut doc, "div");
    doc.append_child(root, container);
    let leaf = node(&mut doc, "div");
    doc.append_child(container, leaf);

    assert!(doc.is_ancestor(root, leaf));
    assert!(doc.is_ancestor(container, leaf));
    assert!(!doc.is_ancestor(leaf, root));
    assert_eq!(doc.get(root).unwrap().child_ids(), &[container]);
    assert_eq!(doc.get(container).unwrap().child_ids(), &[leaf]);
}

#[test]
fn inline_style_setter_parses_replaces_and_clears_observable_style() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let view = node(&mut doc, "div");
    doc.append_child(root, view);

    doc.set_inline_style(view, "color: red; definitely-not-a-property: 1");
    doc.layout();
    assert_eq!(
        doc.get(view).unwrap().attribute("style"),
        Some("color: red; definitely-not-a-property: 1")
    );
    assert_eq!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(255, 0, 0),
    );

    doc.set_inline_style(view, "color: blue");
    doc.layout();
    assert_eq!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );

    doc.set_inline_style(view, "");
    doc.layout();
    assert_eq!(doc.get(view).unwrap().attribute("style"), Some(""));
    assert_ne!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );
}

#[test]
fn inline_style_property_updates_merge_remove_and_reject_invalid_values() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let view = node(&mut doc, "div");
    doc.append_child(root, view);

    doc.set_inline_style(view, "color: red; width: 10px !important");
    doc.set_inline_style_property(view, "color", "blue");
    doc.set_inline_style_property(view, "width", "20px");
    doc.layout();

    let style = doc.get(view).unwrap().attribute("style").unwrap();
    assert!(style.contains("color: blue"));
    assert!(style.contains("width: 20px"));
    assert!(!style.contains("important"));
    assert_eq!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );

    let unchanged = style.to_owned();
    doc.set_inline_style_property(view, "definitely-not-a-property", "1");
    doc.set_inline_style_property(view, "color", "green !important");
    assert_eq!(doc.get(view).unwrap().attribute("style"), Some(&*unchanged));

    doc.set_inline_style_property(view, "color", "");
    doc.layout();
    let style = doc.get(view).unwrap().attribute("style").unwrap();
    assert!(!style.contains("color"));
    assert!(style.contains("width: 20px"));
    assert_ne!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );

    doc.set_inline_style_property(view, "width", "");
    assert_eq!(doc.get(view).unwrap().attribute("style"), Some(""));
}

#[test]
fn inline_style_property_updates_custom_properties_and_descendant_cascade() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let child = node(&mut doc, "div");
    doc.append_child(root, child);
    doc.set_inline_style(child, "color: var(--theme-color)");

    doc.set_inline_style_property(root, "--theme-color", "red");
    doc.layout();
    assert_eq!(
        doc.get(child)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(255, 0, 0),
    );

    doc.set_inline_style_property(root, "--theme-color", "blue");
    doc.layout();
    assert_eq!(
        doc.get(child)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );
}

#[test]
fn inline_style_property_updates_expand_and_remove_shorthands() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let view = node(&mut doc, "div");
    doc.append_child(root, view);
    doc.set_inline_style(view, "color: red; margin-left: 1px");

    doc.set_inline_style_property(view, "margin", "2px 3px 4px 5px");
    let style = doc.get(view).unwrap().attribute("style").unwrap();
    assert!(style.contains("color: red"));
    assert!(style.contains("margin: 2px 3px 4px 5px"));

    doc.set_inline_style_property(view, "margin", "");
    let style = doc.get(view).unwrap().attribute("style").unwrap();
    assert!(style.contains("color: red"));
    assert!(!style.contains("margin"));
}

#[test]
fn inline_style_property_update_invalidates_style_attribute_selectors() {
    let mut doc = test_document();
    doc.add_stylesheet(
        r#"[style*="width: 20px"] { color: blue; }"#,
        StylesheetOrigin::Author,
    );
    let root = doc.document_element().id();
    let view = node(&mut doc, "div");
    doc.append_child(root, view);
    doc.set_inline_style(view, "width: 10px");
    doc.layout();
    assert_ne!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );

    doc.set_inline_style_property(view, "width", "20px");
    doc.layout();
    assert_eq!(
        doc.get(view)
            .unwrap()
            .computed_style()
            .unwrap()
            .clone_color(),
        common::rgb(0, 0, 255),
    );
}

#[test]
fn root_matching_uses_document_structure() {
    use selectors::Element as _;

    let mut doc = test_document();
    let root = doc.document_element().id();
    let child = node(&mut doc, "div");
    let detached = node(&mut doc, "section");
    doc.append_child(root, child);

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
    let root = doc.document_element().id();
    let detached = node(&mut doc, "section");

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
#[should_panic(
    expected = "the permanent document element cannot be removed from the document node"
)]
fn the_document_element_cannot_be_reparented() {
    let mut doc = test_document();
    let root = doc.document_element().id();
    let other = node(&mut doc, "view");
    doc.append_child(other, root);
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
#[should_panic(expected = "element-only Document method")]
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
    doc.drop_subtree(a);
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
