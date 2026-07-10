//! Integration tests for the `stylo-dom` primitives an embedder's API layer
//! delegates to: the generational [`Arena`], [`ElementRef`] navigation, the
//! tree-mutation primitives (`attach_at`/`detach`/`drop_subtree`) with their
//! coarse invalidation, the read helpers, inline-style parsing, and the
//! [`ExternalState`] hook defaults.
//!
//! Everything runs against `Element<()>` (the no-op payload) except the
//! `drop_subtree` harvest test, which uses a payload type to observe what the
//! arena returns.

use stylo_dom::{Arena, Element, ElementId, ElementState, ExternalState, PseudoState};

/// Append `child` as the last child of `parent`, via the `attach_at` primitive.
fn append<T>(arena: &mut Arena<T>, parent: ElementId, child: ElementId) {
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
fn attach_detach_invalidates_parent_following_siblings() {
    // `root > [before_sib, list, hint]`, `list > child`. A structural change to
    // `list` dirties its subtree and its FOLLOWING siblings (`hint`) — like an
    // attribute change — covering `.list:empty + .hint`-style selectors.
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

    // Detaching `list`'s only child can flip `.list:empty` → `hint` restyles.
    arena.detach(child);
    assert!(arena.get(hint).unwrap().style_dirty);
    assert!(
        !arena.get(before_sib).unwrap().style_dirty,
        "earlier siblings are unaffected by later-sibling mutations"
    );

    arena.clear_dirty();
    // Re-attaching invalidates the same way.
    append(&mut arena, list, child);
    assert!(arena.get(hint).unwrap().style_dirty);
}

#[test]
fn mark_attribute_changed_dirties_subtree_and_following() {
    // `root > container > [a, b, c]`, `b > b1`, `c > c1`.
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
    let c1 = insert(&mut arena, "div");
    append(&mut arena, c, c1);
    arena.clear_dirty();

    arena.mark_attribute_changed(b);

    // The mutated node and its subtree are dirty.
    assert!(arena.get(b).unwrap().style_dirty);
    assert!(arena.get(b1).unwrap().style_dirty);
    // Ancestors gain dirty_descendants but not style_dirty.
    assert!(arena.get(container).unwrap().dirty_descendants);
    assert!(!arena.get(container).unwrap().style_dirty);
    assert!(arena.get(root).unwrap().dirty_descendants);
    // Following sibling's subtree is dirtied (covers + / ~ combinators).
    assert!(arena.get(c).unwrap().style_dirty);
    assert!(arena.get(c1).unwrap().style_dirty);
    // Earlier sibling is untouched.
    assert!(!arena.get(a).unwrap().style_dirty);
    assert!(!arena.get(a).unwrap().dirty_descendants);
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

    let mut removed = arena.drop_subtree(child);
    removed.sort_unstable();
    assert_eq!(
        removed,
        vec![Payload(11), Payload(12)],
        "every freed element's payload is returned"
    );

    assert!(arena.get(child).is_none());
    assert!(arena.get(grandchild).is_none());
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
