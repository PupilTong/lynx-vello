//! Integration tests for the `lynx-widget` Element-PAPI surface.

use lynx_widget::{EventKind, PseudoState, WidgetError, WidgetKind, WidgetTree};

/// Build `page > container > [a, b, c]` and return the handles.
fn three_children() -> (WidgetTree, TestTree) {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(container, page).unwrap();
    let a = doc.create_view();
    let b = doc.create_view();
    let c = doc.create_view();
    doc.append_element(a, container).unwrap();
    doc.append_element(b, container).unwrap();
    doc.append_element(c, container).unwrap();
    (
        doc,
        TestTree {
            page,
            container,
            a,
            b,
            c,
        },
    )
}

struct TestTree {
    page: lynx_widget::WidgetId,
    container: lynx_widget::WidgetId,
    a: lynx_widget::WidgetId,
    b: lynx_widget::WidgetId,
    c: lynx_widget::WidgetId,
}

#[test]
fn tree_building_and_navigation() {
    let (doc, t) = three_children();

    // Kinds / tags round-trip.
    assert_eq!(doc.widget(t.page).unwrap().ext.kind, WidgetKind::Page);
    assert_eq!(doc.get_tag(t.container), Some("view"));

    // Parent / child structure via WidgetRef.
    let page = doc.widget_ref(t.page).unwrap();
    assert!(page.parent().is_none());
    let children: Vec<_> = page.children().map(lynx_widget::WidgetRef::id).collect();
    assert_eq!(children, vec![t.container]);

    let container = doc.widget_ref(t.container).unwrap();
    assert_eq!(container.parent().unwrap().id(), t.page);
    let kids: Vec<_> = container
        .children()
        .map(lynx_widget::WidgetRef::id)
        .collect();
    assert_eq!(kids, vec![t.a, t.b, t.c]);
    assert_eq!(container.first_child().unwrap().id(), t.a);
    assert_eq!(container.last_child().unwrap().id(), t.c);

    // Siblings.
    let a = doc.widget_ref(t.a).unwrap();
    assert!(a.prev_sibling().is_none());
    assert_eq!(a.next_sibling().unwrap().id(), t.b);
    assert_eq!(
        doc.widget_ref(t.b).unwrap().next_sibling().unwrap().id(),
        t.c
    );
    assert!(doc.widget_ref(t.c).unwrap().next_sibling().is_none());
    assert_eq!(
        doc.widget_ref(t.b).unwrap().prev_sibling().unwrap().id(),
        t.a
    );

    // Tree-level navigation getters.
    assert_eq!(doc.first_element(t.container), Some(t.a));
    assert_eq!(doc.next_element(t.a), Some(t.b));
    assert_eq!(doc.next_element(t.c), None);
    assert_eq!(doc.get_parent(t.a), Some(t.container));
    assert_eq!(doc.get_page_element(), Some(t.page));
}

#[test]
fn text_and_raw_text() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let text = doc.create_text();
    doc.append_element(text, page).unwrap();
    let raw = doc.create_raw_text("hello");
    doc.append_element(raw, text).unwrap();

    assert_eq!(doc.widget(text).unwrap().ext.kind, WidgetKind::Text);
    let raw_node = doc.widget(raw).unwrap();
    assert_eq!(raw_node.ext.kind, WidgetKind::RawText);
    assert_eq!(raw_node.text.as_deref(), Some("hello"));
}

#[test]
fn create_element_classifies_tag() {
    let mut doc = WidgetTree::new();
    let li = doc.create_element("list-item");
    let unknown = doc.create_element("marquee");
    assert_eq!(doc.widget(li).unwrap().ext.kind, WidgetKind::ListItem);
    assert_eq!(doc.widget(unknown).unwrap().ext.kind, WidgetKind::Unknown);
    assert_eq!(doc.get_tag(unknown), Some("marquee"));
}

#[test]
fn insert_element_before_semantics() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let a = doc.create_view();
    let c = doc.create_view();
    doc.append_element(a, page).unwrap();
    doc.append_element(c, page).unwrap();

    // Insert before an existing reference.
    let b = doc.create_view();
    doc.insert_element_before(b, page, Some(c)).unwrap();
    // before = None appends.
    let d = doc.create_view();
    doc.insert_element_before(d, page, None).unwrap();

    let order: Vec<_> = doc
        .widget_ref(page)
        .unwrap()
        .children()
        .map(lynx_widget::WidgetRef::id)
        .collect();
    assert_eq!(order, vec![a, b, c, d]);

    // Re-inserting a current child reorders it (detach-then-insert).
    doc.insert_element_before(a, page, Some(c)).unwrap();
    let order: Vec<_> = doc
        .widget_ref(page)
        .unwrap()
        .children()
        .map(lynx_widget::WidgetRef::id)
        .collect();
    assert_eq!(order, vec![b, a, c, d]);

    // A reference that is not a child errors.
    let stray = doc.create_view();
    let orphan = doc.create_view();
    let err = doc.insert_element_before(stray, page, Some(orphan));
    assert_eq!(err, Err(WidgetError::BadInsertReference(orphan)));

    // `insertBefore(n, n)` keeps `n` exactly where it is (DOM pre-insert
    // resolves the reference to n's next sibling) — not a move-to-end.
    doc.insert_element_before(b, page, Some(b)).unwrap();
    let order: Vec<_> = doc
        .widget_ref(page)
        .unwrap()
        .children()
        .map(lynx_widget::WidgetRef::id)
        .collect();
    assert_eq!(order, vec![b, a, c, d]);
    // ... and errors when n is not a child of the target parent.
    let outsider = doc.create_view();
    assert_eq!(
        doc.insert_element_before(outsider, page, Some(outsider)),
        Err(WidgetError::BadInsertReference(outsider))
    );
}

#[test]
fn tree_mutations_mark_reachability() {
    // Structural mutations must make the mutation site reachable from the
    // page root (the flush walks `dirty_descendants` down); which siblings
    // actually restyle is decided at flush time from stylo's selector flags
    // (`.list:empty + .hint`-style rules set HAS_EMPTY_SELECTOR on `list`
    // during matching).
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let before_sib = doc.create_view();
    let list = doc.create_view();
    let hint = doc.create_view();
    doc.append_element(before_sib, page).unwrap();
    doc.append_element(list, page).unwrap();
    doc.append_element(hint, page).unwrap();
    let child = doc.create_view();
    doc.append_element(child, list).unwrap();
    doc.clear_dirty();
    assert!(!doc.has_dirty());

    doc.remove_element(list, child).unwrap();
    assert!(doc.has_dirty(), "removal must schedule flush work");
    assert!(
        !doc.widget(before_sib).unwrap().is_style_dirty(),
        "siblings are not blanket-dirtied at mutation time"
    );

    doc.clear_dirty();
    doc.append_element(child, list).unwrap();
    assert!(doc.has_dirty(), "insertion must schedule flush work");
    assert!(doc.widget(list).unwrap().has_dirty_descendants());
}

#[test]
fn remove_detaches_and_destroy_frees() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(container, page).unwrap();
    let child = doc.create_view();
    doc.append_element(child, container).unwrap();
    let grandchild = doc.create_view();
    doc.append_element(grandchild, child).unwrap();
    let sibling = doc.create_view();
    doc.append_element(sibling, container).unwrap();

    let child_uid = doc.get_element_unique_id(child).unwrap();
    let grandchild_uid = doc.get_element_unique_id(grandchild).unwrap();

    // PAPI remove = detach: the subtree stays alive and re-insertable
    // (web-core's __RemoveElement is DOM removeChild; list recycling
    // re-attaches removed subtrees).
    doc.remove_element(container, child).unwrap();
    assert!(doc.widget(child).is_some());
    assert_eq!(doc.get_parent(child), None);
    assert_eq!(doc.get_parent(grandchild), Some(child));
    assert_eq!(doc.element_by_unique_id(child_uid), Some(child));
    let kids: Vec<_> = doc
        .widget_ref(container)
        .unwrap()
        .children()
        .map(lynx_widget::WidgetRef::id)
        .collect();
    assert_eq!(kids, vec![sibling]);

    // Re-inserting the detached subtree works.
    doc.append_element(child, container).unwrap();
    assert_eq!(doc.get_parent(child), Some(container));
    doc.remove_element(container, child).unwrap();

    // Explicit destruction frees the subtree and the unique_id index.
    doc.destroy_element(child).unwrap();
    assert!(doc.widget(child).is_none());
    assert!(doc.widget(grandchild).is_none());
    assert_eq!(doc.element_by_unique_id(child_uid), None);
    assert_eq!(doc.element_by_unique_id(grandchild_uid), None);

    // Removing a non-child errors.
    let other = doc.create_view();
    assert_eq!(
        doc.remove_element(container, other),
        Err(WidgetError::NotAChild {
            parent: container,
            child: other,
        })
    );
}

#[test]
fn replace_element_keeps_position() {
    let (mut doc, t) = three_children();
    let new = doc.create_view();
    doc.replace_element(new, t.b).unwrap();

    let order: Vec<_> = doc
        .widget_ref(t.container)
        .unwrap()
        .children()
        .map(lynx_widget::WidgetRef::id)
        .collect();
    assert_eq!(order, vec![t.a, new, t.c]);
    // Like DOM replaceChild, the old node survives, detached.
    assert!(doc.widget(t.b).is_some());
    assert_eq!(doc.get_parent(t.b), None);
    assert_eq!(doc.get_parent(new), Some(t.container));

    // Replacing a detached element is a no-op (DOM replaceWith on a
    // parentless node).
    let another = doc.create_view();
    doc.replace_element(another, t.b).unwrap();
    assert_eq!(doc.get_parent(another), None);
}

#[test]
fn generation_safety_after_reuse() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let a = doc.create_view();
    doc.append_element(a, page).unwrap();

    doc.destroy_element(a).unwrap();
    // The next created element reuses the freed slot with a bumped generation.
    let b = doc.create_view();
    assert_eq!(a.index(), b.index(), "slot should have been reused");

    // The stale handle no longer resolves; the new one does.
    assert!(doc.widget(a).is_none());
    assert!(doc.widget(b).is_some());
    assert_ne!(a, b);
}

#[test]
fn dirty_propagation_from_set_classes() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(container, page).unwrap();
    let a = doc.create_view();
    let b = doc.create_view();
    let c = doc.create_view();
    doc.append_element(a, container).unwrap();
    doc.append_element(b, container).unwrap();
    doc.append_element(c, container).unwrap();
    let b1 = doc.create_view();
    doc.append_element(b1, b).unwrap();
    let c1 = doc.create_view();
    doc.append_element(c1, c).unwrap();

    // Establish a clean baseline (what a restyle pass would leave behind).
    doc.clear_dirty();
    assert!(!doc.has_dirty());

    doc.set_classes(b, "highlighted").unwrap();

    // The mutated node itself is dirty; ancestors gain reachability bits.
    assert!(doc.widget(b).unwrap().is_style_dirty());
    assert!(doc.widget(container).unwrap().has_dirty_descendants());
    assert!(!doc.widget(container).unwrap().is_style_dirty());
    assert!(doc.widget(page).unwrap().has_dirty_descendants());
    // Nothing else is blanket-dirtied at mutation time: precision comes from
    // the pre-mutation snapshot matched against the stylist's invalidation
    // maps during the flush.
    assert!(!doc.widget(b1).unwrap().is_style_dirty());
    assert!(!doc.widget(c).unwrap().is_style_dirty());
    assert!(!doc.widget(c1).unwrap().is_style_dirty());
    assert!(!doc.widget(a).unwrap().is_style_dirty());
    assert!(!doc.widget(a).unwrap().has_dirty_descendants());

    assert!(doc.has_dirty());
}

#[test]
fn cycle_prevention() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(container, page).unwrap();
    let leaf = doc.create_view();
    doc.append_element(leaf, container).unwrap();

    // Appending an ancestor into its descendant errors.
    assert_eq!(
        doc.append_element(page, leaf),
        Err(WidgetError::WouldCycle {
            ancestor: page,
            descendant: leaf,
        })
    );
    assert_eq!(
        doc.append_element(container, leaf),
        Err(WidgetError::WouldCycle {
            ancestor: container,
            descendant: leaf,
        })
    );
    // Making an element its own parent errors.
    assert_eq!(
        doc.append_element(container, container),
        Err(WidgetError::WouldCycle {
            ancestor: container,
            descendant: container,
        })
    );
    // The tree is unchanged.
    assert_eq!(doc.get_parent(container), Some(page));
    assert_eq!(doc.get_parent(leaf), Some(container));
}

#[test]
fn set_css_id_batch() {
    let (mut doc, t) = three_children();
    doc.set_css_id(&[t.a, t.b, t.c], 42).unwrap();
    assert_eq!(doc.widget(t.a).unwrap().ext.css_id, 42);
    assert_eq!(doc.widget(t.b).unwrap().ext.css_id, 42);
    assert_eq!(doc.widget(t.c).unwrap().ext.css_id, 42);
    // The page keeps its default (unset) css_id.
    assert_eq!(doc.widget(t.page).unwrap().ext.css_id, 0);

    // A stale handle anywhere in the batch fails the whole call.
    doc.destroy_element(t.a).unwrap();
    assert_eq!(
        doc.set_css_id(&[t.b, t.a], 7),
        Err(WidgetError::StaleElement(t.a))
    );
}

#[test]
fn classes_and_inline_styles() {
    let mut doc = WidgetTree::new();
    let view = doc.create_view();
    doc.set_classes(view, "  foo   bar baz ").unwrap();
    let classes: Vec<&str> = doc
        .widget(view)
        .unwrap()
        .classes
        .iter()
        .map(|c| &**c)
        .collect();
    assert_eq!(classes, vec!["foo", "bar", "baz"]);

    // add_class dedups.
    doc.add_class(view, "bar").unwrap();
    doc.add_class(view, "qux").unwrap();
    assert_eq!(doc.widget(view).unwrap().classes.len(), 4);

    // Inline styles are parsed into a stylo `PropertyDeclarationBlock`; lock
    // ownership stays encapsulated in `stylo-dom`.
    doc.add_inline_style(view, "color", "red").unwrap();
    doc.add_inline_style(view, "width", "10px").unwrap();
    assert_eq!(inline_declaration_count(&doc, view), 2);

    // `set_inline_styles` replaces the whole block.
    doc.set_inline_styles(view, "display:flex").unwrap();
    assert_eq!(inline_declaration_count(&doc, view), 1);

    // An empty string clears the inline block.
    doc.set_inline_styles(view, "").unwrap();
    assert!(doc.widget(view).unwrap().inline_block.is_none());
}

/// The number of declarations in an element's parsed inline style block.
fn inline_declaration_count(doc: &WidgetTree, id: lynx_widget::WidgetId) -> usize {
    doc.arena().inline_style_declaration_count(id).unwrap()
}

#[test]
fn attributes_id_and_dataset_and_events() {
    let mut doc = WidgetTree::new();
    let view = doc.create_view();

    // Plain attribute (including a literal "id" attr) goes to attrs, not id_attr.
    doc.set_attribute(view, "id", "not-a-selector").unwrap();
    doc.set_attribute(view, "aria-label", "hi").unwrap();
    assert_eq!(
        doc.get_attributes(view)
            .unwrap()
            .get("id")
            .map(String::as_str),
        Some("not-a-selector")
    );
    assert!(doc.widget(view).unwrap().id_attr.is_none());

    // set_id populates the id selector separately.
    doc.set_id(view, "my-id").unwrap();
    assert_eq!(doc.widget(view).unwrap().id_attr.as_deref(), Some("my-id"));
    doc.set_id(view, "").unwrap();
    assert!(doc.widget(view).unwrap().id_attr.is_none());

    // Dataset.
    doc.set_dataset(view, [("role", "hero"), ("index", "3")])
        .unwrap();
    doc.add_dataset(view, "extra", "yes").unwrap();
    let dataset = &doc.widget(view).unwrap().ext.dataset;
    assert_eq!(dataset.get("role").map(String::as_str), Some("hero"));
    assert_eq!(dataset.get("extra").map(String::as_str), Some("yes"));
    assert_eq!(dataset.len(), 3);

    // Events.
    doc.add_event(view, EventKind::Bind, "tap", "handler#1")
        .unwrap();
    doc.add_event(view, EventKind::CaptureCatch, "touchstart", "handler#2")
        .unwrap();
    let events = &doc.widget(view).unwrap().ext.events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::Bind);
    assert_eq!(&*events[0].name, "tap");
    assert_eq!(&*events[0].handler, "handler#1");
    assert_eq!(events[1].kind, EventKind::CaptureCatch);
}

#[test]
fn pseudo_state_toggling() {
    let mut doc = WidgetTree::new();
    let view = doc.create_view();
    doc.set_pseudo_state(view, PseudoState::HOVER, true)
        .unwrap();
    doc.set_pseudo_state(view, PseudoState::FOCUS, true)
        .unwrap();
    let state = doc.pseudo_state(view).unwrap();
    assert!(state.contains(PseudoState::HOVER));
    assert!(state.contains(PseudoState::FOCUS));
    assert!(!state.contains(PseudoState::ACTIVE));

    doc.set_pseudo_state(view, PseudoState::HOVER, false)
        .unwrap();
    let state = doc.pseudo_state(view).unwrap();
    assert!(!state.contains(PseudoState::HOVER));
    assert!(state.contains(PseudoState::FOCUS));
}

#[test]
fn stale_handle_operations_error() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.destroy_element(view).unwrap();

    assert_eq!(
        doc.set_classes(view, "x"),
        Err(WidgetError::StaleElement(view))
    );
    assert_eq!(
        doc.set_attribute(view, "k", "v"),
        Err(WidgetError::StaleElement(view))
    );
    assert!(doc.get_tag(view).is_none());
}

#[test]
fn unique_ids_are_monotonic_and_one_based() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let view = doc.create_view();
    assert_eq!(doc.get_element_unique_id(page), Some(1));
    assert_eq!(doc.get_element_unique_id(view), Some(2));
    assert_eq!(doc.widget(view).unwrap().ext.unique_id, 2);
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
