//! Custom elements end to end: the definition-before-creation contract, the
//! lifecycle callbacks, `:defined`, and what happens when a callback mutates
//! the tree it is being called about.
//!
//! The behavior asserted here is the W3C one (HTML §4.13 custom elements plus
//! the DOM algorithms that raise its reactions) within this crate's narrowed
//! scope — user-agent components rather than script-defined elements, so the
//! standard's upgrade half is absent by contract. See `tree::custom`'s module
//! doc for what that removes.

mod common;

use std::sync::{Arc, Mutex};

use common::Doc;
use dom::{CustomElement, Document, NodeId, ShadowRootMode};

type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

type Action = Box<dyn Fn(&mut Document<()>, NodeId) + Send + Sync>;
type ReadAction = Box<dyn Fn(&Document<()>, NodeId) + Send + Sync>;

/// Records lifecycle reactions and optional callback mutations.
struct Probe {
    tag: &'static str,
    log: Log,
    observed: Vec<String>,
    on_constructed: Option<Action>,
    on_connected: Option<Action>,
    on_disconnected: Option<ReadAction>,
    on_attribute: Option<Action>,
}

impl Probe {
    fn new(tag: &'static str, log: &Log) -> Self {
        Self {
            tag,
            log: Arc::clone(log),
            observed: Vec::new(),
            on_constructed: None,
            on_connected: None,
            on_disconnected: None,
            on_attribute: None,
        }
    }

    fn observing(mut self, names: &[&str]) -> Self {
        self.observed = names.iter().map(|name| (*name).to_owned()).collect();
        self
    }

    fn on_constructed(mut self, action: Action) -> Self {
        self.on_constructed = Some(action);
        self
    }

    fn on_connected(mut self, action: Action) -> Self {
        self.on_connected = Some(action);
        self
    }

    fn on_disconnected(mut self, action: ReadAction) -> Self {
        self.on_disconnected = Some(action);
        self
    }

    fn on_attribute(mut self, action: Action) -> Self {
        self.on_attribute = Some(action);
        self
    }

    fn record(&self, what: &str) {
        self.log
            .lock()
            .expect("the log is never poisoned")
            .push(format!("{}:{what}", self.tag));
    }
}

impl CustomElement<()> for Probe {
    fn observed_attributes(&self) -> Vec<String> {
        self.observed.clone()
    }

    fn constructed(&self, document: &mut Document<()>, element: NodeId) {
        self.record(&format!("constructed#{element}"));
        if let Some(action) = &self.on_constructed {
            action(document, element);
        }
    }

    fn connected_callback(&self, document: &mut Document<()>, element: NodeId) {
        self.record(&format!("connected#{element}"));
        if let Some(action) = &self.on_connected {
            action(document, element);
        }
    }

    fn disconnected_callback(&self, document: &Document<()>, element: NodeId) {
        self.record(&format!("disconnected#{element}"));
        if let Some(action) = &self.on_disconnected {
            action(document, element);
        }
    }

    fn attribute_changed_callback(
        &self,
        document: &mut Document<()>,
        element: NodeId,
        name: &str,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        self.record(&format!(
            "attr#{element} {name}: {} -> {}",
            old.unwrap_or("<none>"),
            new.unwrap_or("<none>")
        ));
        if let Some(action) = &self.on_attribute {
            action(document, element);
        }
    }
}

fn define(doc: &mut Doc, probe: Probe) {
    let tag = probe.tag;
    doc.dom.define(tag, Box::new(probe));
}

#[test]
fn create_element_after_define_constructs_before_returning_the_id() {
    let log = log();
    let mut doc = Doc::new();
    define(&mut doc, Probe::new("x-item", &log));

    let created = doc.dom.create_element("x-item", ());

    assert_eq!(
        take(&log),
        vec![format!("x-item:constructed#{created}")],
        "the standard creates elements with synchronous custom elements"
    );
    assert!(
        !doc.dom.is_connected(created),
        "and does not connect one that was never inserted"
    );
}

#[test]
#[should_panic(expected = "already has a definition")]
fn redefining_a_local_name_panics() {
    let log = log();
    let mut doc = Doc::new();
    define(&mut doc, Probe::new("x-item", &log));
    define(&mut doc, Probe::new("x-item", &log));
}

#[test]
#[should_panic(expected = "local name cannot be empty")]
fn defining_an_empty_local_name_panics() {
    let mut doc = Doc::new();
    doc.dom.define("", Box::new(Probe::new("x-item", &log())));
}

#[test]
#[should_panic(expected = "already has elements")]
fn defining_a_tag_that_already_has_elements_panics() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    doc.el(root, "x-item");
    define(&mut doc, Probe::new("x-item", &log));
}

#[test]
#[should_panic(expected = "already has elements")]
fn defining_a_tag_that_a_detached_element_already_uses_panics() {
    let log = log();
    let mut doc = Doc::new();
    doc.dom.create_element("x-item", ());
    define(&mut doc, Probe::new("x-item", &log));
}

#[test]
fn defining_the_document_element_tag_constructs_and_connects_it() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;

    define(&mut doc, Probe::new("page", &log));

    assert_eq!(
        take(&log),
        vec![
            format!("page:constructed#{root}"),
            format!("page:connected#{root}"),
        ]
    );
}

#[test]
fn re_inserting_a_constructed_element_never_constructs_it_twice() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let element = doc.el(root, "x-item");
    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{element}"),
            format!("x-item:connected#{element}"),
        ]
    );

    doc.dom.append_child(root, element);

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:disconnected#{element}"),
            format!("x-item:connected#{element}"),
        ]
    );
}

#[test]
fn a_definition_installed_from_a_callback_governs_what_it_then_creates() {
    let log = log();
    let inner_log = log.clone();
    let mut doc = Doc::new();
    let root = doc.root;
    let inner_id: Arc<Mutex<Option<NodeId>>> = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&inner_id);
    define(
        &mut doc,
        Probe::new("x-outer", &log).on_constructed(Box::new(move |document, element| {
            document.define("x-inner", Box::new(Probe::new("x-inner", &inner_log)));
            let inner = document.create_element("x-inner", ());
            *recorded.lock().unwrap() = Some(inner);
            document.append_child(element, inner);
        })),
    );

    let outer = doc.el(root, "x-outer");
    let inner = inner_id.lock().unwrap().expect("the constructor built one");

    assert_eq!(
        take(&log),
        vec![
            format!("x-outer:constructed#{outer}"),
            format!("x-inner:constructed#{inner}"),
            format!("x-outer:connected#{outer}"),
            format!("x-inner:connected#{inner}"),
        ]
    );
}

#[test]
fn every_element_matches_defined_including_undefined_hyphenated_tags() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let plain = doc.el(root, "view");
    let hyphenated = doc.el(root, "x-nobody");
    define(&mut doc, Probe::new("x-item", &log));
    let defined = doc.el(root, "x-item");
    doc.flush();

    for element in [plain, hyphenated, defined] {
        assert!(doc.matches(element, ":defined"));
        assert!(!doc.matches(element, ":not(:defined)"));
    }
}

#[test]
fn attributes_set_after_creation_report_normally() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log).observing(&["value", "other"]),
    );
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.set_attr(element, "value", "7");
    doc.set_attr(element, "other", "x");
    doc.set_attr(element, "unwatched", "-");

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:attr#{element} value: <none> -> 7"),
            format!("x-item:attr#{element} other: <none> -> x"),
        ]
    );
}

#[test]
fn unobserved_attribute_changes_never_reach_the_handler() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log).observing(&["value"]));
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.set_attr(element, "unwatched", "1");
    doc.set_attr(element, "value", "2");

    assert_eq!(
        take(&log),
        vec![format!("x-item:attr#{element} value: <none> -> 2")]
    );
}

#[test]
fn class_id_and_style_fire_like_any_other_attribute() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log).observing(&["class", "id", "style"]),
    );
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.dom.set_classes(element, "one");
    doc.add_class(element, "two");
    doc.remove_class(element, "one");
    doc.set_id(element, Some("hero"));
    doc.set_inline(element, "width: 1px");

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:attr#{element} class: <none> -> one"),
            format!("x-item:attr#{element} class: one -> one two"),
            format!("x-item:attr#{element} class: one two -> two"),
            format!("x-item:attr#{element} id: <none> -> hero"),
            format!("x-item:attr#{element} style: <none> -> width: 1px"),
        ]
    );
}

#[test]
fn removing_an_attribute_reports_none_as_the_new_value() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log).observing(&["value"]));
    let element = doc.el(root, "x-item");
    doc.set_attr(element, "value", "7");
    let _ = take(&log);

    doc.remove_attr(element, "value");

    assert_eq!(
        take(&log),
        vec![format!("x-item:attr#{element} value: 7 -> <none>")]
    );
}

#[test]
fn an_attribute_written_inside_the_constructor_reports_nothing() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log)
            .observing(&["value"])
            .on_constructed(Box::new(|document, element| {
                document.set_attribute(element, "value", "from-constructor");
            })),
    );
    let element = doc.el(root, "x-item");

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{element}"),
            format!("x-item:connected#{element}"),
        ],
        "a precustomized element is not custom, so its own writes raise nothing"
    );
    assert_eq!(
        doc.dom.get(element).unwrap().attribute("value"),
        Some("from-constructor")
    );
}

#[test]
fn connected_fires_once_per_insertion_never_doubled_with_the_construction() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let element = doc.dom.create_element("x-item", ());
    assert_eq!(take(&log), vec![format!("x-item:constructed#{element}")]);

    doc.dom.append_child(root, element);

    assert_eq!(
        take(&log),
        vec![format!("x-item:connected#{element}")],
        "already custom, so the insertion connects rather than upgrading again"
    );
}

#[test]
fn inserting_into_a_disconnected_parent_raises_nothing_until_it_connects() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let holder = doc.dom.create_element("holder", ());
    let child = doc.dom.create_element("x-item", ());
    let _ = take(&log);

    doc.dom.append_child(holder, child);
    assert!(
        take(&log).is_empty(),
        "the subtree is not in the document yet"
    );

    doc.dom.append_child(root, holder);
    assert_eq!(take(&log), vec![format!("x-item:connected#{child}")]);
}

#[test]
fn disconnect_is_gated_on_the_old_parents_connectedness() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let detached_parent = doc.dom.create_element("holder", ());
    let inside = doc.dom.create_element("x-item", ());
    doc.dom.append_child(detached_parent, inside);
    let connected = doc.el(root, "x-item");
    let _ = take(&log);

    doc.dom.remove_element(inside);
    assert!(
        take(&log).is_empty(),
        "its old parent was never connected, so nothing was ever connected"
    );

    doc.dom.remove_element(connected);
    assert_eq!(take(&log), vec![format!("x-item:disconnected#{connected}")]);
}

#[test]
fn moving_a_two_element_subtree_batches_disconnect_and_connect_per_element() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let first = doc.el(root, "x-item");
    let second = doc.el(first, "x-item");
    let destination = doc.el(root, "holder");
    let _ = take(&log);

    doc.dom.append_child(destination, first);

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:disconnected#{first}"),
            format!("x-item:connected#{first}"),
            format!("x-item:disconnected#{second}"),
            format!("x-item:connected#{second}"),
        ],
        "per element FIFO, not a flat disc-disc-conn-conn"
    );
}

#[test]
fn drop_subtree_delivers_disconnect_while_the_subtree_is_still_readable() {
    let log = log();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut doc = Doc::new();
    let root = doc.root;
    let recorded = Arc::clone(&seen);
    define(
        &mut doc,
        Probe::new("x-item", &log).on_disconnected(Box::new(move |document, element| {
            let tag = document
                .get(element)
                .and_then(|node| node.tag_name().map(str::to_owned));
            recorded.lock().unwrap().push(tag);
        })),
    );
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.dom.drop_subtree(element);

    assert_eq!(take(&log), vec![format!("x-item:disconnected#{element}")]);
    assert_eq!(
        &*seen.lock().unwrap(),
        &[Some("x-item".to_owned())],
        "the disconnected element is still readable when its callback runs"
    );
    assert!(
        doc.dom.get(element).is_none(),
        "and freed immediately after"
    );
}

#[test]
fn drop_element_disconnects_the_subtree_that_survives_it() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let parent = doc.el(root, "x-item");
    let child = doc.el(parent, "x-item");
    let _ = take(&log);

    doc.dom.drop_element(parent);

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:disconnected#{parent}"),
            format!("x-item:disconnected#{child}"),
        ],
        "the child left the document with its parent, so it disconnects too — even though only \
         the parent is freed"
    );
    assert!(doc.dom.get(parent).is_none());
    assert!(
        doc.dom
            .get(child)
            .is_some_and(|node| node.parent_id().is_none()),
        "the child stays allocated, unlinked from the node that was freed"
    );
}

#[test]
fn drop_element_delivers_disconnect_while_the_children_are_still_attached() {
    let log = log();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut doc = Doc::new();
    let root = doc.root;
    let recorded = Arc::clone(&seen);
    define(
        &mut doc,
        Probe::new("x-item", &log).on_disconnected(Box::new(move |document, element| {
            let children = document
                .get(element)
                .map(|node| node.child_ids().to_vec())
                .unwrap_or_default();
            recorded.lock().unwrap().push((element, children));
        })),
    );
    let parent = doc.el(root, "x-item");
    let child = doc.el(parent, "x-item");
    let _ = take(&log);

    doc.dom.drop_element(parent);

    assert_eq!(
        &*seen.lock().unwrap(),
        &[(parent, vec![child]), (child, Vec::new())],
        "the subtree is intact while the callbacks read it; the children are unlinked afterwards"
    );
    assert_eq!(doc.dom.get(child).unwrap().parent_id(), None);
}

#[test]
fn subtree_insertion_delivers_callbacks_in_tree_order() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let parent = doc.dom.create_element("x-item", ());
    let first = doc.dom.create_element("x-item", ());
    let second = doc.dom.create_element("x-item", ());
    doc.dom.append_child(parent, first);
    doc.dom.append_child(parent, second);
    let _ = take(&log);

    doc.dom.append_child(root, parent);

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:connected#{parent}"),
            format!("x-item:connected#{first}"),
            format!("x-item:connected#{second}"),
        ]
    );
}

#[test]
fn a_constructor_that_attaches_a_shadow_root_and_a_stylesheet_renders() {
    let log = log();
    let mut doc = Doc::new();
    doc.add_css("page { display: linear; } x-card { display: linear; }");
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-card", &log).on_constructed(Box::new(|document, element| {
            let shadow = document.attach_shadow(element, ShadowRootMode::Open);
            document.add_shadow_stylesheet(
                shadow,
                "frame { display: linear; width: 24px; height: 8px; }",
            );
            let frame = document.create_element("frame", ());
            document.append_child(shadow, frame);
        })),
    );

    let card = doc.el(root, "x-card");
    doc.flush();

    let shadow = doc
        .dom
        .shadow_root(card)
        .expect("the constructor attached one");
    let frame = doc
        .dom
        .get(shadow)
        .and_then(|node| node.child_ids().first().copied())
        .expect("the constructor built its template");
    let layout = doc
        .dom
        .rounded_layout(frame)
        .expect("the frame is laid out");
    assert_eq!((layout.size.width, layout.size.height), (24.0, 8.0));
}

#[test]
fn an_element_inside_a_shadow_tree_is_constructed_and_connected() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let host = doc.el(root, "host");
    let shadow = doc.dom.attach_shadow(host, ShadowRootMode::Open);

    let inside = doc.el(shadow, "x-item");

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{inside}"),
            format!("x-item:connected#{inside}"),
        ],
        "a shadow tree of a connected host is connected"
    );
}

#[test]
fn an_unassigned_light_child_is_still_constructed_and_still_connects() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let host = doc.el(root, "host");
    doc.dom.attach_shadow(host, ShadowRootMode::Open);

    let orphan = doc.el(host, "x-item");
    doc.flush();

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{orphan}"),
            format!("x-item:connected#{orphan}"),
        ]
    );
    assert!(
        doc.dom.get(orphan).unwrap().computed_style().is_none(),
        "and is still out of the flat tree"
    );
}

#[test]
#[should_panic(expected = "owned by the custom element state machine")]
fn clearing_defined_through_the_element_state_api_panics() {
    let mut doc = Doc::new();
    let root = doc.root;
    let element = doc.el(root, "view");
    doc.dom
        .remove_element_state(element, dom::ElementState::DEFINED);
}

#[test]
#[should_panic(expected = "owned by the custom element state machine")]
fn setting_defined_through_the_element_state_api_panics() {
    let mut doc = Doc::new();
    let root = doc.root;
    let element = doc.el(root, "view");
    doc.dom
        .add_element_state(element, dom::ElementState::DEFINED);
}

#[test]
fn insert_before_a_reference_node_delivers_the_same_reactions() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let anchor = doc.el(root, "x-item");
    let _ = take(&log);

    let inserted = doc.dom.create_element("x-item", ());
    assert_eq!(
        take(&log),
        vec![format!("x-item:constructed#{inserted}")],
        "creation constructs but does not connect"
    );

    doc.dom.insert_before(root, inserted, Some(anchor));

    assert_eq!(take(&log), vec![format!("x-item:connected#{inserted}")]);
    assert_eq!(
        doc.dom.get(root).unwrap().child_ids(),
        &[inserted, anchor],
        "and it landed in front of the reference node"
    );
}

#[test]
fn an_attribute_change_on_a_detached_element_still_reports() {
    let log = log();
    let mut doc = Doc::new();
    define(&mut doc, Probe::new("x-item", &log).observing(&["value"]));
    let detached = doc.dom.create_element("x-item", ());
    let _ = take(&log);

    doc.dom.set_attribute(detached, "value", "7");

    assert_eq!(
        take(&log),
        vec![format!("x-item:attr#{detached} value: <none> -> 7")]
    );
}

#[test]
fn setting_an_attribute_to_its_current_value_still_reports() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log).observing(&["value"]));
    let element = doc.el(root, "x-item");
    doc.set_attr(element, "value", "7");
    let _ = take(&log);

    doc.set_attr(element, "value", "7");

    assert_eq!(
        take(&log),
        vec![format!("x-item:attr#{element} value: 7 -> 7")]
    );
}

#[test]
fn a_no_op_class_mutation_reports_nothing_unlike_dom_token_list() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log).observing(&["class"]));
    let element = doc.el(root, "x-item");
    doc.dom.add_class(element, "one");
    let _ = take(&log);

    doc.dom.add_class(element, "one");
    doc.dom.remove_class(element, "absent");

    assert!(take(&log).is_empty());
}

#[test]
fn a_callback_that_creates_another_element_of_its_own_tag_does_not_panic() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let depth = Arc::new(Mutex::new(0u32));
    let counter = Arc::clone(&depth);
    define(
        &mut doc,
        Probe::new("x-row", &log).on_constructed(Box::new(move |document, _| {
            let mut guard = counter.lock().unwrap();
            if *guard >= 3 {
                return;
            }
            *guard += 1;
            drop(guard);
            document.create_element("x-row", ());
        })),
    );

    let outer = doc.el(root, "x-row");

    let transcript = take(&log);
    assert_eq!(
        transcript.len(),
        5,
        "the outer element plus three nested creations, each constructed once: {transcript:?}"
    );
    assert!(transcript.contains(&format!("x-row:connected#{outer}")));
}

#[test]
fn removing_a_later_sibling_from_a_callback_drains_that_siblings_whole_queue() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let victim: Arc<Mutex<Option<NodeId>>> = Arc::new(Mutex::new(None));
    let target = Arc::clone(&victim);
    define(
        &mut doc,
        Probe::new("x-item", &log).on_connected(Box::new(move |document, _| {
            let doomed = target.lock().unwrap().take();
            if let Some(id) = doomed {
                document.drop_subtree(id);
            }
        })),
    );
    let holder = doc.dom.create_element("holder", ());
    let first = doc.dom.create_element("x-item", ());
    let second = doc.dom.create_element("x-item", ());
    doc.dom.append_child(holder, first);
    doc.dom.append_child(holder, second);
    *victim.lock().unwrap() = Some(second);
    let _ = take(&log);

    doc.dom.append_child(root, holder);

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:connected#{first}"),
            format!("x-item:connected#{second}"),
            format!("x-item:disconnected#{second}"),
        ],
        "the nested scope runs the sibling's pending connect before its disconnect"
    );
    assert!(doc.dom.get(second).is_none());
}

#[test]
fn a_freed_id_recycled_by_a_later_creation_receives_no_stale_reaction() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let element = doc.el(root, "x-item");
    doc.dom.drop_subtree(element);
    let _ = take(&log);

    let recycled = doc.dom.create_element("plain", ());
    assert_eq!(recycled, element, "the arena reuses the freed slot");
    doc.dom.append_child(root, recycled);

    assert!(
        take(&log).is_empty(),
        "the recycled id inherits nothing from its previous occupant"
    );
}

#[test]
#[should_panic(expected = "freeing it is not")]
fn a_constructor_that_frees_the_element_being_created_panics() {
    let log = log();
    let mut doc = Doc::new();
    define(
        &mut doc,
        Probe::new("x-item", &log).on_constructed(Box::new(|document, element| {
            document.drop_subtree(element);
        })),
    );
    doc.dom.create_element("x-item", ());
}

#[test]
fn a_caught_pin_panic_leaves_the_subtree_intact() {
    let log = log();
    let mut doc = Doc::new();
    let caught = Arc::new(Mutex::new(false));
    let recorded = Arc::clone(&caught);
    define(
        &mut doc,
        Probe::new("x-item", &log).on_constructed(Box::new(move |document, element| {
            let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                document.drop_subtree(element);
            }));
            *recorded.lock().unwrap() = hit.is_err();
        })),
    );

    let created = doc.dom.create_element("x-item", ());

    assert!(*caught.lock().unwrap(), "the guard fired");
    assert!(
        doc.dom.get(created).is_some(),
        "and fired before anything was freed, so the id still names its element"
    );
    assert_eq!(doc.dom.get(created).unwrap().tag_name(), Some("x-item"));
}

#[test]
#[should_panic(expected = "freeing it is not")]
fn a_constructor_that_frees_and_recycles_its_own_id_panics() {
    let log = log();
    let mut doc = Doc::new();
    define(
        &mut doc,
        Probe::new("x-item", &log).on_constructed(Box::new(|document, element| {
            document.drop_subtree(element);
            document.create_element("view", ());
        })),
    );
    doc.dom.create_element("x-item", ());
}

#[test]
fn a_depth_guard_panic_leaves_the_document_usable() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-deep", &log).on_constructed(Box::new(|document, _| {
            document.create_element("x-deep", ());
        })),
    );

    let hit_the_guard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        doc.dom.create_element("x-deep", ());
    }));
    assert!(hit_the_guard.is_err(), "the nesting guard fires");

    let survivor = doc.dom.create_element("view", ());
    doc.dom.append_child(root, survivor);
    doc.flush();
    assert!(doc.matches(survivor, ":defined"));
}

#[test]
fn clearing_an_id_that_was_never_set_reports_nothing() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log).observing(&["id"]));
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.set_id(element, None);
    assert!(
        take(&log).is_empty(),
        "removing an absent attribute changes nothing, so it reports nothing"
    );

    doc.set_id(element, Some("hero"));
    doc.set_id(element, None);
    assert_eq!(
        take(&log),
        vec![
            format!("x-item:attr#{element} id: <none> -> hero"),
            format!("x-item:attr#{element} id: hero -> <none>"),
        ],
        "a real removal still reports"
    );
}

#[test]
#[should_panic(expected = "cannot flush styles")]
fn a_callback_that_commits_layout_panics() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log).on_connected(Box::new(|document, _| {
            document.layout();
        })),
    );
    doc.el(root, "x-item");
}

#[test]
#[should_panic(expected = "nested more than")]
fn unbounded_reaction_recursion_panics_instead_of_hanging() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log).on_constructed(Box::new(|document, _| {
            document.create_element("x-item", ());
        })),
    );
    doc.el(root, "x-item");
}

#[test]
fn an_attribute_callback_that_mutates_the_tree_drains_in_its_own_scope() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log)
            .observing(&["value"])
            .on_attribute(Box::new(|document, element| {
                let child = document.create_element("x-item", ());
                document.append_child(element, child);
            })),
    );
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.set_attr(element, "value", "1");

    let transcript = take(&log);
    assert_eq!(
        transcript.first(),
        Some(&format!("x-item:attr#{element} value: <none> -> 1")),
        "the attribute reaction is first: {transcript:?}"
    );
    assert_eq!(
        transcript.len(),
        3,
        "then the nested child's construction and connection: {transcript:?}"
    );
    assert_eq!(
        doc.dom.get(element).unwrap().child_ids().len(),
        1,
        "and the mutation itself landed"
    );
}

#[test]
fn a_document_with_no_definitions_behaves_exactly_as_before() {
    let mut doc = Doc::with_css("view { width: 5px; }");
    let root = doc.root;
    let element = doc.el(root, "view");
    let child = doc.el(element, "view");
    doc.dom.remove_element(child);
    doc.dom.append_child(element, child);
    doc.flush();

    assert_eq!(doc.value(child, "width"), "5px");
    assert!(doc.matches(child, ":defined"));
}
