//! Integration tests for the `lynx-widget` Element-PAPI surface.

use std::rc::Rc;

use lynx_widget::{EventKind, PseudoState, WidgetError, WidgetKind, WidgetTree};

/// Build `page > container > [a, b, c]` and return the handles.
fn three_children() -> (WidgetTree, TestTree) {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(&container, &page).unwrap();
    let a = doc.create_view();
    let b = doc.create_view();
    let c = doc.create_view();
    doc.append_element(&a, &container).unwrap();
    doc.append_element(&b, &container).unwrap();
    doc.append_element(&c, &container).unwrap();
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
    page: lynx_widget::WidgetHandle,
    container: lynx_widget::WidgetHandle,
    a: lynx_widget::WidgetHandle,
    b: lynx_widget::WidgetHandle,
    c: lynx_widget::WidgetHandle,
}

#[test]
fn tree_building_and_navigation() {
    let (doc, t) = three_children();

    // Kinds / tags round-trip.
    assert_eq!(doc.get_kind(&t.page), Some(WidgetKind::Page));
    assert_eq!(doc.get_tag(&t.container), Some("view"));

    // Parent / child structure is exposed only as strong node handles.
    assert_eq!(doc.get_parent(&t.page), None);
    assert_eq!(doc.children(&t.page), Some(vec![t.container.clone()]));
    assert_eq!(doc.get_parent(&t.container), Some(t.page.clone()));
    assert_eq!(
        doc.children(&t.container),
        Some(vec![t.a.clone(), t.b.clone(), t.c.clone()])
    );
    assert_eq!(doc.first_element(&t.container), Some(t.a.clone()));

    // Siblings.
    assert_eq!(doc.next_element(&t.a), Some(t.b.clone()));
    assert_eq!(doc.next_element(&t.b), Some(t.c.clone()));
    assert_eq!(doc.next_element(&t.c), None);

    // Tree-level navigation getters.
    assert_eq!(doc.first_element(&t.container), Some(t.a.clone()));
    assert_eq!(doc.next_element(&t.a), Some(t.b.clone()));
    assert_eq!(doc.next_element(&t.c), None);
    assert_eq!(doc.get_parent(&t.a), Some(t.container.clone()));
    assert_eq!(doc.get_page_element(), Some(t.page.clone()));
}

#[test]
fn public_debug_output_does_not_expose_arena_ids() {
    let mut tree = WidgetTree::new();
    let page = tree.create_page();

    let handle_debug = format!("{page:?}");
    let tree_debug = format!("{tree:?}");
    assert!(handle_debug.contains("unique_id"));
    assert!(tree_debug.contains("page_unique_id"));
    for output in [&handle_debug, &tree_debug] {
        assert!(!output.contains("ElementId"));
        assert!(!output.contains("allocation_epoch"));
        assert!(!output.contains("index"));
    }
}

#[test]
fn text_and_raw_text() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let text = doc.create_text();
    doc.append_element(&text, &page).unwrap();
    let raw = doc.create_raw_text("hello");
    doc.append_element(&raw, &text).unwrap();

    assert_eq!(doc.get_kind(&text), Some(WidgetKind::Text));
    assert_eq!(doc.get_kind(&raw), Some(WidgetKind::RawText));
    assert_eq!(doc.get_text(&raw), Some("hello"));
}

#[test]
fn create_element_classifies_tag() {
    let mut doc = WidgetTree::new();
    let li = doc.create_element("list-item");
    let unknown = doc.create_element("marquee");
    assert_eq!(doc.get_kind(&li), Some(WidgetKind::ListItem));
    assert_eq!(doc.get_kind(&unknown), Some(WidgetKind::Unknown));
    assert_eq!(doc.get_tag(&unknown), Some("marquee"));
}

#[test]
fn insert_element_before_semantics() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let a = doc.create_view();
    let c = doc.create_view();
    doc.append_element(&a, &page).unwrap();
    doc.append_element(&c, &page).unwrap();

    // Insert before an existing reference.
    let b = doc.create_view();
    doc.insert_element_before(&b, &page, Some(&c)).unwrap();
    // before = None appends.
    let d = doc.create_view();
    doc.insert_element_before(&d, &page, None).unwrap();

    assert_eq!(
        doc.children(&page),
        Some(vec![a.clone(), b.clone(), c.clone(), d.clone()])
    );

    // Re-inserting a current child reorders it (detach-then-insert).
    doc.insert_element_before(&a, &page, Some(&c)).unwrap();
    assert_eq!(
        doc.children(&page),
        Some(vec![b.clone(), a.clone(), c.clone(), d.clone()])
    );

    // A reference that is not a child errors.
    let stray = doc.create_view();
    let orphan = doc.create_view();
    let err = doc.insert_element_before(&stray, &page, Some(&orphan));
    assert_eq!(err, Err(WidgetError::BadInsertReference(orphan.clone())));

    // `insertBefore(n, n)` keeps `n` exactly where it is (DOM pre-insert
    // resolves the reference to n's next sibling) — not a move-to-end.
    doc.insert_element_before(&b, &page, Some(&b)).unwrap();
    assert_eq!(
        doc.children(&page),
        Some(vec![b.clone(), a.clone(), c.clone(), d.clone()])
    );
    // ... and errors when n is not a child of the target parent.
    let outsider = doc.create_view();
    assert_eq!(
        doc.insert_element_before(&outsider, &page, Some(&outsider)),
        Err(WidgetError::BadInsertReference(outsider.clone()))
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
    doc.append_element(&before_sib, &page).unwrap();
    doc.append_element(&list, &page).unwrap();
    doc.append_element(&hint, &page).unwrap();
    let child = doc.create_view();
    doc.append_element(&child, &list).unwrap();
    doc.clear_dirty();
    assert!(!doc.has_dirty());

    doc.remove_element(&list, &child).unwrap();
    assert!(doc.has_dirty(), "removal must schedule flush work");
    assert!(
        doc.is_style_dirty(&before_sib) == Some(false),
        "siblings are not blanket-dirtied at mutation time"
    );

    doc.clear_dirty();
    doc.append_element(&child, &list).unwrap();
    assert!(doc.has_dirty(), "insertion must schedule flush work");
    assert_eq!(doc.has_dirty_descendants(&list), Some(true));
}

#[test]
fn remove_detaches_and_gc_waits_for_every_descendant_handle() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(&container, &page).unwrap();
    let child = doc.create_view();
    doc.append_element(&child, &container).unwrap();
    let grandchild = doc.create_view();
    doc.append_element(&grandchild, &child).unwrap();
    let sibling = doc.create_view();
    doc.append_element(&sibling, &container).unwrap();

    let child_uid = doc.get_element_unique_id(&child).unwrap();
    let grandchild_uid = doc.get_element_unique_id(&grandchild).unwrap();

    // PAPI remove = detach: the subtree stays alive and re-insertable
    // (web-core's __RemoveElement is DOM removeChild; list recycling
    // re-attaches removed subtrees).
    doc.remove_element(&container, &child).unwrap();
    assert!(doc.contains(&child));
    assert_eq!(doc.get_parent(&child), None);
    assert_eq!(doc.get_parent(&grandchild), Some(child.clone()));
    assert_eq!(doc.element_by_unique_id(child_uid), Some(child.clone()));
    assert_eq!(doc.children(&container), Some(vec![sibling.clone()]));

    // Re-inserting the detached subtree works.
    doc.append_element(&child, &container).unwrap();
    assert_eq!(doc.get_parent(&child), Some(container.clone()));
    doc.remove_element(&container, &child).unwrap();

    // Dropping only the root handle must not let parent reclamation destroy a
    // descendant that is still externally held.
    let child_weak = Rc::downgrade(&child);
    let grandchild_weak = Rc::downgrade(&grandchild);
    drop(child);
    doc.collect_garbage();
    assert!(child_weak.upgrade().is_some());
    assert!(grandchild_weak.upgrade().is_some());
    assert!(doc.element_by_unique_id(child_uid).is_some());

    // Once every external handle in the detached subtree is gone, one GC pass
    // reclaims the whole subtree and clears the unique-id index.
    drop(grandchild);
    doc.collect_garbage();
    assert!(child_weak.upgrade().is_none());
    assert!(grandchild_weak.upgrade().is_none());
    assert_eq!(doc.element_by_unique_id(child_uid), None);
    assert_eq!(doc.element_by_unique_id(grandchild_uid), None);

    // Removing a non-child errors.
    let other = doc.create_view();
    assert_eq!(
        doc.remove_element(&container, &other),
        Err(WidgetError::NotAChild {
            parent: container.clone(),
            child: other.clone(),
        })
    );
}

#[test]
fn replace_element_keeps_position() {
    let (mut doc, t) = three_children();
    let new = doc.create_view();
    doc.replace_element(&new, &t.b).unwrap();

    assert_eq!(
        doc.children(&t.container),
        Some(vec![t.a.clone(), new.clone(), t.c.clone()])
    );
    // Like DOM replaceChild, the old node survives, detached.
    assert!(doc.contains(&t.b));
    assert_eq!(doc.get_parent(&t.b), None);
    assert_eq!(doc.get_parent(&new), Some(t.container.clone()));

    // Replacing a detached element is a no-op (DOM replaceWith on a
    // parentless node).
    let another = doc.create_view();
    doc.replace_element(&another, &t.b).unwrap();
    assert_eq!(doc.get_parent(&another), None);
}

#[test]
fn collected_weak_handle_cannot_alias_a_reused_slot() {
    let mut doc = WidgetTree::new();
    let a = doc.create_view();
    let old = Rc::downgrade(&a);
    drop(a);
    doc.collect_garbage();
    assert!(old.upgrade().is_none());

    // The arena is now allowed to reuse the slot, but the old weak identity
    // stays expired and can never resolve to the replacement.
    let b = doc.create_view();
    assert!(old.upgrade().is_none());
    assert!(doc.contains(&b));
}

#[test]
fn connected_tree_retains_nodes_without_external_handles() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let child = doc.create_view();
    doc.append_element(&child, &page).unwrap();
    let child_weak = Rc::downgrade(&child);

    drop(child);
    doc.collect_garbage();

    let child = child_weak
        .upgrade()
        .expect("the connected page tree must retain its nodes");
    assert_eq!(doc.get_parent(&child), Some(page));
}

#[test]
fn dirty_propagation_from_set_classes() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(&container, &page).unwrap();
    let a = doc.create_view();
    let b = doc.create_view();
    let c = doc.create_view();
    doc.append_element(&a, &container).unwrap();
    doc.append_element(&b, &container).unwrap();
    doc.append_element(&c, &container).unwrap();
    let b1 = doc.create_view();
    doc.append_element(&b1, &b).unwrap();
    let c1 = doc.create_view();
    doc.append_element(&c1, &c).unwrap();

    // Establish a clean baseline (what a restyle pass would leave behind).
    doc.clear_dirty();
    assert!(!doc.has_dirty());

    doc.set_classes(&b, "highlighted").unwrap();

    // The mutated node itself is dirty; ancestors gain reachability bits.
    assert_eq!(doc.is_style_dirty(&b), Some(true));
    assert_eq!(doc.has_dirty_descendants(&container), Some(true));
    assert_eq!(doc.is_style_dirty(&container), Some(false));
    assert_eq!(doc.has_dirty_descendants(&page), Some(true));
    // Nothing else is blanket-dirtied at mutation time: precision comes from
    // the pre-mutation snapshot matched against the stylist's invalidation
    // maps during the flush.
    assert_eq!(doc.is_style_dirty(&b1), Some(false));
    assert_eq!(doc.is_style_dirty(&c), Some(false));
    assert_eq!(doc.is_style_dirty(&c1), Some(false));
    assert_eq!(doc.is_style_dirty(&a), Some(false));
    assert_eq!(doc.has_dirty_descendants(&a), Some(false));

    assert!(doc.has_dirty());
}

#[test]
fn cycle_prevention() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(&container, &page).unwrap();
    let leaf = doc.create_view();
    doc.append_element(&leaf, &container).unwrap();

    // Appending an ancestor into its descendant errors.
    assert_eq!(
        doc.append_element(&page, &leaf),
        Err(WidgetError::WouldCycle {
            ancestor: page.clone(),
            descendant: leaf.clone(),
        })
    );
    assert_eq!(
        doc.append_element(&container, &leaf),
        Err(WidgetError::WouldCycle {
            ancestor: container.clone(),
            descendant: leaf.clone(),
        })
    );
    // Making an element its own parent errors.
    assert_eq!(
        doc.append_element(&container, &container),
        Err(WidgetError::WouldCycle {
            ancestor: container.clone(),
            descendant: container.clone(),
        })
    );
    // The tree is unchanged.
    assert_eq!(doc.get_parent(&container), Some(page.clone()));
    assert_eq!(doc.get_parent(&leaf), Some(container.clone()));
}

#[test]
fn set_css_id_batch() {
    let (mut doc, t) = three_children();
    doc.set_css_id([&t.a, &t.b, &t.c], 42).unwrap();
    assert_eq!(doc.get_state(&t.a).unwrap().css_id, 42);
    assert_eq!(doc.get_state(&t.b).unwrap().css_id, 42);
    assert_eq!(doc.get_state(&t.c).unwrap().css_id, 42);
    // The page keeps its default (unset) css_id.
    assert_eq!(doc.get_state(&t.page).unwrap().css_id, 0);
}

#[test]
fn classes_and_inline_styles() {
    let mut doc = WidgetTree::new();
    let view = doc.create_view();
    doc.set_classes(&view, "  foo   bar baz ").unwrap();
    assert_eq!(doc.get_classes(&view), Some(vec!["foo", "bar", "baz"]));

    // add_class dedups.
    doc.add_class(&view, "bar").unwrap();
    doc.add_class(&view, "qux").unwrap();
    assert_eq!(doc.get_classes(&view).unwrap().len(), 4);

    // Inline styles are parsed into a stylo `PropertyDeclarationBlock`; lock
    // ownership stays encapsulated in `stylo-dom`.
    doc.add_inline_style(&view, "color", "red").unwrap();
    doc.add_inline_style(&view, "width", "10px").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 2);

    // `set_inline_styles` replaces the whole block.
    doc.set_inline_styles(&view, "display:flex").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 1);

    // An empty string clears the inline block.
    doc.set_inline_styles(&view, "").unwrap();
    assert_eq!(doc.has_inline_styles(&view), Some(false));
}

/// The number of declarations in an element's parsed inline style block.
fn inline_declaration_count(doc: &WidgetTree, handle: &lynx_widget::WidgetHandle) -> usize {
    doc.inline_style_declaration_count(handle).unwrap()
}

#[test]
fn attributes_id_and_dataset_and_events() {
    let mut doc = WidgetTree::new();
    let view = doc.create_view();

    // Plain attribute (including a literal "id" attr) goes to attrs, not id_attr.
    doc.set_attribute(&view, "id", "not-a-selector").unwrap();
    doc.set_attribute(&view, "aria-label", "hi").unwrap();
    assert_eq!(
        doc.get_attributes(&view)
            .unwrap()
            .get("id")
            .map(String::as_str),
        Some("not-a-selector")
    );
    assert_eq!(doc.get_id_selector(&view), None);

    // set_id populates the id selector separately.
    doc.set_id(&view, "my-id").unwrap();
    assert_eq!(doc.get_id_selector(&view), Some("my-id"));
    doc.set_id(&view, "").unwrap();
    assert_eq!(doc.get_id_selector(&view), None);

    // Dataset.
    doc.set_dataset(&view, [("role", "hero"), ("index", "3")])
        .unwrap();
    doc.add_dataset(&view, "extra", "yes").unwrap();
    let dataset = &doc.get_state(&view).unwrap().dataset;
    assert_eq!(dataset.get("role").map(String::as_str), Some("hero"));
    assert_eq!(dataset.get("extra").map(String::as_str), Some("yes"));
    assert_eq!(dataset.len(), 3);

    // Events.
    doc.add_event(&view, EventKind::Bind, "tap", "handler#1")
        .unwrap();
    doc.add_event(&view, EventKind::CaptureCatch, "touchstart", "handler#2")
        .unwrap();
    let events = &doc.get_state(&view).unwrap().events;
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
    doc.set_pseudo_state(&view, PseudoState::HOVER, true)
        .unwrap();
    doc.set_pseudo_state(&view, PseudoState::FOCUS, true)
        .unwrap();
    let state = doc.pseudo_state(&view).unwrap();
    assert!(state.contains(PseudoState::HOVER));
    assert!(state.contains(PseudoState::FOCUS));
    assert!(!state.contains(PseudoState::ACTIVE));

    doc.set_pseudo_state(&view, PseudoState::HOVER, false)
        .unwrap();
    let state = doc.pseudo_state(&view).unwrap();
    assert!(!state.contains(PseudoState::HOVER));
    assert!(state.contains(PseudoState::FOCUS));
}

#[test]
fn foreign_tree_handles_error_instead_of_hitting_same_slot() {
    let mut first = WidgetTree::new();
    let first_view = first.create_view();
    let mut second = WidgetTree::new();
    let second_page = second.create_page();

    assert_eq!(
        second.append_element(&first_view, &second_page),
        Err(WidgetError::ForeignElement(first_view.clone()))
    );
    assert_eq!(
        second.set_attribute(&first_view, "k", "v"),
        Err(WidgetError::ForeignElement(first_view.clone()))
    );
    assert_eq!(second.get_tag(&first_view), None);
}

#[test]
fn unique_ids_are_monotonic_and_one_based() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let view = doc.create_view();
    assert_eq!(doc.get_element_unique_id(&page), Some(1));
    assert_eq!(doc.get_element_unique_id(&view), Some(2));
    assert_eq!(view.unique_id(), 2);
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
