//! Integration tests for the `lynx-dom` Element-PAPI surface.

use lynx_dom::{Document, DomError, EventKind, NodeKind, PseudoState};

/// Build `page > container > [a, b, c]` and return the handles.
fn three_children() -> (Document, TestTree) {
    let mut doc = Document::new();
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
    page: lynx_dom::ElementId,
    container: lynx_dom::ElementId,
    a: lynx_dom::ElementId,
    b: lynx_dom::ElementId,
    c: lynx_dom::ElementId,
}

#[test]
fn tree_building_and_navigation() {
    let (doc, t) = three_children();

    // Kinds / tags round-trip.
    assert_eq!(doc.elem(t.page).unwrap().kind(), NodeKind::Page);
    assert_eq!(doc.get_tag(t.container), Some("view"));

    // Parent / child structure via ElemRef.
    let page = doc.elem(t.page).unwrap();
    assert!(page.parent().is_none());
    let children: Vec<_> = page.children().map(lynx_dom::ElemRef::id).collect();
    assert_eq!(children, vec![t.container]);

    let container = doc.elem(t.container).unwrap();
    assert_eq!(container.parent().unwrap().id(), t.page);
    let kids: Vec<_> = container.children().map(lynx_dom::ElemRef::id).collect();
    assert_eq!(kids, vec![t.a, t.b, t.c]);
    assert_eq!(container.first_child().unwrap().id(), t.a);
    assert_eq!(container.last_child().unwrap().id(), t.c);

    // Siblings.
    let a = doc.elem(t.a).unwrap();
    assert!(a.prev_sibling().is_none());
    assert_eq!(a.next_sibling().unwrap().id(), t.b);
    assert_eq!(doc.elem(t.b).unwrap().next_sibling().unwrap().id(), t.c);
    assert!(doc.elem(t.c).unwrap().next_sibling().is_none());
    assert_eq!(doc.elem(t.b).unwrap().prev_sibling().unwrap().id(), t.a);

    // Document-level navigation getters.
    assert_eq!(doc.first_element(t.container), Some(t.a));
    assert_eq!(doc.next_element(t.a), Some(t.b));
    assert_eq!(doc.next_element(t.c), None);
    assert_eq!(doc.get_parent(t.a), Some(t.container));
    assert_eq!(doc.get_page_element(), Some(t.page));
}

#[test]
fn text_and_raw_text() {
    let mut doc = Document::new();
    let page = doc.create_page();
    let text = doc.create_text();
    doc.append_element(text, page).unwrap();
    let raw = doc.create_raw_text("hello");
    doc.append_element(raw, text).unwrap();

    assert_eq!(doc.elem(text).unwrap().kind(), NodeKind::Text);
    let raw_node = doc.node(raw).unwrap();
    assert_eq!(raw_node.kind, NodeKind::RawText);
    assert_eq!(raw_node.text.as_deref(), Some("hello"));
}

#[test]
fn create_element_classifies_tag() {
    let mut doc = Document::new();
    let li = doc.create_element("list-item");
    let unknown = doc.create_element("marquee");
    assert_eq!(doc.node(li).unwrap().kind, NodeKind::ListItem);
    assert_eq!(doc.node(unknown).unwrap().kind, NodeKind::Unknown);
    assert_eq!(doc.get_tag(unknown), Some("marquee"));
}

#[test]
fn insert_element_before_semantics() {
    let mut doc = Document::new();
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
        .elem(page)
        .unwrap()
        .children()
        .map(lynx_dom::ElemRef::id)
        .collect();
    assert_eq!(order, vec![a, b, c, d]);

    // Re-inserting a current child reorders it (detach-then-insert).
    doc.insert_element_before(a, page, Some(c)).unwrap();
    let order: Vec<_> = doc
        .elem(page)
        .unwrap()
        .children()
        .map(lynx_dom::ElemRef::id)
        .collect();
    assert_eq!(order, vec![b, a, c, d]);

    // A reference that is not a child errors.
    let stray = doc.create_view();
    let orphan = doc.create_view();
    let err = doc.insert_element_before(stray, page, Some(orphan));
    assert_eq!(err, Err(DomError::BadInsertReference(orphan)));
}

#[test]
fn remove_element_drops_subtree() {
    let mut doc = Document::new();
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

    doc.remove_element(container, child).unwrap();

    // The subtree is gone from the arena.
    assert!(doc.node(child).is_none());
    assert!(doc.node(grandchild).is_none());
    assert!(doc.elem(child).is_none());
    // The unique_id index is cleaned.
    assert_eq!(doc.element_by_unique_id(child_uid), None);
    assert_eq!(doc.element_by_unique_id(grandchild_uid), None);
    // The surviving sibling remains.
    let kids: Vec<_> = doc
        .elem(container)
        .unwrap()
        .children()
        .map(lynx_dom::ElemRef::id)
        .collect();
    assert_eq!(kids, vec![sibling]);

    // Removing a non-child errors.
    let other = doc.create_view();
    assert_eq!(
        doc.remove_element(container, other),
        Err(DomError::NotAChild {
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
        .elem(t.container)
        .unwrap()
        .children()
        .map(lynx_dom::ElemRef::id)
        .collect();
    assert_eq!(order, vec![t.a, new, t.c]);
    assert!(doc.node(t.b).is_none());
    assert_eq!(doc.get_parent(new), Some(t.container));
}

#[test]
fn generation_safety_after_reuse() {
    let mut doc = Document::new();
    let page = doc.create_page();
    let a = doc.create_view();
    doc.append_element(a, page).unwrap();

    doc.remove_element(page, a).unwrap();
    // The next created element reuses the freed slot with a bumped generation.
    let b = doc.create_view();
    assert_eq!(a.index(), b.index(), "slot should have been reused");

    // The stale handle no longer resolves; the new one does.
    assert!(doc.node(a).is_none());
    assert!(doc.node(b).is_some());
    assert_ne!(a, b);
}

#[test]
fn dirty_propagation_from_set_classes() {
    let mut doc = Document::new();
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

    // The mutated node and its subtree are dirty.
    assert!(doc.node(b).unwrap().style_dirty);
    assert!(doc.node(b1).unwrap().style_dirty);
    // Ancestors gain dirty_descendants but not style_dirty.
    assert!(doc.node(container).unwrap().dirty_descendants);
    assert!(!doc.node(container).unwrap().style_dirty);
    assert!(doc.node(page).unwrap().dirty_descendants);
    // Following sibling's subtree is dirtied (covers + / ~ combinators).
    assert!(doc.node(c).unwrap().style_dirty);
    assert!(doc.node(c1).unwrap().style_dirty);
    // Earlier sibling is untouched.
    assert!(!doc.node(a).unwrap().style_dirty);
    assert!(!doc.node(a).unwrap().dirty_descendants);

    assert!(doc.has_dirty());
}

#[test]
fn cycle_prevention() {
    let mut doc = Document::new();
    let page = doc.create_page();
    let container = doc.create_view();
    doc.append_element(container, page).unwrap();
    let leaf = doc.create_view();
    doc.append_element(leaf, container).unwrap();

    // Appending an ancestor into its descendant errors.
    assert_eq!(
        doc.append_element(page, leaf),
        Err(DomError::WouldCycle {
            ancestor: page,
            descendant: leaf,
        })
    );
    assert_eq!(
        doc.append_element(container, leaf),
        Err(DomError::WouldCycle {
            ancestor: container,
            descendant: leaf,
        })
    );
    // Making an element its own parent errors.
    assert_eq!(
        doc.append_element(container, container),
        Err(DomError::WouldCycle {
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
    assert_eq!(doc.node(t.a).unwrap().css_id, 42);
    assert_eq!(doc.node(t.b).unwrap().css_id, 42);
    assert_eq!(doc.node(t.c).unwrap().css_id, 42);
    // The page keeps its default (unset) css_id.
    assert_eq!(doc.node(t.page).unwrap().css_id, 0);

    // A stale handle anywhere in the batch fails the whole call.
    doc.remove_element(t.container, t.a).unwrap();
    assert_eq!(
        doc.set_css_id(&[t.b, t.a], 7),
        Err(DomError::StaleElement(t.a))
    );
}

#[test]
fn classes_and_inline_styles() {
    let mut doc = Document::new();
    let view = doc.create_view();
    doc.set_classes(view, "  foo   bar baz ").unwrap();
    let classes: Vec<&str> = doc
        .node(view)
        .unwrap()
        .classes
        .iter()
        .map(|c| &**c)
        .collect();
    assert_eq!(classes, vec!["foo", "bar", "baz"]);

    // add_class dedups.
    doc.add_class(view, "bar").unwrap();
    doc.add_class(view, "qux").unwrap();
    assert_eq!(doc.node(view).unwrap().classes.len(), 4);

    // Inline styles are now parsed into a stylo `PropertyDeclarationBlock`
    // guarded by the document's shared lock.
    doc.add_inline_style(view, "color", "red").unwrap();
    doc.add_inline_style(view, "width", "10px").unwrap();
    assert_eq!(inline_declaration_count(&doc, view), 2);

    // `set_inline_styles` replaces the whole block.
    doc.set_inline_styles(view, "display:flex").unwrap();
    assert_eq!(inline_declaration_count(&doc, view), 1);

    // An empty string clears the inline block.
    doc.set_inline_styles(view, "").unwrap();
    assert!(doc.node(view).unwrap().inline_block.is_none());
}

/// The number of declarations in an element's parsed inline style block.
fn inline_declaration_count(doc: &Document, id: lynx_dom::ElementId) -> usize {
    let guard = doc.shared_lock().read();
    let node = doc.node(id).unwrap();
    let block = node
        .inline_block
        .as_ref()
        .expect("element has an inline block");
    block.read_with(&guard).declarations().len()
}

#[test]
fn attributes_id_and_dataset_and_events() {
    let mut doc = Document::new();
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
    assert!(doc.node(view).unwrap().id_attr.is_none());

    // set_id populates the id selector separately.
    doc.set_id(view, "my-id").unwrap();
    assert_eq!(doc.node(view).unwrap().id_attr.as_deref(), Some("my-id"));
    doc.set_id(view, "").unwrap();
    assert!(doc.node(view).unwrap().id_attr.is_none());

    // Dataset.
    doc.set_dataset(view, [("role", "hero"), ("index", "3")])
        .unwrap();
    doc.add_dataset(view, "extra", "yes").unwrap();
    let dataset = &doc.node(view).unwrap().dataset;
    assert_eq!(dataset.get("role").map(String::as_str), Some("hero"));
    assert_eq!(dataset.get("extra").map(String::as_str), Some("yes"));
    assert_eq!(dataset.len(), 3);

    // Events.
    doc.add_event(view, EventKind::Bind, "tap", "handler#1")
        .unwrap();
    doc.add_event(view, EventKind::CaptureCatch, "touchstart", "handler#2")
        .unwrap();
    let events = &doc.node(view).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::Bind);
    assert_eq!(&*events[0].name, "tap");
    assert_eq!(&*events[0].handler, "handler#1");
    assert_eq!(events[1].kind, EventKind::CaptureCatch);
}

#[test]
fn pseudo_state_toggling() {
    let mut doc = Document::new();
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
    let mut doc = Document::new();
    let page = doc.create_page();
    let view = doc.create_view();
    doc.append_element(view, page).unwrap();
    doc.remove_element(page, view).unwrap();

    assert_eq!(
        doc.set_classes(view, "x"),
        Err(DomError::StaleElement(view))
    );
    assert_eq!(
        doc.set_attribute(view, "k", "v"),
        Err(DomError::StaleElement(view))
    );
    assert!(doc.get_tag(view).is_none());
}
