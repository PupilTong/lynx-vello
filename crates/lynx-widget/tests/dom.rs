//! Element-PAPI behavior: creation, structure opcodes, attribute/style
//! setters, the `unique_id` index, and the ownership model — handles retain
//! their nodes and drive reclamation of detached subtrees.

use std::collections::HashSet;

use lynx_widget::{
    ElementState, EventBindingKind, ViewMetrics, WidgetError, WidgetHandle, WidgetKind, WidgetTree,
};

fn tree() -> WidgetTree {
    WidgetTree::new(ViewMetrics::new(800.0, 600.0, 1.0))
}

/// `page > container > [a, b, c]`.
struct ThreeChildren {
    page: WidgetHandle,
    container: WidgetHandle,
    a: WidgetHandle,
    b: WidgetHandle,
    c: WidgetHandle,
}

fn three_children() -> (WidgetTree, ThreeChildren) {
    let mut doc = tree();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_child(&page, &container).unwrap();
    let a = doc.create_view();
    let b = doc.create_view();
    let c = doc.create_view();
    doc.append_child(&container, &a).unwrap();
    doc.append_child(&container, &b).unwrap();
    doc.append_child(&container, &c).unwrap();
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

fn child_uids(doc: &WidgetTree, parent: &WidgetHandle) -> Vec<i32> {
    doc.widget(parent)
        .unwrap()
        .children()
        .map(|node| node.payload().unique_id)
        .collect()
}

fn uid(doc: &WidgetTree, handle: &WidgetHandle) -> i32 {
    doc.unique_id(handle).unwrap()
}

#[test]
fn create_elements_kinds_and_structure() {
    let mut doc = tree();
    let page = doc.create_page();
    assert_eq!(doc.widget(&page).unwrap().payload().kind, WidgetKind::Page);
    assert_eq!(doc.tag_name(&page).unwrap(), "page");
    assert_eq!(doc.page_root(), Some(page.clone()));

    let view = doc.create_view();
    let text = doc.create_text();
    doc.append_child(&page, &view).unwrap();
    doc.append_child(&view, &text).unwrap();
    assert_eq!(doc.widget(&text).unwrap().payload().kind, WidgetKind::Text);

    let raw = doc.create_raw_text("hello");
    doc.append_child(&text, &raw).unwrap();
    let raw_node = doc.widget(&raw).unwrap();
    assert_eq!(raw_node.payload().kind, WidgetKind::RawText);
    assert_eq!(raw_node.text(), Some("hello"));

    let li = doc.create_element("list-item");
    doc.append_child(&view, &li).unwrap();
    assert_eq!(
        doc.widget(&li).unwrap().payload().kind,
        WidgetKind::ListItem
    );
    assert_eq!(doc.tag_name(&li).unwrap(), "list-item");

    let custom = doc.create_element("marquee");
    doc.append_child(&view, &custom).unwrap();
    assert_eq!(
        doc.widget(&custom).unwrap().payload().kind,
        WidgetKind::Unknown
    );
    assert_eq!(doc.tag_name(&custom).unwrap(), "marquee", "real tag kept");

    assert_eq!(doc.parent(&view).unwrap(), Some(page.clone()));
    assert_eq!(doc.parent(&page).unwrap(), None);
    assert_eq!(doc.first_child(&view).unwrap(), Some(text.clone()));
    assert_eq!(doc.next_sibling(&text).unwrap(), Some(li.clone()));
}

#[test]
fn page_tag_alone_does_not_become_the_widget_root() {
    let mut tree = tree();
    let detached_page = tree.create_element("page");
    assert_eq!(tree.page_root(), None);
    assert!(
        !tree
            .document()
            .is_connected(tree.widget(&detached_page).unwrap().id())
    );

    let page = tree.create_page();
    assert_eq!(tree.page_root(), Some(page.clone()));
    assert!(
        tree.document()
            .is_connected(tree.widget(&page).unwrap().id())
    );
}

#[test]
fn unique_ids_are_monotonic_and_one_based() {
    let mut doc = tree();
    let page = doc.create_page();
    let view = doc.create_view();
    assert_eq!(uid(&doc, &page), 1);
    assert_eq!(uid(&doc, &view), 2);
    assert_eq!(doc.widget_by_unique_id(2), Some(view.clone()));
    assert_eq!(doc.widget_by_unique_id(99), None);
}

#[test]
fn widget_kind_tag_mapping() {
    assert_eq!(WidgetKind::from_tag_name("list-item"), WidgetKind::ListItem);
    assert_eq!(WidgetKind::from_tag_name("none"), WidgetKind::NoneElement);
    assert_eq!(WidgetKind::from_tag_name("marquee"), WidgetKind::Unknown);
    assert_eq!(WidgetKind::Page.tag_name(), "page");
    assert_eq!(WidgetKind::ScrollView.tag_name(), "scroll-view");
    assert_eq!(WidgetKind::Unknown.tag_name(), "unknown");
}

#[test]
fn handles_are_canonical() {
    let (doc, t) = three_children();
    let again = doc.page_root().unwrap();
    assert_eq!(again, t.page);
    let by_uid = doc.widget_by_unique_id(uid(&doc, &t.a)).unwrap();
    assert_eq!(by_uid, t.a);
    assert_ne!(t.a, t.b);
}

#[test]
fn misrouted_handles_are_rejected_at_the_native_boundary() {
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
        doc_b.append_child(&t_b.container, &t_a.a),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert!(matches!(
        doc_b.widget(&t_a.a),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert!(matches!(
        doc_b.computed_style(&t_a.a),
        Err(WidgetError::ForeignWidget(_))
    ));
    doc_a.set_classes(&t_a.a, "only-in-a").unwrap();
    let b_classes: Vec<&str> = doc_b.widget(&t_b.a).unwrap().classes().collect();
    assert!(b_classes.is_empty(), "misrouted mutation reached context B");
}

#[test]
fn insert_before_orders_and_reorders() {
    let (mut doc, t) = three_children();
    let d = doc.create_view();
    doc.insert_before(&t.container, &d, Some(&t.b)).unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![
            uid(&doc, &t.a),
            uid(&doc, &d),
            uid(&doc, &t.b),
            uid(&doc, &t.c)
        ]
    );

    doc.insert_before(&t.container, &t.c, Some(&t.a)).unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![
            uid(&doc, &t.c),
            uid(&doc, &t.a),
            uid(&doc, &d),
            uid(&doc, &t.b)
        ]
    );

    doc.insert_before(&t.container, &t.a, Some(&t.a)).unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![
            uid(&doc, &t.c),
            uid(&doc, &t.a),
            uid(&doc, &d),
            uid(&doc, &t.b)
        ]
    );

    let stranger = doc.create_view();
    assert!(matches!(
        doc.insert_before(&t.container, &d, Some(&stranger)),
        Err(WidgetError::InvalidSiblingReference(_))
    ));
}

#[test]
fn cycles_are_rejected() {
    let (mut doc, t) = three_children();
    assert!(matches!(
        doc.append_child(&t.container, &t.container),
        Err(WidgetError::WouldCycle { .. })
    ));
    assert!(matches!(
        doc.append_child(&t.a, &t.container),
        Err(WidgetError::WouldCycle { .. })
    ));
    assert_eq!(doc.parent(&t.container).unwrap(), Some(t.page.clone()));
    assert_eq!(doc.parent(&t.a).unwrap(), Some(t.container.clone()));
}

#[test]
fn the_page_root_cannot_be_reparented() {
    let (mut doc, t) = three_children();
    assert!(matches!(
        doc.append_child(&t.container, &t.page),
        Err(WidgetError::CannotReparentRoot(_))
    ));
    assert!(matches!(
        doc.insert_before(&t.container, &t.page, Some(&t.a)),
        Err(WidgetError::CannotReparentRoot(_))
    ));
    assert_eq!(doc.parent(&t.page).unwrap(), None);
    assert_eq!(doc.page_root(), Some(t.page.clone()));
}

#[test]
fn remove_detaches_and_keeps_the_subtree_alive() {
    let (mut doc, t) = three_children();
    let grandchild = doc.create_view();
    doc.append_child(&t.b, &grandchild).unwrap();

    doc.remove_child(&t.container, &t.b).unwrap();
    assert_eq!(doc.parent(&t.b).unwrap(), None);
    assert_eq!(doc.parent(&grandchild).unwrap(), Some(t.b.clone()));
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![uid(&doc, &t.a), uid(&doc, &t.c)]
    );
    doc.set_classes(&t.b, "pending").unwrap();

    doc.append_child(&t.container, &t.b).unwrap();
    assert_eq!(doc.parent(&t.b).unwrap(), Some(t.container.clone()));

    doc.remove_child(&t.container, &t.b).unwrap();
    assert!(matches!(
        doc.remove_child(&t.container, &t.b),
        Err(WidgetError::NotAChild { .. })
    ));
}

#[test]
fn replace_with_keeps_position_and_old_survives() {
    let (mut doc, t) = three_children();
    let new = doc.create_view();
    doc.replace_with(&t.b, &new).unwrap();
    assert_eq!(
        child_uids(&doc, &t.container),
        vec![uid(&doc, &t.a), uid(&doc, &new), uid(&doc, &t.c)]
    );
    assert_eq!(doc.parent(&t.b).unwrap(), None);
    assert!(doc.widget(&t.b).is_ok());

    let another = doc.create_view();
    doc.replace_with(&t.b, &another).unwrap();
    assert_eq!(doc.parent(&another).unwrap(), None);
}

#[test]
fn detached_subtree_is_reclaimed_only_when_no_handle_survives() {
    let (mut doc, t) = three_children();
    let grandchild = doc.create_view();
    doc.append_child(&t.b, &grandchild).unwrap();
    let b_uid = uid(&doc, &t.b);
    let grandchild_uid = uid(&doc, &grandchild);

    doc.remove_child(&t.container, &t.b).unwrap();

    drop(t.b);
    doc.reclaim_detached_widgets();
    assert_eq!(
        doc.parent(&grandchild)
            .unwrap()
            .map(|parent| uid(&doc, &parent)),
        Some(b_uid),
        "a held descendant handle keeps the whole detached subtree alive"
    );
    assert!(doc.widget_by_unique_id(b_uid).is_some());

    let b_again = doc.widget_by_unique_id(b_uid).unwrap();
    doc.append_child(&t.container, &b_again).unwrap();
    doc.remove_child(&t.container, &b_again).unwrap();

    drop(b_again);
    drop(grandchild);
    doc.reclaim_detached_widgets();
    assert_eq!(doc.widget_by_unique_id(b_uid), None);
    assert_eq!(doc.widget_by_unique_id(grandchild_uid), None);
}

#[test]
fn attached_nodes_are_never_collected() {
    let (mut doc, t) = three_children();
    let a_uid = uid(&doc, &t.a);
    drop(t.a);
    doc.reclaim_detached_widgets();
    let a_again = doc
        .widget_by_unique_id(a_uid)
        .expect("attached content survives with no external handles");
    assert_eq!(doc.parent(&a_again).unwrap(), Some(t.container.clone()));
}

#[test]
fn never_attached_nodes_reclaim_on_drop() {
    let mut doc = tree();
    let _page = doc.create_page();
    let orphan = doc.create_view();
    let orphan_uid = uid(&doc, &orphan);
    drop(orphan);
    doc.reclaim_detached_widgets();
    assert_eq!(
        doc.widget_by_unique_id(orphan_uid),
        None,
        "a created-but-never-attached node frees once its handle drops"
    );
}

#[test]
fn reclaimed_unique_ids_do_not_resurrect() {
    let (mut doc, t) = three_children();
    let c_uid = uid(&doc, &t.c);
    doc.remove_child(&t.container, &t.c).unwrap();
    drop(t.c);
    doc.reclaim_detached_widgets();
    let fresh = doc.create_view();
    assert_eq!(doc.widget_by_unique_id(c_uid), None);
    assert_ne!(uid(&doc, &fresh), c_uid, "unique_ids are never reused");
}

#[test]
fn classes_and_inline_styles() {
    let mut doc = tree();
    let view = doc.create_view();
    doc.set_classes(&view, "  foo   bar baz ").unwrap();
    let classes: Vec<&str> = doc.widget(&view).unwrap().classes().collect();
    assert_eq!(classes, vec!["foo", "bar", "baz"]);

    doc.add_class(&view, "bar").unwrap();
    doc.add_class(&view, "qux").unwrap();
    assert_eq!(doc.widget(&view).unwrap().classes().len(), 4);
    assert_eq!(
        doc.widget(&view).unwrap().attribute("class"),
        Some("foo bar baz qux")
    );

    doc.add_inline_style(&view, "color", "red").unwrap();
    doc.add_inline_style(&view, "width", "10px").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 2);
    assert_eq!(
        doc.widget(&view).unwrap().attribute("style"),
        Some("color: red; width: 10px;")
    );

    doc.set_inline_styles(&view, "display:flex").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 1);

    doc.set_inline_styles(&view, "").unwrap();
    assert_eq!(inline_declaration_count(&doc, &view), 0);
    assert_eq!(doc.widget(&view).unwrap().attribute("style"), Some(""));
}

fn inline_declaration_count(doc: &WidgetTree, handle: &WidgetHandle) -> usize {
    let node = doc.widget(handle).unwrap();
    doc.document().inline_style_declaration_count(node.id())
}

#[test]
fn attributes_id_dataset_and_events() {
    let mut doc = tree();
    let view = doc.create_view();

    doc.set_attribute(&view, "id", "not-a-selector").unwrap();
    doc.set_attribute(&view, "aria-label", "hi").unwrap();
    let attributes: Vec<_> = doc.attributes(&view).unwrap().collect();
    assert!(attributes.contains(&("l-css-id", "0")));
    assert!(attributes.contains(&("id", "not-a-selector")));
    assert_eq!(
        doc.widget(&view).unwrap().attribute("l-css-id"),
        Some("0"),
        "the default css scope is a real attribute"
    );
    assert_eq!(
        doc.widget(&view).unwrap().attribute("id"),
        Some("not-a-selector")
    );
    assert_eq!(
        doc.widget(&view).unwrap().id_attribute(),
        Some("not-a-selector")
    );

    doc.set_id_attribute(&view, "my-id").unwrap();
    assert_eq!(doc.widget(&view).unwrap().id_attribute(), Some("my-id"));
    assert_eq!(doc.widget(&view).unwrap().attribute("id"), Some("my-id"));
    doc.set_id_attribute(&view, "").unwrap();
    assert!(doc.widget(&view).unwrap().id_attribute().is_none());
    assert_eq!(doc.widget(&view).unwrap().attribute("id"), None);

    doc.set_dataset_entry(&view, "role", "hero").unwrap();
    doc.set_dataset_entry(&view, "index", "3").unwrap();
    doc.set_dataset_entry(&view, "extra", "yes").unwrap();
    let node = doc.widget(&view).unwrap();
    assert_eq!(node.attribute("data-role"), Some("hero"));
    assert_eq!(node.attribute("data-index"), Some("3"));
    assert_eq!(node.attribute("data-extra"), Some("yes"));

    doc.set_dataset_entry(&view, "role", "villain").unwrap();
    let node = doc.widget(&view).unwrap();
    assert_eq!(node.attribute("data-role"), Some("villain"));
    assert_eq!(node.attribute("data-index"), Some("3"));
    assert_eq!(node.attribute("data-extra"), Some("yes"));

    doc.set_dataset(&view, [("role", "updated")]).unwrap();
    let node = doc.widget(&view).unwrap();
    assert_eq!(node.attribute("data-role"), Some("updated"));
    assert_eq!(node.attribute("data-index"), None);
    assert_eq!(node.attribute("data-extra"), None);

    doc.add_event_binding(&view, EventBindingKind::Bind, "tap", "onTap")
        .unwrap();
    doc.add_event_binding(
        &view,
        EventBindingKind::CaptureCatch,
        "touchstart",
        "onTouch",
    )
    .unwrap();
    let node = doc.widget(&view).unwrap();
    let events = node.payload().events();
    assert_eq!(events.len(), 2);
    assert_eq!(&*events[0].name, "tap");
    assert_eq!(events[1].kind, EventBindingKind::CaptureCatch);
}

#[test]
fn set_css_id_batch() {
    let (mut doc, t) = three_children();
    doc.set_css_id(&[&t.a, &t.b, &t.c], 42).unwrap();
    assert_eq!(doc.widget(&t.a).unwrap().attribute("l-css-id"), Some("42"));
    assert_eq!(doc.widget(&t.b).unwrap().attribute("l-css-id"), Some("42"));
    assert_eq!(doc.widget(&t.c).unwrap().attribute("l-css-id"), Some("42"));
    assert_eq!(
        doc.widget(&t.page).unwrap().attribute("l-css-id"),
        Some("0")
    );

    let (_other, other_t) = three_children();
    assert!(matches!(
        doc.set_css_id(&[&t.b, &other_t.a], 7),
        Err(WidgetError::ForeignWidget(_))
    ));
    assert_eq!(
        doc.widget(&t.b).unwrap().attribute("l-css-id"),
        Some("42"),
        "batch atomic"
    );
}

#[test]
fn pseudo_state_methods_toggle_bits() {
    let mut doc = tree();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_child(&page, &view).unwrap();

    doc.enable_pseudo_state(&view, ElementState::HOVER).unwrap();
    doc.enable_pseudo_state(&view, ElementState::FOCUS).unwrap();
    let state = doc.pseudo_state(&view).unwrap();
    assert!(state.contains(ElementState::HOVER));
    assert!(state.contains(ElementState::FOCUS));
    assert!(!state.contains(ElementState::ACTIVE));

    doc.disable_pseudo_state(&view, ElementState::HOVER)
        .unwrap();
    let state = doc.pseudo_state(&view).unwrap();
    assert!(!state.contains(ElementState::HOVER));
    assert!(state.contains(ElementState::FOCUS));
}
