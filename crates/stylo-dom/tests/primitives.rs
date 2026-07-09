//! Integration tests for the `stylo-dom` primitives the PAPI layer delegates
//! to: the generational [`Arena`], [`WidgetRef`] navigation, the tree-mutation
//! primitives (`attach_at`/`detach`/`drop_subtree`) with their coarse
//! invalidation, the read helpers, and inline-style parsing.

use stylo_dom::{Arena, ElementState, PseudoState, Widget, WidgetId, WidgetKind};

/// Append `child` as the last child of `parent`, via the `attach_at` primitive.
fn append(arena: &mut Arena, parent: WidgetId, child: WidgetId) {
    let index = arena.children_len(parent);
    arena.attach_at(parent, child, index);
}

/// Insert a widget of `kind`/`tag` and return its handle.
fn insert(arena: &mut Arena, kind: WidgetKind, tag: &str) -> WidgetId {
    arena.insert(Widget::new(kind, tag))
}

/// The number of declarations in an element's parsed inline style block.
fn inline_declaration_count(arena: &Arena, id: WidgetId) -> usize {
    let guard = arena.shared_lock().read();
    let block = arena
        .get(id)
        .unwrap()
        .inline_block
        .as_ref()
        .expect("element has an inline block");
    block.read_with(&guard).declarations().len()
}

#[test]
fn arena_generational_reuse() {
    let mut arena = Arena::new();
    let a = insert(&mut arena, WidgetKind::View, "view");
    assert!(arena.get(a).is_some());

    arena.remove(a);
    assert!(arena.get(a).is_none());

    // The next insert reuses the freed slot with a bumped generation.
    let b = insert(&mut arena, WidgetKind::View, "view");
    assert_eq!(a.index(), b.index(), "slot should have been reused");
    assert!(
        arena.get(a).is_none(),
        "the stale handle no longer resolves"
    );
    assert!(arena.get(b).is_some());
    assert_ne!(a, b);
}

#[test]
fn unique_ids_are_monotonic_and_one_based() {
    let mut arena = Arena::new();
    let a = insert(&mut arena, WidgetKind::Page, "page");
    let b = insert(&mut arena, WidgetKind::View, "view");
    assert_eq!(arena.get(a).unwrap().unique_id, 1);
    assert_eq!(arena.get(b).unwrap().unique_id, 2);
}

#[test]
fn widget_ref_navigation() {
    let mut arena = Arena::new();
    let page = insert(&mut arena, WidgetKind::Page, "page");
    let container = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, page, container);
    let a = insert(&mut arena, WidgetKind::View, "view");
    let b = insert(&mut arena, WidgetKind::View, "view");
    let c = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, container, a);
    append(&mut arena, container, b);
    append(&mut arena, container, c);

    let cref = arena.widget_ref(container).unwrap();
    assert_eq!(cref.kind(), WidgetKind::View);
    assert_eq!(cref.tag(), "view");
    assert_eq!(cref.parent().unwrap().id(), page);
    let kids: Vec<_> = cref.children().map(stylo_dom::WidgetRef::id).collect();
    assert_eq!(kids, vec![a, b, c]);
    assert_eq!(cref.first_child().unwrap().id(), a);
    assert_eq!(cref.last_child().unwrap().id(), c);

    assert!(arena.widget_ref(a).unwrap().prev_sibling().is_none());
    assert_eq!(arena.widget_ref(a).unwrap().next_sibling().unwrap().id(), b);
    assert_eq!(arena.widget_ref(b).unwrap().prev_sibling().unwrap().id(), a);
    assert!(arena.widget_ref(c).unwrap().next_sibling().is_none());
}

#[test]
fn attach_detach_invalidates_parent_following_siblings() {
    // `page > [before_sib, list, hint]`, `list > child`. A structural change to
    // `list` dirties its subtree and its FOLLOWING siblings (`hint`) — like an
    // attribute change — covering `.list:empty + .hint`-style selectors.
    let mut arena = Arena::new();
    let page = insert(&mut arena, WidgetKind::Page, "page");
    let before_sib = insert(&mut arena, WidgetKind::View, "view");
    let list = insert(&mut arena, WidgetKind::View, "view");
    let hint = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, page, before_sib);
    append(&mut arena, page, list);
    append(&mut arena, page, hint);
    let child = insert(&mut arena, WidgetKind::View, "view");
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
    // `page > container > [a, b, c]`, `b > b1`, `c > c1`.
    let mut arena = Arena::new();
    let page = insert(&mut arena, WidgetKind::Page, "page");
    let container = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, page, container);
    let a = insert(&mut arena, WidgetKind::View, "view");
    let b = insert(&mut arena, WidgetKind::View, "view");
    let c = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, container, a);
    append(&mut arena, container, b);
    append(&mut arena, container, c);
    let b1 = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, b, b1);
    let c1 = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, c, c1);
    arena.clear_dirty();

    arena.mark_attribute_changed(b);

    // The mutated node and its subtree are dirty.
    assert!(arena.get(b).unwrap().style_dirty);
    assert!(arena.get(b1).unwrap().style_dirty);
    // Ancestors gain dirty_descendants but not style_dirty.
    assert!(arena.get(container).unwrap().dirty_descendants);
    assert!(!arena.get(container).unwrap().style_dirty);
    assert!(arena.get(page).unwrap().dirty_descendants);
    // Following sibling's subtree is dirtied (covers + / ~ combinators).
    assert!(arena.get(c).unwrap().style_dirty);
    assert!(arena.get(c1).unwrap().style_dirty);
    // Earlier sibling is untouched.
    assert!(!arena.get(a).unwrap().style_dirty);
    assert!(!arena.get(a).unwrap().dirty_descendants);
}

#[test]
fn drop_subtree_frees_and_reports_unique_ids() {
    let mut arena = Arena::new();
    let container = insert(&mut arena, WidgetKind::View, "view");
    let child = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, container, child);
    let grandchild = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, child, grandchild);

    let child_uid = arena.get(child).unwrap().unique_id;
    let grandchild_uid = arena.get(grandchild).unwrap().unique_id;

    let mut removed = arena.drop_subtree(child);
    removed.sort_unstable();
    let mut expected = vec![child_uid, grandchild_uid];
    expected.sort_unstable();
    assert_eq!(
        removed, expected,
        "every freed element's unique_id is reported"
    );

    assert!(arena.get(child).is_none());
    assert!(arena.get(grandchild).is_none());
    // `container` is untouched (drop_subtree does not unlink from a parent).
    assert!(arena.get(container).is_some());
}

#[test]
fn ancestor_and_child_queries() {
    let mut arena = Arena::new();
    let page = insert(&mut arena, WidgetKind::Page, "page");
    let container = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, page, container);
    let leaf = insert(&mut arena, WidgetKind::View, "view");
    append(&mut arena, container, leaf);

    assert!(arena.is_ancestor(page, leaf));
    assert!(arena.is_ancestor(container, leaf));
    assert!(!arena.is_ancestor(leaf, page));
    assert!(arena.is_child_of(container, page));
    assert!(!arena.is_child_of(leaf, page));
    assert_eq!(arena.child_position(container, leaf), Some(0));
    assert_eq!(arena.child_position(page, leaf), None);
    assert_eq!(arena.children_len(container), 1);
}

#[test]
fn inline_style_helpers_parse_merge_and_clear() {
    let mut arena = Arena::new();
    let view = insert(&mut arena, WidgetKind::View, "view");

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
fn widget_kind_tag_mapping() {
    assert_eq!(WidgetKind::from_tag("list-item"), WidgetKind::ListItem);
    assert_eq!(WidgetKind::from_tag("none"), WidgetKind::NoneElement);
    assert_eq!(WidgetKind::from_tag("marquee"), WidgetKind::Unknown);
    assert_eq!(WidgetKind::Page.tag_name(), "page");
    assert_eq!(WidgetKind::ScrollView.tag_name(), "scroll-view");
    assert_eq!(WidgetKind::Unknown.tag_name(), "unknown");
}

#[test]
fn pseudo_state_bridges_to_element_state() {
    let state = PseudoState::HOVER.union(PseudoState::FOCUS);
    let element_state = state.to_element_state();
    assert!(element_state.contains(ElementState::HOVER));
    assert!(element_state.contains(ElementState::FOCUS));
    assert!(!element_state.contains(ElementState::ACTIVE));

    // The bridge round-trips through the three bits Lynx tracks.
    let recovered = PseudoState::from_element_state(element_state);
    assert!(recovered.contains(PseudoState::HOVER));
    assert!(recovered.contains(PseudoState::FOCUS));
    assert!(!recovered.contains(PseudoState::ACTIVE));
}
