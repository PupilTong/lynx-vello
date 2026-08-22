//! Heap-accounting harness: builds the ReactLynx-shaped list page and reports
//! live heap bytes per phase through a counting global allocator.

#![allow(
    unsafe_code,
    missing_debug_implementations,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::needless_raw_string_hashes,
    reason = "measurement harness: a counting global allocator and ratio prints"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use dom::{Device, Document, Node, StylesheetOrigin};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            let live = LIVE
                .fetch_add(new_size.wrapping_sub(layout.size()), Ordering::Relaxed)
                .wrapping_add(new_size.wrapping_sub(layout.size()));
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const CSS: &str = r#"
    page, view, text, image, scroll-view { overflow: hidden; box-sizing: border-box; }
    page { display: flex; flex-direction: column; width: 100%; height: 100%; }
    .list { display: flex; flex-direction: column; overflow-y: scroll;
            flex-grow: 1; flex-basis: 0px; }
    .row { display: flex; flex-direction: row; padding: 8px; margin: 2px; }
    .avatar { width: 40px; height: 40px; background-color: rgb(180, 180, 180); }
    .col { display: flex; flex-direction: column; flex-grow: 1; }
    .title { font-size: 16px; color: rgb(20, 20, 20); }
    .subtitle { font-size: 12px; color: rgb(120, 120, 120); }
    .badge { width: 24px; height: 24px; background-color: rgb(255, 80, 80); }
"#;

const ROWS: usize = 1024;

fn report(label: &str, base: usize) {
    let live = LIVE.load(Ordering::Relaxed);
    println!(
        "{label:28} live {:>9.2} MiB  (delta {:>+9.2} KiB)",
        live as f64 / (1024.0 * 1024.0),
        (live as f64 - base as f64) / 1024.0,
    );
}

fn main() {
    let start = LIVE.load(Ordering::Relaxed);
    let device = Device::new(390.0, 844.0, 3.0);
    let mut doc = Document::new(device, "page", ());
    doc.add_stylesheet(CSS, StylesheetOrigin::Author);
    report("empty document", start);

    let before_build = LIVE.load(Ordering::Relaxed);
    let root = doc.document_element().id();
    let list = doc.create_element("scroll-view", ());
    doc.set_classes(list, "list");
    doc.append_child(root, list);
    let mut node_count = 2usize;
    for i in 0..ROWS {
        let row = doc.create_element("view", ());
        doc.set_classes(row, "row");
        doc.set_inline_style(row, "height: 56px");
        let avatar = doc.create_element("view", ());
        doc.set_classes(avatar, "avatar");
        let col = doc.create_element("view", ());
        doc.set_classes(col, "col");
        let title = doc.create_element("text", ());
        doc.set_classes(title, "title");
        let title_text = doc.create_text_node(format!("Row title {i}"), ());
        let subtitle = doc.create_element("text", ());
        doc.set_classes(subtitle, "subtitle");
        let subtitle_text = doc.create_text_node("subtitle line of text", ());
        let badge = doc.create_element("view", ());
        doc.set_classes(badge, "badge");
        doc.append_child(title, title_text);
        doc.append_child(subtitle, subtitle_text);
        doc.append_child(col, title);
        doc.append_child(col, subtitle);
        doc.append_child(row, avatar);
        doc.append_child(row, col);
        doc.append_child(row, badge);
        doc.append_child(list, row);
        node_count += 8;
    }
    report("after build", before_build);
    let built = LIVE.load(Ordering::Relaxed);

    doc.layout();
    report("after style+layout", built);
    let laid = LIVE.load(Ordering::Relaxed);

    doc.render();
    report("after first render", laid);

    let per_node = (LIVE.load(Ordering::Relaxed) - before_build) as f64 / node_count as f64;
    println!(
        "\nnodes: {node_count}   bytes/node (build+layout+scene): {per_node:.0}   peak: {:.2} MiB",
        PEAK.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
    );

    census(&doc);
}

/// Node counts by kind, and what the one-node-type-for-all-kinds layout costs.
///
/// This prices bytes, not loads. Every element-only field is inline on every
/// node because that is what made the styling hot paths fast — the A/B that put
/// `StylingData` inline moved `noop_commit` by 13% — so a byte total here is an
/// input to that trade, never a verdict on it.
fn census(doc: &Document<()>) {
    let mut counts = [0usize; 4];
    let (document, element, text, shadow) = (0, 1, 2, 3);

    let mut stack = vec![doc.document_element().id()];
    // The document node parents the document element, so start one above it.
    if let Some(root) = doc.document_element().parent_id() {
        stack = vec![root];
    }
    let mut text_attr_bytes = 0usize;
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        let kind = if node.is_element() {
            element
        } else if node.is_text_node() {
            text
        } else if node.is_shadow_root() {
            shadow
        } else {
            document
        };
        counts[kind] += 1;
        if kind == text {
            text_attr_bytes += node.attributes().count();
        }
        stack.extend(node.child_ids().iter().copied());
        if let Some(root) = doc.shadow_root(id) {
            stack.push(root);
        }
    }

    let fields = Node::<()>::census_field_sizes();
    let node_size = std::mem::size_of::<Node<()>>();
    let field_total: usize = fields.iter().map(|&(_, size, _)| size).sum();
    let element_only: usize = fields
        .iter()
        .filter(|&&(_, _, only)| only)
        .map(|&(_, size, _)| size)
        .sum();

    println!(
        "\n--- node census ({} profile) ---",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "size_of::<Node<()>>() = {node_size} B   (fields {field_total} B + {} B padding)",
        node_size.saturating_sub(field_total),
    );
    for &(name, size, only) in fields {
        println!(
            "  {name:<20} {size:>4} B  {}",
            if only { "element-only" } else { "" }
        );
    }
    println!(
        "\n  document {:>6}   element {:>6}   text {:>6}   shadow-root {:>6}",
        counts[document], counts[element], counts[text], counts[shadow],
    );
    let dead = (counts[document] + counts[text] + counts[shadow]) * element_only;
    println!(
        "\n  element-only fields: {element_only} B/node, inline on every node.\n  \
         carried by the {} non-element nodes: {:.1} KiB ({:.1}% of the {:.1} KiB of node storage)",
        counts[document] + counts[text] + counts[shadow],
        dead as f64 / 1024.0,
        100.0 * dead as f64 / (counts.iter().sum::<usize>() * node_size) as f64,
        (counts.iter().sum::<usize>() * node_size) as f64 / 1024.0,
    );
    println!(
        "  text nodes hold {text_attr_bytes} attributes between them, so the \
         attribute\n  machinery on them is dead as well as unused."
    );
    println!(
        "  against {:.2} MiB of live heap for the whole page, that is {:.1}%.",
        LIVE.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0),
        100.0 * dead as f64 / LIVE.load(Ordering::Relaxed) as f64,
    );
}
