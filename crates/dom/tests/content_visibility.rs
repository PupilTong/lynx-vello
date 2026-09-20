//! `content-visibility: auto` relevance determination
//! ([css-contain-2 §4.1](https://drafts.csswg.org/css-contain-2/#relevant-to-the-user)).
//!
//! Every case here goes through `Document::render`, never a bare `layout()`:
//! relevance is determined by the *rendering update*, and an `auto` box no
//! rendering update has reached skips its contents. The reveal happens inside
//! the same commit, so one `render()` is always enough — no frame is ever
//! published with a flip pending.
//!
//! Skipping is read two ways, and both matter. `text_block_size` is `None`
//! for a label whose paragraph was never shaped (or whose shaped paragraph
//! was thrown away when its row went back to skipping), which is the
//! statement that skipping reaches all the way into the text engine rather
//! than merely hiding a box. `rounded_layout` is the zero box for everything
//! under a skipped row.

#![allow(clippy::float_cmp, clippy::cast_precision_loss)]

mod common;

use common::{Doc, device};
use dom::{FontBlob, NodeId};
use euclid::default::{Point2D, Vector2D};

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");

/// 200 x 100 CSS px of viewport, a 100px-tall `overflow: hidden` scroller,
/// and `ROWS` rows of 20px each.
///
/// The row height and `contain-intrinsic-size` agree exactly — Ahem's line
/// box at `font-size: 20px` is 20px tall — so a reveal never moves anything,
/// which is what makes the scroll range stable across one.
const ROWS: usize = 20;
const ROW_HEIGHT: f32 = 20.0;
const VIEWPORT_HEIGHT: f32 = 100.0;

/// The row indices `render()` must have laid out, at this scroll offset and
/// scrollport height — derived here from the spec of the margin rather than
/// from the implementation, so the two have to agree.
///
/// The margin is the painter's encode window: one scrollport past the
/// committed offset in each direction, clamped to the scroll range. The
/// scrollport clips the admitted region to `[0, scrollport]` in the
/// scroller's own coordinates, and the window then carries it by
/// `[low, high]`, giving the band `[low, scrollport + high]` in unscrolled
/// content coordinates. Touching the band counts as reaching it: the cull
/// test admits anything it cannot rule out.
fn admitted_rows(offset: f32, scrollport: f32) -> Vec<usize> {
    let content = ROW_HEIGHT * ROWS as f32;
    let max_offset = (content - scrollport).max(0.0);
    let low = (offset - scrollport).max(0.0);
    let high = (offset + scrollport).min(max_offset);
    let (top, bottom) = (low, scrollport + high);
    (0..ROWS)
        .filter(|&row| {
            let y = ROW_HEIGHT * row as f32;
            y <= bottom && y + ROW_HEIGHT >= top
        })
        .collect()
}

struct Page {
    doc: Doc,
    scroller: NodeId,
    rows: Vec<NodeId>,
    labels: Vec<NodeId>,
}

impl Page {
    /// `extra` is appended to the stylesheet so a case can add its own rules.
    fn new(extra: &str) -> Self {
        let mut doc = Doc::with_device(device(200.0, VIEWPORT_HEIGHT));
        doc.add_css(&format!(
            "page {{ display: flex; width: 200px; height: 100vh;
                     align-items: flex-start; font-family: Ahem; }}
             .scroller {{ display: flex; flex-direction: column; overflow: hidden;
                          width: 200px; height: 100vh;
                          align-items: flex-start; }}
             .row {{ display: flex; width: 200px; flex-shrink: 0;
                     content-visibility: auto; contain-intrinsic-size: 200px {ROW_HEIGHT}px; }}
             .label {{ display: -lynx-text; font-size: {ROW_HEIGHT}px; }}
             {extra}"
        ));
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = doc.root;
        let scroller = doc.el(root, "view.scroller");
        let mut rows = Vec::with_capacity(ROWS);
        let mut labels = Vec::with_capacity(ROWS);
        for _ in 0..ROWS {
            let row = doc.el(scroller, "view.row");
            let label = doc.el(row, "text.label");
            let run = doc.dom.create_text_node("x", ());
            doc.dom.append_child(label, run);
            rows.push(row);
            labels.push(label);
        }
        Self {
            doc,
            scroller,
            rows,
            labels,
        }
    }

    /// The rows whose contents this render actually laid out, read off the
    /// shaped paragraphs rather than off the bit.
    fn laid_out_rows(&self) -> Vec<usize> {
        (0..ROWS)
            .filter(|&row| self.doc.dom.text_block_size(self.labels[row]).is_some())
            .collect()
    }

    fn row_rect(&self, row: usize) -> (f32, f32, f32, f32) {
        rect(&self.doc.dom, self.rows[row])
    }

    fn label_rect(&self, row: usize) -> (f32, f32, f32, f32) {
        rect(&self.doc.dom, self.labels[row])
    }

    fn scroll_to(&mut self, offset: f32) -> Vector2D<f32> {
        self.doc
            .dom
            .scroll_to(self.scroller, Vector2D::new(0.0, offset))
    }
}

fn rect(dom: &dom::Document<()>, id: NodeId) -> (f32, f32, f32, f32) {
    let layout = dom.rounded_layout(id).expect("node id is live");
    (
        layout.location.x,
        layout.location.y,
        layout.size.width,
        layout.size.height,
    )
}

#[test]
fn the_first_render_lays_out_only_the_rows_inside_the_encode_window() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render(), "the first render commits a frame");

    assert_eq!(
        page.laid_out_rows(),
        admitted_rows(0.0, VIEWPORT_HEIGHT),
        "only the rows the encode window admits are shaped",
    );
    assert_eq!(
        page.label_rect(0),
        (0.0, 0.0, ROW_HEIGHT, ROW_HEIGHT),
        "a relevant row lays its label out, one Ahem glyph wide",
    );
    assert_eq!(
        page.label_rect(ROWS - 1),
        (0.0, 0.0, 0.0, 0.0),
        "a skipped row's contents keep the zero box the hide pass left",
    );
    // Every row still has a box of its own, at the position and size its
    // `contain-intrinsic-size` says.
    for row in 0..ROWS {
        assert_eq!(
            page.row_rect(row),
            (0.0, ROW_HEIGHT * row as f32, 200.0, ROW_HEIGHT),
            "row {row} keeps its own geometry whether or not it skips",
        );
    }
}

#[test]
fn a_scroll_reveals_and_re_skips_rows_in_one_render() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    let before = page.laid_out_rows();
    assert_eq!(before, admitted_rows(0.0, VIEWPORT_HEIGHT));

    // Half a window is composable from the committed frame and changes
    // nothing: the encode window already covers it, which is exactly why
    // relevance is defined against that window.
    assert_eq!(page.scroll_to(50.0).y, 50.0);
    assert!(
        !page.doc.dom.needs_render(),
        "a scroll inside the committed encode window needs no new frame",
    );

    // Past the window, the frame can no longer be composed, so a render is
    // owed — and that render re-determines every row.
    assert_eq!(page.scroll_to(150.0).y, 150.0);
    assert!(
        page.doc.dom.needs_render(),
        "a scroll past the encode window makes the retained frame stale",
    );
    assert!(page.doc.dom.render(), "and the render commits");

    let after = page.laid_out_rows();
    assert_eq!(after, admitted_rows(150.0, VIEWPORT_HEIGHT));
    assert!(
        after.iter().any(|row| !before.contains(row)),
        "rows entering the new window were revealed: {before:?} -> {after:?}",
    );
    assert!(
        before.iter().any(|row| !after.contains(row)),
        "rows leaving it went back to skipping: {before:?} -> {after:?}",
    );
    assert_eq!(
        page.label_rect(0),
        (0.0, 0.0, 0.0, 0.0),
        "a re-skipped row's contents are hidden again",
    );
    assert!(
        !page.doc.dom.needs_render(),
        "the reveal happened inside this commit, so nothing is left owed",
    );
}

#[test]
fn a_nested_auto_box_is_determined_in_the_same_commit() {
    let mut page = Page::new(
        ".inner { display: flex; width: 200px; content-visibility: auto;
                  contain-intrinsic-size: 200px 20px; }",
    );
    // The nesting goes inside row 0, which is on screen: the inner box has
    // no paint item at all until its row is revealed, so determining it
    // takes a second pass of the very same render.
    let inner = page.doc.el(page.rows[0], "view.inner");
    let inner_label = page.doc.el(inner, "text.label");
    let run = page.doc.dom.create_text_node("y", ());
    page.doc.dom.append_child(inner_label, run);

    assert!(
        page.doc.dom.render(),
        "one render, however deep the nesting"
    );
    assert!(
        page.doc.dom.text_block_size(inner_label).is_some(),
        "the nested auto box was determined relevant in this same commit",
    );
    assert_eq!(rect(&page.doc.dom, inner_label).3, ROW_HEIGHT);
    assert!(!page.doc.dom.needs_render());

    // The same nesting below the window stays skipped, both levels.
    let deep = page.doc.el(page.rows[ROWS - 1], "view.inner");
    let deep_label = page.doc.el(deep, "text.label");
    let deep_run = page.doc.dom.create_text_node("z", ());
    page.doc.dom.append_child(deep_label, deep_run);
    assert!(page.doc.dom.render());
    assert!(
        page.doc.dom.text_block_size(deep_label).is_none(),
        "a nested auto box under a skipped row is never reached",
    );
}

#[test]
fn an_auto_box_outside_every_scroller_follows_the_viewport() {
    let mut doc = Doc::with_device(device(200.0, VIEWPORT_HEIGHT));
    doc.add_css(
        "page { display: flex; width: 200px; height: 100px; font-family: Ahem; }
         .far { display: flex; position: absolute; left: 0; top: 400px;
                width: 100px; content-visibility: auto;
                contain-intrinsic-size: 100px 20px; }
         .label { display: -lynx-text; font-size: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let far = doc.el(root, "view.far");
    let label = doc.el(far, "text.label");
    let run = doc.dom.create_text_node("x", ());
    doc.dom.append_child(label, run);

    assert!(doc.dom.render());
    assert!(
        doc.dom.text_block_size(label).is_none(),
        "400px below a 100px viewport, with no scroller to widen the margin",
    );

    // Moving it into view is an ordinary style change: the render it forces
    // determines the box and reveals it in that same commit.
    doc.set_inline(far, "top: 10px");
    assert!(doc.dom.render());
    assert!(
        doc.dom.text_block_size(label).is_some(),
        "the style change and the reveal land in one render",
    );
    // A `-lynx-text` flex item is as wide as its one Ahem glyph.
    assert_eq!(rect(&doc.dom, label), (0.0, 0.0, 20.0, 20.0));
    assert!(!doc.dom.needs_render());
}

#[test]
fn a_computed_value_that_is_no_longer_auto_ignores_the_bit() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    let skipped = ROWS - 1;
    assert!(page.doc.dom.text_block_size(page.labels[skipped]).is_none());

    // `visible` reveals a box whose bit says skipped.
    page.doc
        .set_inline(page.rows[skipped], "content-visibility: visible");
    assert!(page.doc.dom.render());
    assert!(
        page.doc.dom.text_block_size(page.labels[skipped]).is_some(),
        "auto -> visible reveals regardless of the bit",
    );

    // `hidden` skips a box whose bit says relevant.
    page.doc
        .set_inline(page.rows[0], "content-visibility: hidden");
    assert!(page.doc.dom.render());
    assert!(
        page.doc.dom.text_block_size(page.labels[0]).is_none(),
        "auto -> hidden skips regardless of the bit",
    );
    assert_eq!(
        page.row_rect(0),
        (0.0, 0.0, 200.0, ROW_HEIGHT),
        "and sizes from contain-intrinsic-size like any skipped box",
    );
}

#[test]
fn the_scroll_range_is_stable_across_a_reveal() {
    let mut page = Page::new("");
    let range = |page: &Page| {
        page.doc
            .dom
            .rounded_layout(page.scroller)
            .expect("live")
            .content_size
            .height
    };
    assert!(page.doc.dom.render());
    let first = range(&page);
    assert_eq!(
        first,
        ROW_HEIGHT * ROWS as f32,
        "every row contributes its box, skipped or not",
    );

    assert_eq!(page.scroll_to(150.0).y, 150.0);
    assert!(page.doc.dom.render());
    assert_eq!(
        range(&page),
        first,
        "revealing rows whose estimate matched moves no scroll range",
    );
    assert_eq!(
        page.scroll_to(300.0).y,
        300.0,
        "so the bottom of the range is where it was",
    );
}

#[test]
fn a_transformed_ancestor_moves_the_relevance_test_with_it() {
    let mut doc = Doc::with_device(device(200.0, VIEWPORT_HEIGHT));
    doc.add_css(
        "page { display: flex; width: 200px; height: 100px; font-family: Ahem; }
         .mover { display: flex; width: 200px; height: 20px; }
         .cv { display: flex; width: 200px; content-visibility: auto;
               contain-intrinsic-size: 200px 20px; }
         .label { display: -lynx-text; font-size: 20px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let mover = doc.el(root, "view.mover");
    let cv = doc.el(mover, "view.cv");
    let label = doc.el(cv, "text.label");
    let run = doc.dom.create_text_node("x", ());
    doc.dom.append_child(label, run);

    // Laid out at the top of the viewport, but translated far above it.
    doc.set_inline(mover, "transform: translateY(-500px)");
    assert!(doc.dom.render());
    assert!(
        doc.dom.text_block_size(label).is_none(),
        "the ancestor transform is part of the world matrix the test uses",
    );

    doc.set_inline(mover, "transform: translateY(0px)");
    assert!(doc.dom.render());
    assert!(
        doc.dom.text_block_size(label).is_some(),
        "and untranslating it reveals the box in that render",
    );
}

#[test]
fn a_revealed_row_is_hit_testable_after_the_same_render() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    // Row 8 is off screen but inside the window at offset 0, so it is
    // already laid out; scrolling brings it under the pointer.
    assert_eq!(page.scroll_to(150.0).y, 150.0);
    assert!(page.doc.dom.render());

    // Viewport y = 10 is content y = 160, which is row 8.
    let hits = page.doc.dom.elements_from_point(Point2D::new(10.0, 10.0));
    assert_eq!(
        hits.first().copied(),
        Some(page.labels[8]),
        "the revealed row's own content is the front-most hit, got {hits:?}",
    );
    assert!(
        hits.contains(&page.rows[8]) && hits.contains(&page.scroller),
        "with its row and the scroller behind it, got {hits:?}",
    );
}

#[test]
fn a_viewport_change_re_determines_every_auto_box() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    assert_eq!(page.laid_out_rows(), admitted_rows(0.0, VIEWPORT_HEIGHT));

    // A taller viewport is a taller scrollport, so both the visible band and
    // the encode window around it grow.
    page.doc.dom.set_viewport(200.0, 200.0);
    assert!(
        page.doc.dom.needs_render(),
        "a viewport change always makes the retained frame stale",
    );
    assert!(page.doc.dom.render());
    assert_eq!(
        page.laid_out_rows(),
        admitted_rows(0.0, 200.0),
        "a taller viewport is a taller scrollport and a wider window",
    );
    assert!(
        page.laid_out_rows().len() > admitted_rows(0.0, VIEWPORT_HEIGHT).len(),
        "which admits strictly more rows",
    );

    page.doc.dom.set_viewport(200.0, VIEWPORT_HEIGHT);
    assert!(page.doc.dom.render());
    assert_eq!(
        page.laid_out_rows(),
        admitted_rows(0.0, VIEWPORT_HEIGHT),
        "and shrinking it back re-skips what left the window",
    );
}

#[test]
fn a_render_that_flips_nothing_builds_exactly_once() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    let commit = |page: &Page| {
        page.doc
            .dom
            .committed_frame()
            .expect("a render leaves a frame")
            .commit_id()
    };
    let first = commit(&page);

    // An idle document refuses to render at all.
    assert!(!page.doc.dom.render());
    assert_eq!(commit(&page), first);

    // A mutation that moves nothing into or out of the window still costs
    // exactly one commit: the determination confirms every bit and stops.
    page.doc
        .set_inline(page.rows[0], "background-color: rebeccapurple");
    assert!(page.doc.dom.render());
    assert_eq!(
        commit(&page),
        first + 1,
        "one commit id per render, however many passes it took",
    );
    assert_eq!(page.laid_out_rows(), admitted_rows(0.0, VIEWPORT_HEIGHT));
}

#[test]
fn a_document_with_no_auto_box_is_untouched_by_the_pass() {
    let mut doc = Doc::with_device(device(200.0, VIEWPORT_HEIGHT));
    doc.add_css(
        "page { display: flex; width: 200px; height: 100px; font-family: Ahem; }
         .plain { display: flex; width: 50px; height: 400px; }",
    );
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    let root = doc.root;
    let plain = doc.el(root, "view.plain");
    assert!(doc.dom.render());
    assert_eq!(
        rect(&doc.dom, plain),
        (0.0, 0.0, 50.0, 400.0),
        "nothing off screen is skipped without content-visibility",
    );
    assert!(!doc.dom.needs_render());
}

#[test]
fn a_skipped_row_whose_contents_mutate_stays_deferred_until_it_is_revealed() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    let skipped = ROWS - 1;
    assert!(page.doc.dom.text_block_size(page.labels[skipped]).is_none());

    page.doc.set_inline(page.labels[skipped], "font-size: 40px");
    assert!(page.doc.dom.render());
    assert!(
        page.doc.dom.text_block_size(page.labels[skipped]).is_none(),
        "a mutation under a skipped row changes nothing until the reveal",
    );
    assert_eq!(
        page.row_rect(skipped).3,
        ROW_HEIGHT,
        "the row keeps its estimate",
    );

    page.doc
        .set_inline(page.rows[skipped], "content-visibility: visible");
    assert!(page.doc.dom.render());
    assert_eq!(
        page.doc
            .dom
            .text_block_size(page.labels[skipped])
            .map(|size| size.height),
        Some(40.0),
        "and the deferred mutation applies at the reveal",
    );
}

#[test]
fn a_removed_row_takes_its_relevance_with_it() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    assert!(page.doc.dom.text_block_size(page.labels[0]).is_some());

    // Freeing the relevant row recycles its arena key. The next occupant
    // must start undetermined rather than inherit "relevant" from it.
    page.doc.dom.remove_element(page.rows[0]);
    page.doc.dom.drop_subtree(page.rows[0]);
    let fresh_row = page.doc.el(page.scroller, "view.row");
    let fresh_label = page.doc.el(fresh_row, "text.label");
    let run = page.doc.dom.create_text_node("q", ());
    page.doc.dom.append_child(fresh_label, run);
    assert!(page.doc.dom.render());

    // The new row is appended last, well past the window, so it must skip —
    // which it can only do if it did not inherit the freed row's bit.
    assert!(
        page.doc.dom.text_block_size(fresh_label).is_none(),
        "a recycled slot starts undetermined",
    );
}

/// Relevance is a geometric fact about the border box, not a paint-visibility
/// one. A `visibility: hidden` `auto` box emits no paint item at all, so a
/// determination keyed on the item list would leave it undetermined — and
/// therefore skipping — forever, taking its `visibility: visible` contents
/// down with it.
#[test]
fn an_invisible_auto_box_inside_the_window_is_still_determined_relevant() {
    let mut page = Page::new(
        ".invisible { visibility: hidden; }
         .shown { visibility: visible; }",
    );
    page.doc.add_class(page.rows[0], "invisible");
    page.doc.add_class(page.labels[0], "shown");

    assert!(page.doc.dom.render(), "the first render commits a frame");
    assert!(
        page.doc.dom.text_block_size(page.labels[0]).is_some(),
        "the invisible row was determined relevant, so its contents laid out",
    );
    assert_eq!(
        page.label_rect(0),
        (0.0, 0.0, ROW_HEIGHT, ROW_HEIGHT),
        "and the visible child has real geometry",
    );
    assert!(
        !page.doc.dom.needs_render(),
        "determined in this same commit, like any other auto box",
    );

    // And it reached the paint order: the visible child is hit where the
    // invisible row is, while the row itself — painting nothing — is not.
    let hits = page.doc.dom.elements_from_point(Point2D::new(10.0, 10.0));
    assert_eq!(
        hits.first().copied(),
        Some(page.labels[0]),
        "the visible child of an invisible auto row paints, got {hits:?}",
    );
    assert!(
        !hits.contains(&page.rows[0]),
        "while the invisible row itself paints nothing, got {hits:?}",
    );
}

/// The same box outside the window is skipped on exactly the geometry, with
/// `visibility` playing no part either way.
#[test]
fn an_invisible_auto_box_outside_the_window_still_skips() {
    let mut page = Page::new(
        ".invisible { visibility: hidden; }
         .shown { visibility: visible; }",
    );
    let far = ROWS - 1;
    page.doc.add_class(page.rows[far], "invisible");
    page.doc.add_class(page.labels[far], "shown");

    assert!(page.doc.dom.render());
    assert!(
        page.doc.dom.text_block_size(page.labels[far]).is_none(),
        "past the encode window, visibility changes nothing: it still skips",
    );
    assert_eq!(page.label_rect(far), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(
        page.row_rect(far),
        (0.0, ROW_HEIGHT * far as f32, 200.0, ROW_HEIGHT),
        "and keeps its own contain-intrinsic-size box",
    );
}
