//! Integration tests for the `w3c-dom` primitives an embedder's API layer
//! delegates to: the ONE-TREE [`Document`] (generational storage, structure
//! ops, queries), `&Node` navigation, invalidation scheduling carried by
//! the setters, inline-style parsing, the [`ExternalState`] hook defaults,
//! and the let-it-crash mutation contract.
//!
//! Invalidation *scheduling* (dirty bits making mutations reachable from the
//! root) is asserted here; invalidation *precision* (which nodes a flush
//! actually restyles, via snapshots and selector flags) is behavioral and
//! covered by the style/flush integration tests.
//!
//! Everything runs against `Node<()>` (the no-op payload) except the
//! `remove_subtree` harvest test, which uses a payload type to observe what
//! the document returns.

use w3c_dom::{Document, ExternalState, Node, NodeId, NodeType};

/// Create an element with the given tag (no-op payload) and return its handle.
fn node(doc: &mut Document<()>, tag: &str) -> NodeId {
    doc.create_node(tag, ())
}

#[test]
fn document_generational_reuse() {
    let mut doc = Document::new();
    let a = node(&mut doc, "div");
    assert!(doc.get(a).is_some());

    doc.remove_subtree(a);
    assert!(doc.get(a).is_none());

    // The next create reuses the freed slot with a bumped generation.
    let b = node(&mut doc, "div");
    assert_eq!(a.index(), b.index(), "slot should have been reused");
    assert!(doc.get(a).is_none(), "the stale handle no longer resolves");
    assert!(doc.get(b).is_some());
    assert_ne!(a, b);
}

#[test]
fn node_ref_navigation() {
    let mut doc = Document::new();
    let root = node(&mut doc, "html");
    let container = node(&mut doc, "div");
    doc.append(root, container);
    let a = node(&mut doc, "div");
    let b = node(&mut doc, "div");
    let c = node(&mut doc, "div");
    doc.append(container, a);
    doc.append(container, b);
    doc.append(container, c);

    let cref = doc.get(container).unwrap();
    assert_eq!(cref.tag(), Some("div"));
    assert_eq!(cref.parent().unwrap().id(), root);
    let kids: Vec<_> = cref.children().map(Node::id).collect();
    assert_eq!(kids, vec![a, b, c]);
    assert_eq!(cref.first_child().unwrap().id(), a);
    assert_eq!(cref.last_child().unwrap().id(), c);

    assert!(doc.get(a).unwrap().prev_sibling().is_none());
    assert_eq!(doc.get(a).unwrap().next_sibling().unwrap().id(), b);
    assert_eq!(doc.get(b).unwrap().prev_sibling().unwrap().id(), a);
    assert!(doc.get(c).unwrap().next_sibling().is_none());
}

#[test]
fn element_and_text_nodes_share_the_document_tree() {
    let mut doc = Document::new();
    let parent = doc.create_element("p", ());
    let text = doc.create_text_node("hello", ());
    doc.append(parent, text);

    let element = doc.get(parent).unwrap();
    assert_eq!(element.node_type(), NodeType::Element);
    assert!(element.is_element());
    assert!(!element.is_text_node());
    assert_eq!(element.tag(), Some("p"));
    assert_eq!(element.text(), None);

    let text_node = doc.get(text).unwrap();
    assert_eq!(text_node.node_type(), NodeType::Text);
    assert!(!text_node.is_element());
    assert!(text_node.is_text_node());
    assert_eq!(text_node.tag(), None);
    assert_eq!(text_node.text(), Some("hello"));
    assert_eq!(text_node.parent_id(), Some(parent));
    assert_eq!(element.first_child().unwrap().id(), text);

    doc.set_text_data(text, "updated");
    assert_eq!(doc.get(text).unwrap().text(), Some("updated"));
}

#[test]
fn element_navigation_and_empty_matching_handle_text_children() {
    use selectors::Element as _;

    let mut doc = Document::new();
    let parent = node(&mut doc, "div");
    let leading_text = doc.create_text_node("", ());
    let first = node(&mut doc, "span");
    let middle_text = doc.create_text_node("between", ());
    let second = node(&mut doc, "span");
    doc.append(parent, leading_text);
    doc.append(parent, first);
    doc.append(parent, middle_text);
    doc.append(parent, second);

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
    doc.append(empty_parent, empty_text);
    assert!(
        doc.get(empty_parent).unwrap().is_empty(),
        "an empty text child does not affect :empty"
    );
    doc.set_text_data(empty_text, " ");
    assert!(
        !doc.get(empty_parent).unwrap().is_empty(),
        "whitespace character data is non-empty"
    );
}

#[test]
fn insert_before_reorders_within_one_parent() {
    let mut doc = Document::new();
    let parent = node(&mut doc, "div");
    let a = node(&mut doc, "div");
    let b = node(&mut doc, "div");
    let c = node(&mut doc, "div");
    doc.append(parent, a);
    doc.append(parent, b);
    doc.append(parent, c);

    // Moving `c` before `a` detaches it first, then re-links.
    doc.insert_before(parent, c, Some(a));
    assert_eq!(doc.get(parent).unwrap().child_ids(), &[c, a, b]);
    assert_eq!(doc.get(c).unwrap().parent_id(), Some(parent));
}

#[test]
fn attach_detach_marks_reachability() {
    // `root > [before_sib, list, hint]`, `list > child`. Structural changes
    // must make the mutation site reachable from the root (the flush walks
    // `dirty_descendants` bits down); precision (which siblings actually
    // restyle) is the flush's job, driven by stylo selector flags.
    let mut doc = Document::new();
    let root = node(&mut doc, "html");
    let before_sib = node(&mut doc, "div");
    let list = node(&mut doc, "div");
    let hint = node(&mut doc, "div");
    doc.append(root, before_sib);
    doc.append(root, list);
    doc.append(root, hint);
    let child = node(&mut doc, "div");
    doc.append(list, child);
    doc.clear_dirty();

    doc.detach(child);
    assert!(
        doc.get(root).unwrap().has_dirty_descendants(),
        "detaching under `list` must be reachable from the root"
    );
    assert!(
        !doc.get(before_sib).unwrap().is_style_dirty(),
        "siblings are not blanket-dirtied at mutation time"
    );
    assert!(!doc.get(hint).unwrap().is_style_dirty());

    doc.clear_dirty();
    doc.append(list, child);
    assert!(doc.get(list).unwrap().has_dirty_descendants());
    assert!(doc.get(root).unwrap().has_dirty_descendants());
}

#[test]
fn attribute_change_marks_node_and_ancestors() {
    // `root > container > [a, b, c]`, `b > b1`. An attribute change marks the
    // node itself dirty and its ancestor chain reachable — nothing else at
    // mutation time (invalidation-set matching happens at flush, driven by
    // the pre-mutation snapshot the setter records).
    let mut doc = Document::new();
    let root = node(&mut doc, "html");
    let container = node(&mut doc, "div");
    doc.append(root, container);
    let a = node(&mut doc, "div");
    let b = node(&mut doc, "div");
    let c = node(&mut doc, "div");
    doc.append(container, a);
    doc.append(container, b);
    doc.append(container, c);
    let b1 = node(&mut doc, "div");
    doc.append(b, b1);
    doc.clear_dirty();

    doc.set_attribute(b, "title", "hi");

    assert!(doc.get(b).unwrap().is_style_dirty());
    assert!(doc.get(container).unwrap().has_dirty_descendants());
    assert!(!doc.get(container).unwrap().is_style_dirty());
    assert!(doc.get(root).unwrap().has_dirty_descendants());
    // Siblings and descendants are not blanket-dirtied.
    assert!(!doc.get(a).unwrap().is_style_dirty());
    assert!(!doc.get(c).unwrap().is_style_dirty());
    assert!(!doc.get(b1).unwrap().is_style_dirty());
}

#[test]
fn remove_subtree_frees_detaches_and_returns_payloads() {
    /// A payload carrying an embedder-side id, to observe the harvest.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Payload(i32);
    impl ExternalState for Payload {}

    let mut doc: Document<Payload> = Document::new();
    let container = doc.create_node("div", Payload(10));
    let child = doc.create_node("div", Payload(11));
    doc.append(container, child);
    let grandchild = doc.create_text_node("payload", Payload(12));
    doc.append(child, grandchild);

    let mut removed = doc.remove_subtree(child);
    removed.sort_unstable();
    assert_eq!(
        removed,
        vec![Payload(11), Payload(12)],
        "every freed node's payload is returned"
    );

    assert!(doc.get(child).is_none());
    assert!(doc.get(grandchild).is_none());
    // `container` survives, with the removed child unlinked.
    assert!(doc.get(container).is_some());
    assert!(doc.get(container).unwrap().child_ids().is_empty());
}

#[test]
fn remove_subtree_clears_the_root() {
    let mut doc = Document::new();
    let root = node(&mut doc, "page");
    doc.set_root(root);
    assert_eq!(doc.root(), Some(root));
    assert!(doc.needs_flush(), "a fresh root needs its initial pass");

    doc.remove_subtree(root);
    assert_eq!(doc.root(), None);
    assert!(!doc.needs_flush());
}

#[test]
fn ancestor_and_child_queries() {
    let mut doc = Document::new();
    let root = node(&mut doc, "html");
    let container = node(&mut doc, "div");
    doc.append(root, container);
    let leaf = node(&mut doc, "div");
    doc.append(container, leaf);

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
    let mut doc = Document::new();
    let view = node(&mut doc, "div");

    // `add_inline_style` parses and folds one declaration at a time.
    doc.add_inline_style(view, "color", "red");
    doc.add_inline_style(view, "width", "10px");
    assert_eq!(doc.inline_style_declaration_count(view), 2);

    // An unparseable property/value is dropped — CSS error handling, not an
    // unexpected parameter.
    doc.add_inline_style(view, "definitely-not-a-property", "1");
    assert_eq!(doc.inline_style_declaration_count(view), 2);

    // `set_inline_style` replaces the whole block.
    doc.set_inline_style(view, "display:flex");
    assert_eq!(doc.inline_style_declaration_count(view), 1);

    // An empty string clears it.
    doc.set_inline_style(view, "");
    assert_eq!(doc.inline_style_declaration_count(view), 0);
}

#[test]
fn external_state_default_root_matching() {
    use selectors::Element as _;

    // The `()` payload keeps the HTML-ish default: parentless ⇒ `:root`.
    let mut doc = Document::new();
    let root = node(&mut doc, "html");
    let child = node(&mut doc, "div");
    doc.append(root, child);

    assert!(doc.get(root).unwrap().is_root());
    assert!(!doc.get(child).unwrap().is_root());
}

#[test]
fn external_state_default_attr_hooks() {
    use stylo::dom::TElement;

    // The `()` payload serves no synthetic attributes: only the real attrs
    // map answers `get_attr`.
    let mut doc = Document::new();
    let el = node(&mut doc, "div");
    doc.set_attribute(el, "title", "hi");

    let elem = doc.get(el).unwrap();
    let ns = stylo::Namespace::default();
    assert_eq!(
        elem.attr("title"),
        Some("hi"),
        "the accessor sees the plain attribute"
    );
    assert_eq!(
        elem.get_attr(&stylo::LocalName::from("title"), &ns),
        Some("hi".to_owned())
    );
    assert_eq!(elem.get_attr(&stylo::LocalName::from("data-x"), &ns), None);
}

// --- document identity ---------------------------------------------------------

#[test]
fn cross_document_ids_fail_closed() {
    // Two documents mint identical (index, generation) sequences; the
    // document token in NodeId keeps them from ever aliasing.
    let mut doc_a = Document::new();
    let mut doc_b = Document::new();
    let a0 = node(&mut doc_a, "div");
    let b0 = node(&mut doc_b, "div");
    assert_eq!(a0.index(), b0.index());
    assert_eq!(a0.generation(), b0.generation());
    assert_ne!(a0, b0, "ids differ by document token");
    assert_ne!(a0.document_token(), b0.document_token());
    assert_eq!(doc_a.token(), a0.document_token());

    // Queries with a foreign id answer None instead of aliasing the
    // same-slot occupant.
    assert!(doc_b.get(a0).is_none());
    assert!(!doc_b.contains(a0));
}

#[test]
#[should_panic(expected = "stale or foreign NodeId")]
fn mutating_with_a_foreign_id_crashes() {
    let mut doc_a = Document::new();
    let mut doc_b = Document::new();
    let a0 = node(&mut doc_a, "div");
    let _b0 = node(&mut doc_b, "div");
    // Same slot shape, wrong tree: must crash, not mutate doc_b's node.
    doc_b.set_attribute(a0, "title", "boom");
}

#[test]
#[should_panic(expected = "document root cannot be linked under a parent")]
fn reparenting_the_root_crashes() {
    let mut doc = Document::new();
    let root = node(&mut doc, "page");
    doc.set_root(root);
    let other = node(&mut doc, "view");
    // Linking the root under another node would let a later subtree removal
    // free the root out from under the document.
    doc.append(other, root);
}

#[test]
#[should_panic(expected = "parent must be a live element")]
fn text_nodes_cannot_have_children() {
    let mut doc = Document::new();
    let text = doc.create_text_node("parent", ());
    let child = node(&mut doc, "span");
    doc.append(text, child);
}

#[test]
#[should_panic(expected = "parentless element")]
fn text_nodes_cannot_be_the_document_root() {
    let mut doc = Document::new();
    let text = doc.create_text_node("root", ());
    doc.set_root(text);
}

#[test]
#[should_panic(expected = "element-only Document mutation")]
fn text_nodes_reject_element_attributes() {
    let mut doc = Document::new();
    let text = doc.create_text_node("hello", ());
    doc.set_attribute(text, "title", "not an element");
}

// --- the let-it-crash mutation contract -------------------------------------

#[test]
#[should_panic(expected = "stale or foreign NodeId")]
fn mutating_through_a_stale_handle_crashes() {
    let mut doc = Document::new();
    let a = node(&mut doc, "div");
    doc.remove_subtree(a);
    // Queries answer `None`; mutations crash.
    assert!(doc.get(a).is_none());
    doc.set_attribute(a, "title", "boom");
}

#[test]
#[should_panic(expected = "stale or foreign NodeId")]
fn ext_mut_through_a_stale_handle_crashes() {
    let mut doc = Document::new();
    let a = node(&mut doc, "div");
    doc.remove_subtree(a);
    doc.ext_mut(a);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "descendant")]
fn cycle_creating_insert_crashes_in_debug() {
    let mut doc = Document::new();
    let outer = node(&mut doc, "div");
    let inner = node(&mut doc, "div");
    doc.append(outer, inner);
    // Linking `outer` under its own descendant must crash (debug builds).
    doc.append(inner, outer);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "reference")]
fn foreign_insert_reference_crashes_in_debug() {
    let mut doc = Document::new();
    let parent = node(&mut doc, "div");
    let child = node(&mut doc, "div");
    let stranger = node(&mut doc, "div");
    // `stranger` is not a child of `parent`.
    doc.insert_before(parent, child, Some(stranger));
}
