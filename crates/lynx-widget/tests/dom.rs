//! Element-PAPI behavior: creation, structure opcodes, attribute/style
//! setters, the `unique_id` index, and the ownership model — handles retain
//! their nodes and drive reclamation of detached subtrees.

use std::collections::HashSet;

use lynx_widget::{ElementState, EventKind, WidgetError, WidgetHandle, WidgetKind, WidgetTree};

/// `page > container > [a, b, c]`.
struct ThreeChildren {
    page: WidgetHandle,
    container: WidgetHandle,
    a: WidgetHandle,
    b: WidgetHandle,
    c: WidgetHandle,
}

fn three_children() -> (WidgetTree, ThreeChildren) {
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
        ThreeChildren {
            page,
            container,
            a,
            b,
            c,
        },
    )
}

/// The children of `parent`, as their Lynx `unique_id`s (document order).
fn child_uids(doc: &WidgetTree, parent: &WidgetHandle) -> Vec<i32> {
    doc.widget(parent)
        .unwrap()
        .children()
        .map(|node| node.ext().unique_id)
        .collect()
}

fn uid(doc: &WidgetTree, handle: &WidgetHandle) -> i32 {
    doc.get_element_unique_id(handle).unwrap()
}

// --- creation ---------------------------------------------------------------

#[test]
fn create_elements_kinds_and_structure() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    assert_eq!(doc.widget(&page).unwrap().ext().kind, WidgetKind::Page);
    assert_eq!(doc.get_tag(&page).unwrap(), "page");
    assert_eq!(doc.get_page_element(), Some(page.clone()));

    let view = doc.create_view();
    let text = doc.create_text();
    doc.append_element(&view, &page).unwrap();
    doc.append_element(&text, &view).unwrap();
    assert_eq!(doc.widget(&text).unwrap().ext().kind, WidgetKind::Text);

    let raw = doc.create_raw_text("hello");
    doc.append_element(&raw, &text).unwrap();
    let raw_node = doc.widget(&raw).unwrap();
    assert_eq!(raw_node.ext().kind, WidgetKind::RawText);
    assert_eq!(raw_node.text(), Some("hello"));

    let li = doc.create_element("list-item");
    doc.append_element(&li, &view).unwrap();
    assert_eq!(doc.widget(&li).unwrap().ext().kind, WidgetKind::ListItem);
    assert_eq!(doc.get_tag(&li).unwrap(), "list-item");

    let custom = doc.create_element("marquee");
    doc.append_element(&custom, &view).unwrap();
    assert_eq!(doc.widget(&custom).unwrap().ext().kind, WidgetKind::Unknown);
    assert_eq!(doc.get_tag(&custom).unwrap(), "marquee", "real tag kept");

    // Parent / child navigation.
    assert_eq!(doc.get_parent(&view).unwrap(), Some(page.clone()));
    assert_eq!(doc.get_parent(&page).unwrap(), None);
    assert_eq!(doc.first_element(&view).unwrap(), Some(text.clone()));
    assert_eq!(doc.next_element(&text).unwrap(), Some(li.clone()));
}

#[test]
fn page_tag_alone_does_not_become_the_widget_root() {
    let mut tree = WidgetTree::new();
    let detached_page = tree.create_element("page");
    assert_eq!(tree.get_page_element(), None);
    assert!(
        !tree
            .document()
            .is_connected(tree.widget(&detached_page).unwrap().id())
    );

    let page = tree.create_page();
    assert_eq!(tree.get_page_element(), Some(page.clone()));
    assert!(
        tree.document()
            .is_connected(tree.widget(&page).unwrap().id())
    );
}

#[test]
fn unique_ids_are_monotonic_and_one_based() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let view = doc.create_view();
    assert_eq!(uid(&doc, &page), 1);
    assert_eq!(uid(&doc, &view), 2);
    assert_eq!(doc.element_by_unique_id(2), Some(view.clone()));
    assert_eq!(doc.element_by_unique_id(99), None);
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

// --- handles: canonicality and context ownership -----------------------------

#[test]
fn handles_are_canonical() {
    let (doc, t) = three_children();
    // Every lookup for the same node yields the *same* canonical handle.
    let again = doc.get_page_element().unwrap();
    assert_eq!(again, t.page);
    let by_uid = doc.element_by_unique_id(uid(&doc, &t.a)).unwrap();
    assert_eq!(by_uid, t.a);
    assert_ne!(t.a, t.b);
}

#[test]
fn misrouted_handles_are_rejected_at_the_native_boundary() {
    // Runtime JS contexts never exchange handles. If native code violates
    // that boundary, the handle's existing Reaper owner rejects the routing;
    // NodeId itself remains scoped to one Document.
    let (mut doc_a, t_a) = three_children();
    let (mut doc_b, t_b) = three_children();

    assert_ne!(t_a.a, t_b.a);
    let mut identities = HashSet::new();
    assert!(identities.insert(t_a.a.clone()));
    assert!(!identities.insert(t_a.a.clone()));
    assert!(identities.insert(t_b.a.clone()));
    assert_eq!(identities.len(), 2);

    assert!(matches!(
        doc_b.set_classes(&t_a.a, "x"),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert!(matches!(
        doc_b.append_element(&t_a.a, &t_b.container),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert!(matches!(
        doc_b.widget(&t_a.a),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert!(matches!(
        doc_b.computed(&t_a.a),
        Err(WidgetError::ForeignWidget(_))
    ));
    // The corresponding node in context B is untouched.
    doc_a.set_classes(&t_a.a, "only-in-a").unwrap();
    let b_classes: Vec<&str> = doc_b.widget(&t_b.a).unwrap().classes().collect();
    assert!(b_classes.is_empty(), "misrouted mutation reached context B");
}

// --- structure opcodes --------------------------------------------------------

#[test]
fn insert_before_orders_and_reorders() {
    let (mut doc, t) = three_children();
    let d = doc.create_view();
    doc.insert_element_before(&d, &t.container, Some(&t.b))
        .unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![
            uid(&doc, &t.a),
            uid(&doc, &d),
            uid(&doc, &t.b),
            uid(&doc, &t.c)
        ]
    );

    // Re-inserting an attached child moves it.
    doc.insert_element_before(&t.c, &t.container, Some(&t.a))
        .unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![
            uid(&doc, &t.c),
            uid(&doc, &t.a),
            uid(&doc, &d),
            uid(&doc, &t.b)
        ]
    );

    // `insertBefore(n, n)` keeps `n` exactly where it is (DOM pre-insert
    // resolves the reference to n's next sibling).
    doc.insert_element_before(&t.a, &t.container, Some(&t.a))
        .unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![
            uid(&doc, &t.c),
            uid(&doc, &t.a),
            uid(&doc, &d),
            uid(&doc, &t.b)
        ]
    );

    // A reference that is not a child of the parent errors.
    let stranger = doc.create_view();
    assert!(matches!(
        doc.insert_element_before(&d, &t.container, Some(&stranger)),
        Err(WidgetError::BadInsertReference(_))
    ));
}

#[test]
fn cycles_are_rejected() {
    let (mut doc, t) = three_children();
    assert!(matches!(
        doc.append_element(&t.container, &t.container),
        Err(WidgetError::WouldCycle { .. })
    ));
    assert!(matches!(
        doc.append_element(&t.container, &t.a),
        Err(WidgetError::WouldCycle { .. })
    ));
    // The tree is unchanged.
    assert_eq!(doc.get_parent(&t.container).unwrap(), Some(t.page.clone()));
    assert_eq!(doc.get_parent(&t.a).unwrap(), Some(t.container.clone()));
}

#[test]
fn the_page_root_cannot_be_reparented() {
    let (mut doc, t) = three_children();
    assert!(matches!(
        doc.append_element(&t.page, &t.container),
        Err(WidgetError::CannotReparentRoot(_))
    ));
    assert!(matches!(
        doc.insert_element_before(&t.page, &t.container, Some(&t.a)),
        Err(WidgetError::CannotReparentRoot(_))
    ));
    // Structure is untouched; the page is still the WidgetTree root and has
    // no parent widget (its DOM parent is the distinct Document node).
    assert_eq!(doc.get_parent(&t.page).unwrap(), None);
    assert_eq!(doc.get_page_element(), Some(t.page.clone()));
}

#[test]
fn remove_detaches_and_keeps_the_subtree_alive() {
    let (mut doc, t) = three_children();
    let grandchild = doc.create_view();
    doc.append_element(&grandchild, &t.b).unwrap();

    // PAPI remove = DOM removeChild: detached, alive, mutable, re-insertable.
    doc.remove_element(&t.container, &t.b).unwrap();
    assert_eq!(doc.get_parent(&t.b).unwrap(), None);
    assert_eq!(doc.get_parent(&grandchild).unwrap(), Some(t.b.clone()));
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![uid(&doc, &t.a), uid(&doc, &t.c)]
    );
    doc.set_classes(&t.b, "pending").unwrap();

    doc.append_element(&t.b, &t.container).unwrap();
    assert_eq!(doc.get_parent(&t.b).unwrap(), Some(t.container.clone()));

    // Removing a non-child errors.
    doc.remove_element(&t.container, &t.b).unwrap();
    assert!(matches!(
        doc.remove_element(&t.container, &t.b),
        Err(WidgetError::NotAChild { .. })
    ));
}

#[test]
fn replace_element_keeps_position_and_old_survives() {
    let (mut doc, t) = three_children();
    let new = doc.create_view();
    doc.replace_element(&new, &t.b).unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![uid(&doc, &t.a), uid(&doc, &new), uid(&doc, &t.c)]
    );
    // Like DOM replaceChild, the old node survives, detached, owned by its
    // handles.
    assert_eq!(doc.get_parent(&t.b).unwrap(), None);
    assert!(doc.widget(&t.b).is_ok());

    // Replacing a detached element is a no-op (DOM replaceWith on a
    // parentless node).
    let another = doc.create_view();
    doc.replace_element(&another, &t.b).unwrap();
    assert_eq!(doc.get_parent(&another).unwrap(), None);
}

// --- ownership-driven reclamation ---------------------------------------------

#[test]
fn detached_subtree_is_reclaimed_only_when_no_handle_survives() {
    let (mut doc, t) = three_children();
    let grandchild = doc.create_view();
    doc.append_element(&grandchild, &t.b).unwrap();
    let b_uid = uid(&doc, &t.b);
    let grandchild_uid = uid(&doc, &grandchild);

    doc.remove_element(&t.container, &t.b).unwrap();

    // Drop the *parent* wrapper while the child wrapper survives — the
    // finding-3 scenario. The whole detached subtree must stay alive: the
    // held descendant retains it.
    drop(t.b);
    doc.collect();
    assert_eq!(
        doc.get_parent(&grandchild)
            .unwrap()
            .map(|parent| uid(&doc, &parent)),
        Some(b_uid),
        "a held descendant handle keeps the whole detached subtree alive"
    );
    assert!(doc.element_by_unique_id(b_uid).is_some());

    // The subtree can even be re-attached — nothing was lost.
    let b_again = doc.element_by_unique_id(b_uid).unwrap();
    doc.append_element(&b_again, &t.container).unwrap();
    doc.remove_element(&t.container, &b_again).unwrap();

    // Now drop every handle into the subtree: reclaimed atomically at the
    // next boundary.
    drop(b_again);
    drop(grandchild);
    doc.collect();
    assert_eq!(doc.element_by_unique_id(b_uid), None);
    assert_eq!(doc.element_by_unique_id(grandchild_uid), None);
}

#[test]
fn attached_nodes_are_never_collected() {
    let (mut doc, t) = three_children();
    let a_uid = uid(&doc, &t.a);
    // Drop every handle to an *attached* node: the tree itself retains
    // document content (browser semantics — GC only collects detached
    // nodes).
    drop(t.a);
    doc.collect();
    let a_again = doc
        .element_by_unique_id(a_uid)
        .expect("attached content survives with no external handles");
    assert_eq!(doc.get_parent(&a_again).unwrap(), Some(t.container.clone()));
}

#[test]
fn never_attached_nodes_reclaim_on_drop() {
    let mut doc = WidgetTree::new();
    let _page = doc.create_page();
    let orphan = doc.create_view();
    let orphan_uid = uid(&doc, &orphan);
    drop(orphan);
    doc.collect();
    assert_eq!(
        doc.element_by_unique_id(orphan_uid),
        None,
        "a created-but-never-attached node frees once its handle drops"
    );
}

#[test]
fn reclaimed_unique_ids_do_not_resurrect() {
    let (mut doc, t) = three_children();
    let c_uid = uid(&doc, &t.c);
    doc.remove_element(&t.container, &t.c).unwrap();
    drop(t.c);
    // Force slot reuse: reclaim, then create a new element.
    doc.collect();
    let fresh = doc.create_view();
    assert_eq!(doc.element_by_unique_id(c_uid), None);
    assert_ne!(uid(&doc, &fresh), c_uid, "unique_ids are never reused");
}

// --- attributes / styles / state ------------------------------------------------

#[test]
fn classes_and_inline_styles() {
    let mut doc = WidgetTree::new();
    let view = doc.create_view();
    doc.set_classes(&view, "  foo   bar baz ").unwrap();
    let classes: Vec<&str> = doc.widget(&view).unwrap().classes().collect();
    assert_eq!(classes, vec!["foo", "bar", "baz"]);

    // add_class dedups.
    doc.add_class(&view, "bar").unwrap();
    doc.add_class(&view, "qux").unwrap();
    assert_eq!(doc.widget(&view).unwrap().classes().len(), 4);

    // Inline styles are parsed into a stylo `PropertyDeclarationBlock`; lock
    // ownership stays encapsulated in `w3c-dom`.
    doc.add_inline_style(&view, "color", "red").unwrap();
    doc.add_inline_style(&view, "width", "10px").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 2);

    // `set_inline_styles` replaces the whole block.
    doc.set_inline_styles(&view, "display:flex").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 1);

    // An empty string clears the inline block.
    doc.set_inline_styles(&view, "").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 0);
}

/// The number of declarations in an element's parsed inline style block.
fn inline_declaration_count(doc: &WidgetTree, handle: &WidgetHandle) -> usize {
    let node = doc.widget(handle).unwrap();
    doc.document().inline_style_declaration_count(node.id())
}

#[test]
fn attributes_id_dataset_and_events() {
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
    assert!(doc.widget(&view).unwrap().id_attr().is_none());

    // set_id populates the id selector separately.
    doc.set_id(&view, "my-id").unwrap();
    assert_eq!(doc.widget(&view).unwrap().id_attr(), Some("my-id"));
    doc.set_id(&view, "").unwrap();
    assert!(doc.widget(&view).unwrap().id_attr().is_none());

    // Dataset.
    doc.set_dataset(&view, [("role", "hero"), ("index", "3")])
        .unwrap();
    doc.add_dataset(&view, "extra", "yes").unwrap();
    let node = doc.widget(&view).unwrap();
    let dataset = &node.ext().dataset;
    assert_eq!(dataset.get("role").map(String::as_str), Some("hero"));
    assert_eq!(dataset.get("index").map(String::as_str), Some("3"));
    assert_eq!(dataset.get("extra").map(String::as_str), Some("yes"));

    // Events are stored verbatim.
    doc.add_event(&view, EventKind::Bind, "tap", "onTap")
        .unwrap();
    doc.add_event(&view, EventKind::CaptureCatch, "touchstart", "onTouch")
        .unwrap();
    let node = doc.widget(&view).unwrap();
    assert_eq!(node.ext().events.len(), 2);
    assert_eq!(&*node.ext().events[0].name, "tap");
    assert_eq!(node.ext().events[1].kind, EventKind::CaptureCatch);
}

#[test]
fn set_css_id_batch() {
    let (mut doc, t) = three_children();
    doc.set_css_id(&[&t.a, &t.b, &t.c], 42).unwrap();
    assert_eq!(doc.widget(&t.a).unwrap().ext().css_id, 42);
    assert_eq!(doc.widget(&t.b).unwrap().ext().css_id, 42);
    assert_eq!(doc.widget(&t.c).unwrap().ext().css_id, 42);
    // The page keeps its default (unset) css_id.
    assert_eq!(doc.widget(&t.page).unwrap().ext().css_id, 0);

    // A misrouted handle anywhere in the batch fails the whole call.
    let (_other, other_t) = three_children();
    assert!(matches!(
        doc.set_css_id(&[&t.b, &other_t.a], 7),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert_eq!(doc.widget(&t.b).unwrap().ext().css_id, 42, "batch atomic");
}

#[test]
fn set_pseudo_state_toggles_bits() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(&view, &page).unwrap();

    doc.set_pseudo_state(&view, ElementState::HOVER, true)
        .unwrap();
    doc.set_pseudo_state(&view, ElementState::FOCUS, true)
        .unwrap();
    let state = doc.pseudo_state(&view).unwrap();
    assert!(state.contains(ElementState::HOVER));
    assert!(state.contains(ElementState::FOCUS));
    assert!(!state.contains(ElementState::ACTIVE));

    doc.set_pseudo_state(&view, ElementState::HOVER, false)
        .unwrap();
    let state = doc.pseudo_state(&view).unwrap();
    assert!(!state.contains(ElementState::HOVER));
    assert!(state.contains(ElementState::FOCUS));
}

// --- dirty state ------------------------------------------------------------------

#[test]
fn structural_and_attribute_changes_schedule_flush_work() {
    let mut doc = WidgetTree::new();
    let page = doc.create_page();
    let list = doc.create_view();
    let hint = doc.create_view();
    doc.append_element(&list, &page).unwrap();
    doc.append_element(&hint, &page).unwrap();
    let child = doc.create_view();
    doc.append_element(&child, &list).unwrap();
    doc.clear_dirty();
    assert!(!doc.has_dirty());

    doc.remove_element(&list, &child).unwrap();
    assert!(doc.has_dirty(), "removal must schedule flush work");
    assert!(
        !doc.widget(&hint).unwrap().is_style_dirty(),
        "siblings are not blanket-dirtied at mutation time"
    );

    doc.clear_dirty();
    doc.append_element(&child, &list).unwrap();
    assert!(doc.has_dirty(), "insertion must schedule flush work");
    assert!(doc.widget(&list).unwrap().has_dirty_descendants());

    doc.clear_dirty();
    doc.set_classes(&child, "hot").unwrap();
    assert!(doc.has_dirty(), "class change must schedule flush work");
    assert!(doc.widget(&child).unwrap().is_style_dirty());
    assert!(!doc.widget(&hint).unwrap().is_style_dirty());
}
