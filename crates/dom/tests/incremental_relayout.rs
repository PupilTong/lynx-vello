//! Incremental relayout equivalence: a document mutated and re-laid-out must
//! end with exactly the geometry of a fresh document built directly in the
//! final state — whether the mutation resolves through an in-place
//! committed-input relayout, escalates to a whole-tree pass, or reaches the
//! root outright.

use dom::{Device, Document, StylesheetOrigin};

// Mirrors the Lynx UA sheet (`bobcat-core`): border-box, overflow hidden,
// viewport-anchored page. That anchoring is what licenses the in-place
// relayout path, so these tests exercise it rather than always falling back
// to a whole-tree pass.
const CSS: &str = "
    page, view, text, image, scroll-view { overflow: hidden; box-sizing: border-box; }
    page { display: flex; flex-direction: column; width: 100%; height: 100%; }
    .list { display: flex; flex-direction: column; overflow-y: scroll;
            flex-grow: 1; flex-basis: 0px; }
    .row { display: flex; flex-direction: row; padding: 8px; margin: 2px; }
    .cell { display: flex; flex-direction: column; flex-grow: 1; }
    .fixed { width: 40px; height: 40px; }
    .grown { width: 40px; height: 64px; }
    .visible-row { display: flex; flex-direction: row; overflow: visible; }
";

fn doc() -> Document<()> {
    let mut doc = Document::new(Device::new(390.0, 844.0, 2.0), "page", ());
    doc.add_stylesheet(CSS, StylesheetOrigin::Author);
    doc
}

struct Page {
    doc: Document<()>,
    rows: Vec<dom::NodeId>,
    cells: Vec<dom::NodeId>,
    texts: Vec<dom::NodeId>,
}

fn build(row_class: &str, text: &str, rows: usize) -> Page {
    let mut page = Page {
        doc: doc(),
        rows: Vec::new(),
        cells: Vec::new(),
        texts: Vec::new(),
    };
    let root = page.doc.document_element().id();
    let list = page.doc.create_element("scroll-view", ());
    page.doc.set_classes(list, "list");
    page.doc.append_child(root, list);
    for i in 0..rows {
        let row = page.doc.create_element("view", ());
        page.doc.set_classes(row, row_class);
        page.doc.set_inline_style(row, "height: 56px");
        let fixed = page.doc.create_element("view", ());
        page.doc.set_classes(fixed, "fixed");
        let cell = page.doc.create_element("view", ());
        page.doc.set_classes(cell, "cell");
        let label = page.doc.create_element("text", ());
        let run = page
            .doc
            .create_text_node(format!("{text} {i}"), ());
        page.doc.append_child(label, run);
        page.doc.append_child(cell, label);
        page.doc.append_child(row, fixed);
        page.doc.append_child(row, cell);
        page.doc.append_child(list, row);
        page.rows.push(row);
        page.cells.push(cell);
        page.texts.push(run);
    }
    page
}

/// Every live node's rounded layout, in preorder document position.
fn geometry(doc: &Document<()>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![doc.document_element().id()];
    while let Some(id) = stack.pop() {
        if let Some(layout) = doc.rounded_layout(id) {
            out.push((format!("{id:?}"), format!("{layout:?}")));
        }
        if let Some(node) = doc.get(id) {
            stack.extend(node.children().map(dom::Node::id));
        }
    }
    out.sort();
    out
}

fn assert_same_geometry(mutated: &Page, fresh: &Page) {
    let got = geometry(&mutated.doc);
    let expected = geometry(&fresh.doc);
    assert_eq!(
        got.len(),
        expected.len(),
        "both documents must lay out the same node set"
    );
    let mut diverged = 0;
    for ((id_a, a), (id_b, b)) in got.iter().zip(&expected) {
        assert_eq!(id_a, id_b);
        if a != b {
            diverged += 1;
            eprintln!("node {id_a} diverged:\n  incremental: {a}\n  fresh:       {b}");
        }
    }
    assert_eq!(diverged, 0, "{diverged} nodes diverged after incremental relayout");
}

#[test]
fn same_size_text_change_relayouts_in_place_to_the_fresh_result() {
    let mut mutated = build("row", "alpha", 24);
    mutated.doc.layout();
    for (i, &run) in mutated.texts.iter().enumerate() {
        mutated.doc.set_text_node_data(run, format!("bravo {i}"));
    }
    mutated.doc.layout();

    let mut fresh = build("row", "bravo", 24);
    fresh.doc.layout();
    assert_same_geometry(&mutated, &fresh);
}

#[test]
fn a_growing_subtree_escalates_and_still_matches_the_fresh_result() {
    let mut mutated = build("row", "alpha", 24);
    mutated.doc.layout();
    // Growing one grandchild taller than its row's fixed sibling changes the
    // cell's content size; whatever path the engine picks, geometry must land
    // exactly where a fresh layout lands.
    let target = mutated.cells[7];
    let grown = mutated.doc.create_element("view", ());
    mutated.doc.set_classes(grown, "grown");
    mutated.doc.append_child(target, grown);
    mutated.doc.layout();

    let mut fresh = build("row", "alpha", 24);
    let target = fresh.cells[7];
    let grown = fresh.doc.create_element("view", ());
    fresh.doc.set_classes(grown, "grown");
    fresh.doc.append_child(target, grown);
    fresh.doc.layout();
    assert_same_geometry(&mutated, &fresh);
}

#[test]
fn a_row_height_change_reaches_the_root_and_matches_the_fresh_result() {
    let mut mutated = build("row", "alpha", 24);
    mutated.doc.layout();
    mutated
        .doc
        .set_inline_style_property(mutated.rows[3], "height", "72px");
    mutated.doc.layout();

    let mut fresh = build("row", "alpha", 24);
    fresh.doc.layout();
    fresh
        .doc
        .set_inline_style_property(fresh.rows[3], "height", "72px");
    fresh.doc.layout();
    // The fresh document took the same mutation, so this also checks the
    // incremental result against a converged double-layout.
    let mut rebuilt = build("row", "alpha", 24);
    rebuilt
        .doc
        .set_inline_style_property(rebuilt.rows[3], "height", "72px");
    rebuilt.doc.layout();
    assert_same_geometry(&mutated, &rebuilt);
    assert_same_geometry(&fresh, &rebuilt);
}

#[test]
fn overflow_visible_rows_stay_correct_without_the_in_place_path() {
    let mut mutated = build("visible-row", "alpha", 24);
    mutated.doc.layout();
    for (i, &run) in mutated.texts.iter().enumerate() {
        mutated
            .doc
            .set_text_node_data(run, format!("charlie delta {i}"));
    }
    mutated.doc.layout();

    let mut fresh = build("visible-row", "charlie delta", 24);
    fresh.doc.layout();
    assert_same_geometry(&mutated, &fresh);
}

#[test]
fn repeated_alternating_mutations_converge_to_each_fresh_state() {
    let mut mutated = build("row", "alpha", 16);
    mutated.doc.layout();
    for round in 0..4 {
        let text = if round % 2 == 0 { "bravo" } else { "alpha" };
        for (i, &run) in mutated.texts.iter().enumerate() {
            mutated.doc.set_text_node_data(run, format!("{text} {i}"));
        }
        mutated
            .doc
            .set_inline_style_property(mutated.rows[round], "height", "60px");
        mutated.doc.layout();
    }

    let mut fresh = build("row", "alpha", 16);
    for round in 0..4 {
        fresh
            .doc
            .set_inline_style_property(fresh.rows[round], "height", "60px");
    }
    fresh.doc.layout();
    assert_same_geometry(&mutated, &fresh);
}

#[test]
fn initial_layouts_of_identical_documents_agree() {
    let mut a = build("row", "alpha", 24);
    a.doc.layout();
    let mut b = build("row", "alpha", 24);
    b.doc.layout();
    assert_same_geometry(&a, &b);
}

#[test]
fn a_second_noop_layout_changes_nothing() {
    let mut a = build("row", "alpha", 24);
    a.doc.layout();
    let before = geometry(&a.doc);
    a.doc.set_text_node_data(a.texts[0], "alpha 0".to_string());
    a.doc.layout();
    let after = geometry(&a.doc);
    assert_eq!(before, after);
}

#[test]
fn minimal_grow_repro() {
    let mut a = build("row", "alpha", 2);
    a.doc.layout();
    eprintln!("== after initial layout ==");
    for (id, l) in geometry(&a.doc) {
        eprintln!("  {id}: {l}");
    }
    let grown = a.doc.create_element("view", ());
    a.doc.set_classes(grown, "grown");
    a.doc.append_child(a.cells[0], grown);
    a.doc.layout();
    eprintln!("== after grow+layout ==");
    for (id, l) in geometry(&a.doc) {
        eprintln!("  {id}: {l}");
    }
    let mut b = build("row", "alpha", 2);
    let grown = b.doc.create_element("view", ());
    b.doc.set_classes(grown, "grown");
    b.doc.append_child(b.cells[0], grown);
    b.doc.layout();
    eprintln!("== fresh ==");
    for (id, l) in geometry(&b.doc) {
        eprintln!("  {id}: {l}");
    }
    assert_same_geometry(&a, &b);
}
