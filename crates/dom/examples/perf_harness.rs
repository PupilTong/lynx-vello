//! ReactLynx-shaped profiling harness (new perf test, self-authored).
//!
//! Models the workload a ReactLynx list page produces: a few thousand
//! elements in a scroll-view list, each row a `view` with classes and inline
//! styles wrapping `text` nodes, then incremental updates (inline style
//! writes, class flips) each followed by a style flush + layout + scene
//! rebuild. Run under a sampling profiler to find hot paths.

use std::hint::black_box;
use std::time::Instant;

use dom::{Device, Document, StylesheetOrigin};

const CSS: &str = r#"
    page, view, text, image, scroll-view { overflow: hidden; box-sizing: border-box; }
    page { display: flex; flex-direction: column; width: 100%; height: 100%; }
    .list { display: flex; flex-direction: column; overflow-y: scroll;
            flex-grow: 1; flex-basis: 0px; }
    .row { display: flex; flex-direction: row; padding: 8px; margin: 2px; }
    .row.selected { background-color: rgb(200, 220, 255); }
    .avatar { width: 40px; height: 40px; border-radius: 20px; background-color: rgb(180, 180, 180); }
    .col { display: flex; flex-direction: column; flex-grow: 1; }
    .title { font-size: 16px; color: rgb(20, 20, 20); }
    .subtitle { font-size: 12px; color: rgb(120, 120, 120); }
    .badge { width: 24px; height: 24px; background-color: rgb(255, 80, 80); }
    .row:nth-child(2n) { background-color: rgb(245, 245, 245); }
"#;

const ROWS: usize = 1024;
const NB: usize = 1;

fn time<R>(label: &str, mut f: impl FnMut() -> R) -> R {
    let start = Instant::now();
    let r = f();
    println!("{label:32} {:>10.3} ms", start.elapsed().as_secs_f64() * 1e3);
    r
}

fn commit(doc: &mut Document<()>) {
    doc.layout();
    black_box(doc.render());
}

fn main() {
    let mode = std::env::args().nth(2).unwrap_or_default();
    if mode == "single" {
        // Loop: flip one row's height, commit. For profiler attribution.
        let device = Device::new(390.0, 844.0, 3.0);
        let mut doc = Document::new(device, "page", ());
        doc.add_stylesheet(CSS, StylesheetOrigin::Author);
        let root = doc.document_element().id();
        let list = doc.create_element("scroll-view", ());
        doc.set_classes(list, "list");
        doc.append_child(root, list);
        let mut rows = Vec::new();
        for i in 0..ROWS {
            let row = doc.create_element("view", ());
            doc.set_classes(row, "row");
            doc.set_inline_style(row, "height: 56px");
            let t = doc.create_element("text", ());
            doc.set_classes(t, "title");
            let tt = doc.create_text_node(format!("Row title {i}"), ());
            doc.append_child(t, tt);
            doc.append_child(row, t);
            doc.append_child(list, row);
            rows.push(row);
        }
        commit(&mut doc);
        let n: usize = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);
        let start = Instant::now();
        for k in 0..n {
            let h = 56 + (k % 2);
            doc.set_inline_style_property(rows[0], "height", &format!("{h}px"));
            commit(&mut doc);
        }
        println!(
            "single-row flip commit avg {:.3} ms",
            start.elapsed().as_secs_f64() * 1e3 / n as f64
        );
        return;
    }
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    for _ in 0..iterations {
        let device = Device::new(390.0, 844.0, 3.0);
        let mut doc = Document::new(device, "page", ());
        doc.add_stylesheet(CSS, StylesheetOrigin::Author);

        let mut rows = Vec::with_capacity(ROWS);
        time("build 1024-row list", || {
            let root = doc.document_element().id();
            let list = doc.create_element("scroll-view", ());
            doc.set_classes(list, "list");
            doc.append_child(root, list);
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
                rows.push(row);
            }
        });

        time("initial commit", || commit(&mut doc));
        time("noop commit", || commit(&mut doc));

        time("inline style x256 + commit", || {
            for (i, &row) in rows.iter().enumerate().take(NB) {
                let h = 56 + (i % 8);
                doc.set_inline_style(row, &format!("height: {h}px"));
            }
            commit(&mut doc);
        });

        time("inline property x256 + commit", || {
            for (i, &row) in rows.iter().enumerate().take(NB) {
                let h = 57 + (i % 8);
                doc.set_inline_style_property(row, "height", &format!("{h}px"));
            }
            commit(&mut doc);
        });

        time("class flip x256 + commit", || {
            for &row in rows.iter().take(NB) {
                doc.set_classes(row, "row selected");
            }
            commit(&mut doc);
        });

        time("class unflip x256 + commit", || {
            for &row in rows.iter().take(NB) {
                doc.set_classes(row, "row");
            }
            commit(&mut doc);
        });

        time("text update x256 + commit", || {
            for (i, &row) in rows.iter().enumerate().take(NB) {
                let col = doc.get(row).unwrap().children().nth(1).unwrap().id();
                let title = doc.get(col).unwrap().children().next().unwrap().id();
                let text = doc.get(title).unwrap().children().next().unwrap().id();
                doc.set_text_node_data(text, format!("Row retitled {i}"));
            }
            commit(&mut doc);
        });

        time("teardown", || {
            let root = doc.document_element().id();
            let list = doc.get(root).unwrap().children().next().unwrap().id();
            black_box(doc.drop_subtree(list));
            commit(&mut doc);
        });
    }
}
