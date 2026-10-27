//! css-scroll-anchoring-1 through `Document::render`: a layout change above
//! what a scroll container shows moves its offset by as much, so the shown
//! content stays where it was.

#![allow(clippy::float_cmp, clippy::cast_precision_loss)]

mod common;

use common::{Doc, device};
use euclid::default::Vector2D;
use terminus::link::NodeId;

/// A 100px-tall scroller of thirty 20px rows: offsets 0..=500.
struct Page {
    doc: Doc,
    scroller: NodeId,
    rows: Vec<NodeId>,
}

fn page(extra: &str) -> Page {
    let mut doc = Doc::with_device(device(200.0, 100.0));
    doc.add_css(&format!(
        "page {{ display: flex; width: 200px; height: 100vh; align-items: flex-start; }}
         .scroller {{ display: flex; flex-direction: column; overflow: scroll;
                      width: 200px; height: 100px; }}
         .row {{ display: flex; flex-shrink: 0; width: 200px; height: 20px; }}
         {extra}"
    ));
    let root = doc.root;
    let scroller = doc.el(root, "view.scroller");
    let rows = (0..30).map(|_| doc.el(scroller, "view.row")).collect();
    let mut page = Page {
        doc,
        scroller,
        rows,
    };
    page.render();
    page
}

impl Page {
    fn render(&mut self) {
        self.doc.dom.render();
    }

    /// As the runtime adopts a painter's post: a scroll anchoring did not
    /// make.
    fn scroll(&mut self, y: f32) {
        self.doc.dom.scroll_to(self.scroller, Vector2D::new(0.0, y));
        self.render();
    }

    fn offset(&self) -> f32 {
        self.doc.dom.scroll_offset(self.scroller).y
    }

    /// Where row `index` stands in the scrollport: its top minus the offset.
    fn on_screen(&self, index: usize) -> f32 {
        let layout = self.doc.dom.rounded_layout(self.rows[index]).unwrap();
        layout.location.y - self.offset()
    }

    fn grow(&mut self, index: usize, height: f32) {
        self.doc
            .set_inline(self.rows[index], &format!("height: {height}px"));
        self.render();
    }
}

#[test]
fn a_row_growing_above_the_shown_ones_moves_the_offset_with_them() {
    let mut page = page("");
    page.scroll(105.0);
    assert_eq!(page.on_screen(5), -5.0);

    page.grow(0, 50.0);
    assert_eq!(
        page.offset(),
        135.0,
        "the offset moved by the 30px row 0 grew"
    );
    assert_eq!(page.on_screen(5), -5.0, "the anchor row stays where it was");
    assert_eq!(
        page.doc.dom.scroll_anchor_shift(page.scroller),
        Vector2D::new(0.0, 30.0)
    );
    let frame = page.doc.dom.committed_frame().unwrap();
    let slot = &frame.scroll_slots()[frame.slot_of(page.scroller).unwrap() as usize];
    assert_eq!(slot.offset, Vector2D::new(0.0, 135.0));
    assert_eq!(
        slot.anchor_shift,
        Vector2D::new(0.0, 30.0),
        "the frame carries the shift to the painter",
    );

    // Shrinking moves it back, and the shift is a sum.
    page.grow(0, 10.0);
    assert_eq!(page.offset(), 95.0);
    assert_eq!(page.on_screen(5), -5.0);
    assert_eq!(
        page.doc.dom.scroll_anchor_shift(page.scroller),
        Vector2D::new(0.0, -10.0)
    );
}

#[test]
fn a_container_at_its_block_start_is_not_anchored() {
    let mut page = page("");
    page.grow(0, 50.0);
    assert_eq!(page.offset(), 0.0, "§2.1: no anchor at offset 0");
    assert_eq!(
        page.doc.dom.scroll_anchor_shift(page.scroller),
        Vector2D::zero()
    );
}

#[test]
fn overflow_anchor_none_opts_the_container_out() {
    let mut page = page(".scroller { overflow-anchor: none; }");
    page.scroll(105.0);
    page.grow(0, 50.0);
    assert_eq!(page.offset(), 105.0);
}

#[test]
fn overflow_anchor_none_on_a_row_passes_the_anchor_to_the_next() {
    let mut page = page(".skip { overflow-anchor: none; }");
    page.doc.dom.add_class(page.rows[5], "skip");
    page.render();
    page.scroll(105.0);
    page.grow(5, 50.0);
    // With row 5 the anchor nothing would move: it grew below its own top.
    assert_eq!(
        page.offset(),
        135.0,
        "row 6 is the anchor, and row 5 growing pushed it 30px down: the offset follows it",
    );
    assert_eq!(page.on_screen(6), 15.0);
}

#[test]
fn a_style_change_on_the_anchor_suppresses_the_adjustment() {
    let mut page = page("");
    page.scroll(105.0);
    page.doc.set_inline(page.rows[5], "margin-top: 10px");
    page.render();
    assert_eq!(
        page.offset(),
        105.0,
        "§2.2.2: a margin change on the anchor clears it instead",
    );
}

#[test]
fn a_scroll_anchoring_did_not_make_selects_a_new_anchor() {
    let mut page = page("");
    page.scroll(105.0);
    page.grow(0, 50.0);
    assert_eq!(page.offset(), 135.0);

    page.scroll(300.0);
    let top = (0..30)
        .find(|&row| page.on_screen(row) + 20.0 > 0.0)
        .unwrap();
    let before = page.on_screen(top);
    page.grow(1, 40.0);
    assert_eq!(page.offset(), 320.0);
    assert_eq!(
        page.on_screen(top),
        before,
        "the anchor at the new offset holds"
    );
}

#[test]
fn rows_revealed_above_the_scrollport_do_not_move_what_it_shows() {
    // `<list>`'s placeholder: each row skips at one scrollport tall and is
    // 20px once revealed.
    let mut page = page(
        ".row { height: auto; flex-direction: column; content-visibility: auto;
                contain-intrinsic-size: none auto 100px; }
         .cell { height: 20px; }",
    );
    for &row in &page.rows.clone() {
        page.doc.el(row, "view.cell");
    }
    let settle = |page: &mut Page| {
        while page.doc.dom.committed_frame().unwrap().needs_main_ticks() {
            page.doc.dom.resume_relevance();
            page.render();
        }
    };
    page.render();
    settle(&mut page);
    // Into the placeholders, 20px into one of them.
    let y = page
        .doc
        .dom
        .rounded_layout(page.rows[20])
        .unwrap()
        .location
        .y
        + 20.0;
    page.scroll(y);
    settle(&mut page);
    let shown: Vec<f32> = (0..30)
        .filter(|&row| {
            let top = page.on_screen(row);
            let height = page
                .doc
                .dom
                .rounded_layout(page.rows[row])
                .unwrap()
                .size
                .height;
            top + height > 0.0 && top < 100.0
        })
        .map(|row| {
            page.doc
                .dom
                .rounded_layout(page.rows[row])
                .unwrap()
                .size
                .height
        })
        .collect();
    assert!(
        shown.iter().all(|&height| height == 20.0),
        "every row in the scrollport is revealed: {shown:?}",
    );
    assert!(
        page.on_screen(20) <= 0.0 && page.on_screen(20) + 20.0 >= 0.0,
        "row 20, the anchor the scroll landed in, stays at the scrollport's top \
         (its bottom no higher than the top edge): {}",
        page.on_screen(20),
    );
}
