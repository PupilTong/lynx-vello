//! Custom elements end to end: definition, upgrade, the lifecycle callbacks,
//! `:defined`, and what happens when a callback mutates the tree it is being
//! called about.
//!
//! The behavior asserted here is the W3C one (HTML §4.13 custom elements plus
//! the DOM algorithms that raise its reactions), not any one engine's
//! approximation of it.

mod common;

use std::sync::{Arc, Mutex};

use common::Doc;
use dom::{CustomElement, Document, NodeId, ShadowRootMode};

/// What every callback appends to, so a test asserts an exact ordered
/// transcript rather than a set of flags.
type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().expect("the log is never poisoned").clone()
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

type Action = Box<dyn Fn(&mut Document<()>, NodeId) + Send + Sync>;

/// A definition that records every reaction it receives and can run an
/// arbitrary mutation from inside any of them.
struct Probe {
    tag: &'static str,
    log: Log,
    observed: Vec<String>,
    on_constructed: Option<Action>,
    on_connected: Option<Action>,
    on_disconnected: Option<Action>,
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

    fn on_disconnected(mut self, action: Action) -> Self {
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

    fn disconnected_callback(&self, document: &mut Document<()>, element: NodeId) {
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

// --- Definition and upgrade ------------------------------------------------

#[test]
fn define_upgrades_a_pre_existing_subtree_in_shadow_including_tree_order() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let outer = doc.el(root, "x-item");
    let inner = doc.el(outer, "x-item");
    let sibling = doc.el(root, "x-item");
    assert!(entries(&log).is_empty(), "no definition, no reactions");

    define(&mut doc, Probe::new("x-item", &log));

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{outer}"),
            format!("x-item:connected#{outer}"),
            format!("x-item:constructed#{inner}"),
            format!("x-item:connected#{inner}"),
            format!("x-item:constructed#{sibling}"),
            format!("x-item:connected#{sibling}"),
        ],
        "tree order, and each element's whole queue drains before the next"
    );
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
fn create_element_before_define_leaves_it_undefined_until_define_runs() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let early = doc.el(root, "x-item");
    doc.flush();
    assert!(doc.matches(early, "x-item:not(:defined)"));

    define(&mut doc, Probe::new("x-item", &log));
    doc.flush();

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{early}"),
            format!("x-item:connected#{early}"),
        ]
    );
    assert!(doc.matches(early, "x-item:defined"));
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
fn a_second_upgrade_reaction_for_the_same_element_is_a_no_op() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let element = doc.el(root, "x-item");
    define(&mut doc, Probe::new("x-item", &log));
    assert_eq!(take(&log).len(), 2);

    // Re-inserting an already-upgraded element must connect it, not construct
    // it a second time.
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
fn define_does_not_upgrade_a_detached_subtree_but_insertion_does() {
    let log = log();
    let mut doc = Doc::new();
    let detached = doc.dom.create_element("x-item", ());
    define(&mut doc, Probe::new("x-item", &log));
    assert!(
        take(&log).is_empty(),
        "the sweep walks the document, not the arena"
    );

    let root = doc.root;
    doc.dom.append_child(root, detached);

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{detached}"),
            format!("x-item:connected#{detached}"),
        ]
    );
}

// --- Attributes ------------------------------------------------------------

#[test]
fn an_attribute_set_before_definition_is_replayed_by_the_upgrade() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let element = doc.el(root, "x-item");
    doc.set_attr(element, "value", "7");
    doc.set_attr(element, "other", "x");
    assert!(take(&log).is_empty());

    define(
        &mut doc,
        Probe::new("x-item", &log).observing(&["value", "other"]),
    );

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{element}"),
            format!("x-item:attr#{element} value: <none> -> 7"),
            format!("x-item:attr#{element} other: <none> -> x"),
            format!("x-item:connected#{element}"),
        ],
        "constructed, then the replay in attribute-list order, then connected"
    );
}

#[test]
fn the_replay_covers_only_observed_attributes_and_uses_attribute_list_order() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    let element = doc.el(root, "x-item");
    doc.set_attr(element, "b", "2");
    doc.set_attr(element, "unwatched", "-");
    doc.set_attr(element, "a", "1");

    define(&mut doc, Probe::new("x-item", &log).observing(&["a", "b"]));

    assert_eq!(
        take(&log),
        vec![
            format!("x-item:constructed#{element}"),
            format!("x-item:attr#{element} b: <none> -> 2"),
            format!("x-item:attr#{element} a: <none> -> 1"),
            format!("x-item:connected#{element}"),
        ],
        "the element's attribute order, not the observed list's"
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
fn a_class_no_op_enqueues_nothing() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log).observing(&["class"]));
    let element = doc.el(root, "x-item");
    doc.add_class(element, "one");
    let _ = take(&log);

    doc.add_class(element, "one");
    doc.remove_class(element, "absent");

    assert!(take(&log).is_empty());
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

// --- Connect and disconnect ------------------------------------------------

#[test]
fn connected_fires_once_per_insertion_never_doubled_with_the_upgrade() {
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
fn inserting_into_a_disconnected_parent_upgrades_nothing_until_it_connects() {
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

    doc.dom.detach(inside);
    assert!(
        take(&log).is_empty(),
        "its old parent was never connected, so nothing was ever connected"
    );

    doc.dom.detach(connected);
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
fn remove_subtree_delivers_disconnect_while_the_subtree_is_still_readable() {
    let log = log();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut doc = Doc::new();
    let root = doc.root;
    let recorded = Arc::clone(&seen);
    define(
        &mut doc,
        Probe::new("x-item", &log).on_disconnected(Box::new(move |document, element| {
            // The window in which the node is unlinked but not yet freed.
            let tag = document
                .get(element)
                .and_then(|node| node.tag_name().map(str::to_owned));
            recorded.lock().unwrap().push(tag);
        })),
    );
    let element = doc.el(root, "x-item");
    let _ = take(&log);

    doc.dom.remove_subtree(element);

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

// --- Ordering --------------------------------------------------------------

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

// --- Shadow trees ----------------------------------------------------------

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
fn an_element_inside_a_shadow_tree_is_upgraded_and_connected() {
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
fn an_unassigned_light_child_is_still_upgraded_and_still_connects() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(&mut doc, Probe::new("x-item", &log));
    let host = doc.el(root, "host");
    doc.dom.attach_shadow(host, ShadowRootMode::Open);

    // No slot claims it, so it is invisible to layout and paint — but it is
    // still in the node tree, still connected, and still a custom element.
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

// --- `:defined` ------------------------------------------------------------

#[test]
fn defined_matches_every_ordinary_tag_from_creation() {
    let mut doc = Doc::new();
    let root = doc.root;
    let plain = doc.el(root, "view");
    let unknown = doc.el(root, "asdf");
    let reserved = doc.el(root, "font-face");
    let candidate = doc.el(root, "x-item");
    doc.flush();

    assert!(doc.matches(plain, ":defined"));
    assert!(
        doc.matches(unknown, ":defined"),
        "an unknown tag is uncustomized, which is defined"
    );
    assert!(
        doc.matches(reserved, ":defined"),
        "a reserved name is not a custom element name"
    );
    assert!(
        doc.matches(candidate, ":not(:defined)"),
        "a custom element name with no definition is undefined"
    );
}

#[test]
fn upgrading_flips_defined_and_restyles_without_an_explicit_invalidation() {
    let log = log();
    let mut doc =
        Doc::with_css("x-item:not(:defined) { width: 3px; } x-item:defined { width: 9px; }");
    let root = doc.root;
    let element = doc.el(root, "x-item");
    doc.flush();
    assert_eq!(doc.value(element, "width"), "3px");

    define(&mut doc, Probe::new("x-item", &log));
    doc.flush();

    assert_eq!(
        doc.value(element, "width"),
        "9px",
        "the upgrade's state flip restyles through the ordinary funnel"
    );
}

// --- Adversarial and re-entrancy -------------------------------------------

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
            // Re-enters this very handler while the outer call is on the
            // stack: the ordinary list-component shape.
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

/// A callback that removes a later sibling does **not** cancel that sibling's
/// already-queued reaction: the nested removal opens its own reaction scope,
/// and that scope drains the sibling's whole per-element queue — the connected
/// reaction the outer insertion queued included, ahead of the disconnect it
/// just added. Every browser does the same, because the per-element reaction
/// queue is shared across scopes while only the element queue is stacked.
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
            // Taken out before the mutation: a callback must not hold a lock
            // across a document mutation, because that mutation can re-enter
            // this very callback for another element.
            let doomed = target.lock().unwrap().take();
            if let Some(id) = doomed {
                document.remove_subtree(id);
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
    doc.dom.remove_subtree(element);
    let _ = take(&log);

    let recycled = doc.dom.create_element("plain", ());
    assert_eq!(recycled, element, "the arena reuses the freed slot");
    doc.dom.append_child(root, recycled);

    assert!(
        take(&log).is_empty(),
        "the recycled id inherits nothing from its previous occupant"
    );
}

/// A `NodeId` is a slab key the arena recycles on free, so it is an occupancy
/// token and never an identity one. A constructor that destroys its own
/// element must therefore be refused outright — detaching it is fine, freeing
/// it is not, because the very next creation would take its id back and the
/// caller would be handed a live id naming a different element.
#[test]
#[should_panic(expected = "freeing it is not")]
fn a_constructor_that_frees_the_element_being_created_panics() {
    let log = log();
    let mut doc = Doc::new();
    define(
        &mut doc,
        Probe::new("x-item", &log).on_constructed(Box::new(|document, element| {
            document.remove_subtree(element);
        })),
    );
    doc.dom.create_element("x-item", ());
}

/// The same rule, in the shape that used to slip through every liveness
/// check: free the element *and* create a replacement, so the id is occupied
/// again by the time the guard looks at it.
#[test]
#[should_panic(expected = "freeing it is not")]
fn a_constructor_that_frees_and_recycles_its_own_id_panics() {
    let log = log();
    let mut doc = Doc::new();
    define(
        &mut doc,
        Probe::new("x-item", &log).on_constructed(Box::new(|document, element| {
            document.remove_subtree(element);
            document.create_element("view", ());
        })),
    );
    doc.dom.create_element("x-item", ());
}

/// And on the removal side: a disconnected callback that frees the subtree its
/// caller is still holding.
#[test]
#[should_panic(expected = "freeing it is not")]
fn a_disconnected_callback_that_frees_the_subtree_being_removed_panics() {
    let log = log();
    let mut doc = Doc::new();
    let root = doc.root;
    define(
        &mut doc,
        Probe::new("x-item", &log).on_disconnected(Box::new(|document, element| {
            document.remove_subtree(element);
        })),
    );
    let element = doc.el(root, "x-item");
    doc.dom.remove_subtree(element);
}

/// The depth guard must balance its own counter. Incrementing before the
/// assert leaked it on the panicking path, and `is_draining()` then answered
/// `true` forever — wedging every later style flush on a document a harness
/// still held.
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

    // The document is unspecified as to content, but it must not be wedged:
    // an unrelated mutation and a commit still work.
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
            // No depth guard of its own: every construction starts another.
            document.create_element("x-item", ());
        })),
    );
    doc.el(root, "x-item");
}

#[test]
fn a_definition_added_from_inside_a_callback_upgrades_in_its_own_scope() {
    let log = log();
    let inner_log = log.clone();
    let mut doc = Doc::new();
    let root = doc.root;
    let pending: Arc<Mutex<bool>> = Arc::new(Mutex::new(true));
    let once = Arc::clone(&pending);
    define(
        &mut doc,
        Probe::new("x-outer", &log).on_connected(Box::new(move |document, _| {
            if !std::mem::replace(&mut *once.lock().unwrap(), false) {
                return;
            }
            document.define("x-inner", Box::new(Probe::new("x-inner", &inner_log)));
        })),
    );
    // Built detached, so `x-inner` is already in the subtree when the outer
    // element's connected callback defines it.
    let outer = doc.dom.create_element("x-outer", ());
    let inner = doc.dom.create_element("x-inner", ());
    doc.dom.append_child(outer, inner);
    let _ = take(&log);

    doc.dom.append_child(root, outer);

    let transcript = take(&log);
    assert!(
        transcript.contains(&format!("x-inner:constructed#{inner}")),
        "the nested define upgraded the already-present element: {transcript:?}"
    );
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
                // Appending a child of the same tag re-enters this definition
                // through a nested scope, which must drain before the outer
                // attribute reaction returns.
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
    doc.dom.detach(child);
    doc.dom.append_child(element, child);
    doc.flush();

    assert_eq!(doc.value(child, "width"), "5px");
    assert!(doc.matches(child, ":defined"));
}
