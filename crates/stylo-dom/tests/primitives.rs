//! Integration tests for the `stylo-dom` primitives an embedder's API layer
//! delegates to: the generational [`Arena`], [`ElementRef`] navigation, the
//! tree-mutation primitives (`attach_at`/`detach`/`drop_subtree`) with their
//! invalidation scheduling, the read helpers, inline-style parsing, and the
//! [`ExternalState`] hook defaults.
//!
//! Invalidation *scheduling* (dirty bits making mutations reachable from the
//! root) is asserted here; invalidation *precision* (which elements a flush
//! actually restyles, via snapshots and selector flags) is behavioral and
//! covered by the style/flush integration tests.
//!
//! Everything runs against `Element<()>` (the no-op payload) except the
//! `drop_subtree` harvest test, which uses a payload type to observe what the
//! arena returns.

use stylo_dom::{Arena, Element, ElementId, ElementState, ExternalState, NodeId, PseudoState};

/// Append `child` as the last child of `parent`, via the `attach_at` primitive.
fn append<T>(arena: &mut Arena<T>, parent: ElementId, child: NodeId) {
    let index = arena.children_len(parent);
    arena.attach_at(parent, child, index);
}

/// Insert an element with the given tag (no-op payload) and return its handle.
fn insert(arena: &mut Arena<()>, tag: &str) -> ElementId {
    arena.insert(Element::new(tag, ()))
}

/// The number of declarations in an element's parsed inline style block.
fn inline_declaration_count(arena: &Arena<()>, id: ElementId) -> usize {
    arena.inline_style_declaration_count(id).unwrap()
}

#[test]
fn arena_generational_reuse() {
    let mut arena = Arena::new();
    let a = insert(&mut arena, "div");
    assert!(arena.get(a).is_some());

    arena.remove(a);
    assert!(arena.get(a).is_none());

    // The next insert reuses the freed slot with a bumped generation.
    let b = insert(&mut arena, "div");
    assert_eq!(a.index(), b.index(), "slot should have been reused");
    assert!(
        arena.get(a).is_none(),
        "the stale handle no longer resolves"
    );
    assert!(arena.get(b).is_some());
    assert_ne!(a, b);
}

#[test]
fn element_ref_navigation() {
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let container = insert(&mut arena, "div");
    append(&mut arena, root, container);
    let a = insert(&mut arena, "div");
    let b = insert(&mut arena, "div");
    let c = insert(&mut arena, "div");
    append(&mut arena, container, a);
    append(&mut arena, container, b);
    append(&mut arena, container, c);

    let cref = arena.element_ref(container).unwrap();
    assert_eq!(cref.tag(), "div");
    assert_eq!(cref.parent().unwrap().id(), root);
    let kids: Vec<_> = cref.children().map(stylo_dom::ElementRef::id).collect();
    assert_eq!(kids, vec![a, b, c]);
    assert_eq!(cref.first_child().unwrap().id(), a);
    assert_eq!(cref.last_child().unwrap().id(), c);

    assert!(arena.element_ref(a).unwrap().prev_sibling().is_none());
    assert_eq!(
        arena.element_ref(a).unwrap().next_sibling().unwrap().id(),
        b
    );
    assert_eq!(
        arena.element_ref(b).unwrap().prev_sibling().unwrap().id(),
        a
    );
    assert!(arena.element_ref(c).unwrap().next_sibling().is_none());
}

#[test]
fn text_is_a_real_dom_node_without_element_state() {
    use stylo::dom::{NodeInfo, TNode};

    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let before = insert(&mut arena, "span");
    let text = arena.insert_text("hello");
    let after = insert(&mut arena, "span");
    append(&mut arena, root, before);
    append(&mut arena, root, text);
    append(&mut arena, root, after);

    let text_ref = arena.node_ref(text).unwrap();
    assert!(!NodeInfo::is_element(&text_ref));
    assert!(NodeInfo::is_text_node(&text_ref));
    assert!(TNode::as_element(&text_ref).is_none());
    assert_eq!(text_ref.text(), Some("hello"));
    assert_eq!(text_ref.parent().unwrap().id(), root);
    assert_eq!(text_ref.prev_sibling().unwrap().id(), before);
    assert_eq!(text_ref.next_sibling().unwrap().id(), after);
    assert_eq!(TNode::owner_doc(&text_ref).id(), root);
    assert!(TNode::is_in_document(&text_ref));

    // Text cannot be reached through Element-only accessors and owns neither
    // an embedder payload nor computed style.
    assert!(arena.get(text).is_none());
    assert!(arena.element_ref(text).is_none());

    let root_ref = arena.element_ref(root).unwrap();
    let all_children = root_ref
        .child_nodes()
        .map(stylo_dom::NodeRef::id)
        .collect::<Vec<_>>();
    assert_eq!(all_children, vec![before, text, after]);

    arena.detach(text);
    let detached = arena.node_ref(text).unwrap();
    assert!(!TNode::is_in_document(&detached));
    assert_eq!(TNode::owner_doc(&detached).id(), root);
}

#[test]
fn attach_rejects_text_parents_and_stale_children_atomically() {
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let text_parent = arena.insert_text("not an Element");
    let child = insert(&mut arena, "span");

    let before_text_parent = arena.layout_revision();
    arena.attach_at(text_parent, child, 0);
    assert_eq!(arena.layout_revision(), before_text_parent);
    assert!(arena.node_ref(child).unwrap().parent().is_none());

    let stale = insert(&mut arena, "stale");
    arena.remove(stale);
    let before_stale_child = arena.layout_revision();
    arena.attach_at(root, stale, 0);
    assert_eq!(arena.layout_revision(), before_stale_child);
    assert_eq!(arena.children_len(root), 0);
}

#[test]
fn element_navigation_and_structural_siblings_skip_text() {
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let leading_text = arena.insert_text("before");
    let first = insert(&mut arena, "div");
    let middle_text = arena.insert_text("between");
    let second = insert(&mut arena, "div");
    append(&mut arena, root, leading_text);
    append(&mut arena, root, first);
    append(&mut arena, root, middle_text);
    append(&mut arena, root, second);

    let root_ref = arena.element_ref(root).unwrap();
    assert_eq!(root_ref.first_child().unwrap().id(), first);
    assert_eq!(root_ref.last_child().unwrap().id(), second);
    assert_eq!(
        root_ref
            .children()
            .map(stylo_dom::ElementRef::id)
            .collect::<Vec<_>>(),
        vec![first, second]
    );
    assert!(arena.element_ref(first).unwrap().prev_sibling().is_none());
    assert_eq!(
        arena
            .element_ref(first)
            .unwrap()
            .next_sibling()
            .unwrap()
            .id(),
        second
    );
    assert_eq!(
        arena
            .element_ref(second)
            .unwrap()
            .prev_sibling()
            .unwrap()
            .id(),
        first
    );
}

#[test]
fn empty_selector_observes_nonempty_text_and_element_children() {
    use selectors::Element as _;

    let mut arena = Arena::new();
    let root = insert(&mut arena, "div");
    let empty_text = arena.insert_text("");
    append(&mut arena, root, empty_text);

    assert!(arena.element_ref(root).unwrap().is_empty());
    assert!(arena.set_text(empty_text, "content"));
    assert!(!arena.element_ref(root).unwrap().is_empty());
    assert!(arena.set_text(empty_text, ""));
    assert!(arena.element_ref(root).unwrap().is_empty());

    // Selectors defines whitespace as character data too; only a zero-length
    // Text node is ignored by :empty.
    assert!(arena.set_text(empty_text, " "));
    assert!(!arena.element_ref(root).unwrap().is_empty());
    assert!(arena.set_text(empty_text, ""));

    let child = insert(&mut arena, "span");
    append(&mut arena, root, child);
    assert!(!arena.element_ref(root).unwrap().is_empty());
    arena.detach(child);
    assert!(arena.element_ref(root).unwrap().is_empty());
}

#[test]
fn text_tree_mutations_advance_layout_revision() {
    let mut arena = Arena::new();
    assert_eq!(arena.layout_revision(), 0);

    let root = insert(&mut arena, "div");
    let after_root = arena.layout_revision();
    assert!(after_root > 0);
    let text = arena.insert_text("a");
    let after_text = arena.layout_revision();
    assert!(after_text > after_root);
    append(&mut arena, root, text);
    let after_attach = arena.layout_revision();
    assert!(after_attach > after_text);

    assert!(arena.set_text(text, "a"));
    assert_eq!(
        arena.layout_revision(),
        after_attach,
        "an equal write is a no-op"
    );
    assert!(arena.set_text(text, "b"));
    let after_data = arena.layout_revision();
    assert!(after_data > after_attach);

    arena.detach(text);
    let after_detach = arena.layout_revision();
    assert!(after_detach > after_data);
    assert!(arena.remove_node(text).is_some());
    assert!(arena.layout_revision() > after_detach);
}

#[test]
fn public_mutable_borrows_advance_layout_revision() {
    let mut arena = Arena::new();
    let root = insert(&mut arena, "div");
    let before_element = arena.layout_revision();
    arena.get_mut(root).unwrap().tag = "changed".into();
    assert!(arena.layout_revision() > before_element);

    let text = arena.insert_text("a");
    let before_text = arena.layout_revision();
    arena.text_mut(text).unwrap().parent = Some(root);
    assert!(arena.layout_revision() > before_text);
}

#[test]
fn attach_detach_marks_reachability() {
    // `root > [before_sib, list, hint]`, `list > child`. Structural changes
    // must make the mutation site reachable from the root (the flush walks
    // `dirty_descendants` bits down); precision (which siblings actually
    // restyle) is the flush's job, driven by stylo selector flags.
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let before_sib = insert(&mut arena, "div");
    let list = insert(&mut arena, "div");
    let hint = insert(&mut arena, "div");
    append(&mut arena, root, before_sib);
    append(&mut arena, root, list);
    append(&mut arena, root, hint);
    let child = insert(&mut arena, "div");
    append(&mut arena, list, child);
    arena.clear_dirty();

    arena.detach(child);
    assert!(
        arena.get(root).unwrap().has_dirty_descendants(),
        "detaching under `list` must be reachable from the root"
    );
    assert!(
        !arena.get(before_sib).unwrap().is_style_dirty(),
        "siblings are not blanket-dirtied at mutation time"
    );
    assert!(!arena.get(hint).unwrap().is_style_dirty());

    arena.clear_dirty();
    append(&mut arena, list, child);
    assert!(arena.get(list).unwrap().has_dirty_descendants());
    assert!(arena.get(root).unwrap().has_dirty_descendants());
}

#[test]
fn note_attribute_change_marks_element_and_ancestors() {
    // `root > container > [a, b, c]`, `b > b1`. An attribute change marks the
    // element itself dirty and its ancestor chain reachable — nothing else at
    // mutation time (invalidation-set matching happens at flush, driven by
    // the pre-mutation snapshot).
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let container = insert(&mut arena, "div");
    append(&mut arena, root, container);
    let a = insert(&mut arena, "div");
    let b = insert(&mut arena, "div");
    let c = insert(&mut arena, "div");
    append(&mut arena, container, a);
    append(&mut arena, container, b);
    append(&mut arena, container, c);
    let b1 = insert(&mut arena, "div");
    append(&mut arena, b, b1);
    arena.clear_dirty();

    arena.note_attribute_change(b, "title");

    assert!(arena.get(b).unwrap().is_style_dirty());
    assert!(arena.get(container).unwrap().has_dirty_descendants());
    assert!(!arena.get(container).unwrap().is_style_dirty());
    assert!(arena.get(root).unwrap().has_dirty_descendants());
    // Siblings and descendants are not blanket-dirtied.
    assert!(!arena.get(a).unwrap().is_style_dirty());
    assert!(!arena.get(c).unwrap().is_style_dirty());
    assert!(!arena.get(b1).unwrap().is_style_dirty());
}

#[test]
fn drop_subtree_frees_and_returns_payloads() {
    /// A payload carrying an embedder-side id, to observe the harvest.
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Payload(i32);
    impl ExternalState for Payload {}

    let mut arena: Arena<Payload> = Arena::new();
    let container = arena.insert(Element::new("div", Payload(10)));
    let child = arena.insert(Element::new("div", Payload(11)));
    append(&mut arena, container, child);
    let grandchild = arena.insert(Element::new("div", Payload(12)));
    append(&mut arena, child, grandchild);
    let text = arena.insert_text("payload-free text");
    append(&mut arena, grandchild, text);

    let mut removed = arena.drop_subtree(child);
    removed.sort_unstable();
    assert_eq!(
        removed,
        vec![Payload(11), Payload(12)],
        "every freed element's payload is returned"
    );

    assert!(arena.get(child).is_none());
    assert!(arena.get(grandchild).is_none());
    assert!(arena.node_ref(text).is_none());
    // `container` is untouched (drop_subtree does not unlink from a parent).
    assert!(arena.get(container).is_some());
}

#[test]
fn ancestor_and_child_queries() {
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let container = insert(&mut arena, "div");
    append(&mut arena, root, container);
    let leaf = insert(&mut arena, "div");
    append(&mut arena, container, leaf);

    assert!(arena.is_ancestor(root, leaf));
    assert!(arena.is_ancestor(container, leaf));
    assert!(!arena.is_ancestor(leaf, root));
    assert!(arena.is_child_of(container, root));
    assert!(!arena.is_child_of(leaf, root));
    assert_eq!(arena.child_position(container, leaf), Some(0));
    assert_eq!(arena.child_position(root, leaf), None);
    assert_eq!(arena.children_len(container), 1);
}

#[test]
fn inline_style_helpers_parse_merge_and_clear() {
    let mut arena = Arena::new();
    let view = insert(&mut arena, "div");

    // `add_inline_style` parses and folds one declaration at a time.
    arena.add_inline_style(view, "color", "red");
    arena.add_inline_style(view, "width", "10px");
    assert_eq!(inline_declaration_count(&arena, view), 2);

    // An unparseable property/value is dropped, not an error.
    arena.add_inline_style(view, "definitely-not-a-property", "1");
    assert_eq!(inline_declaration_count(&arena, view), 2);

    // `set_inline_styles` replaces the whole block.
    arena.set_inline_styles(view, "display:flex");
    assert_eq!(inline_declaration_count(&arena, view), 1);

    // An empty string clears it.
    arena.set_inline_styles(view, "");
    assert!(arena.get(view).unwrap().inline_block.is_none());
}

#[test]
fn external_state_default_root_matching() {
    use selectors::Element as _;

    // The `()` payload keeps the HTML-ish default: parentless ⇒ `:root`.
    let mut arena = Arena::new();
    let root = insert(&mut arena, "html");
    let child = insert(&mut arena, "div");
    append(&mut arena, root, child);

    assert!(arena.element_ref(root).unwrap().is_root());
    assert!(!arena.element_ref(child).unwrap().is_root());
}

#[test]
fn external_state_default_attr_hooks() {
    use stylo::dom::TElement;

    // The `()` payload serves no synthetic attributes: only the real attrs
    // map answers `get_attr`.
    let mut arena = Arena::new();
    let el = insert(&mut arena, "div");
    arena
        .get_mut(el)
        .unwrap()
        .attrs
        .insert("title".into(), "hi".into());

    let elem = arena.element_ref(el).unwrap();
    let ns = stylo::Namespace::default();
    assert_eq!(
        elem.get_attr(&stylo::LocalName::from("title"), &ns),
        Some("hi".to_owned())
    );
    assert_eq!(elem.get_attr(&stylo::LocalName::from("data-x"), &ns), None);
}

#[test]
fn pseudo_state_bridges_to_element_state() {
    let state = PseudoState::HOVER.union(PseudoState::FOCUS);
    let element_state = state.to_element_state();
    assert!(element_state.contains(ElementState::HOVER));
    assert!(element_state.contains(ElementState::FOCUS));
    assert!(!element_state.contains(ElementState::ACTIVE));

    // The bridge round-trips through the three bits tracked.
    let recovered = PseudoState::from_element_state(element_state);
    assert!(recovered.contains(PseudoState::HOVER));
    assert!(recovered.contains(PseudoState::FOCUS));
    assert!(!recovered.contains(PseudoState::ACTIVE));
}
