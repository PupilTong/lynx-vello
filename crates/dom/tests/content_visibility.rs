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

use std::sync::{Arc, Mutex};

use common::{Doc, device};
use dom::event::{ElementEvent, ElementEventKind, EventPhase};
use dom::{CustomElement, Document, FontBlob, NodeId};
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

/// The tags [`Page::with_components`] builds its scroller and rows from.
///
/// Defined tags rather than `view`s, because `Document::define` requires a
/// definition to precede every element carrying its name and a page's root
/// is a `view` long before any case runs.
const SCROLLER_TAG: &str = "x-scroller";
const ROW_TAG: &str = "x-row";

impl Page {
    /// `extra` is appended to the stylesheet so a case can add its own rules.
    fn new(extra: &str) -> Self {
        Self::build(extra, "view", "view", |_| {})
    }

    /// The same page with its scroller and rows built from defined tags, so
    /// engine components are on the path of everything the commit decides.
    ///
    /// `define` runs against the document before the first element exists,
    /// which is the contract [`Document::define`] asserts for itself.
    fn with_components(extra: &str, define: impl FnOnce(&mut Document<()>)) -> Self {
        Self::build(extra, SCROLLER_TAG, ROW_TAG, define)
    }

    fn build(
        extra: &str,
        scroller_tag: &str,
        row_tag: &str,
        define: impl FnOnce(&mut Document<()>),
    ) -> Self {
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
        define(&mut doc.dom);
        let root = doc.root;
        let scroller = doc.el(root, &format!("{scroller_tag}#scroller.scroller"));
        let mut rows = Vec::with_capacity(ROWS);
        let mut labels = Vec::with_capacity(ROWS);
        for index in 0..ROWS {
            let row = doc.el(scroller, &format!("{row_tag}#row{index}.row"));
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

    /// The skipping changes the commits since the last call queued, as
    /// `(row index, skipped)` pairs in the order the document queued them.
    ///
    /// Every case that calls this has `auto` on its rows alone, so a change
    /// naming anything else is the drain reporting something it should not.
    fn changes(&mut self) -> Vec<(usize, bool)> {
        let queued = self.doc.dom.take_content_visibility_changes();
        queued
            .into_iter()
            .map(|change| {
                let row = self
                    .rows
                    .iter()
                    .position(|&row| row == change.node)
                    .expect("every queued change names one of this page's rows");
                (row, change.skipped)
            })
            .collect()
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

/// css-contain-2 §4.4: the commit that determines relevance queues one
/// `contentvisibilityautostatechange` per element whose *skipping* changed,
/// and the host drains them once.
///
/// The first determination is the case the spec's own
/// `content-visibility-auto-state-changed-first-observation.html` pins, and
/// it falls out of the three-state bit rather than needing a rule: an
/// undetermined box skips, so an on-screen box changes state (skipped -> not
/// skipped) and an off-screen box does not.
#[test]
fn the_first_commit_queues_a_change_for_the_rows_it_reveals_and_no_other() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render(), "the first render commits a frame");

    let revealed: Vec<(usize, bool)> = admitted_rows(0.0, VIEWPORT_HEIGHT)
        .into_iter()
        .map(|row| (row, false))
        .collect();
    assert_eq!(
        page.changes(),
        revealed,
        "only the rows whose skipping changed, in frame order",
    );
    assert!(
        page.changes().is_empty(),
        "and the drain is what empties the queue",
    );
}

/// A scroll past the encode window changes both ways at once, and the drain
/// reports each row once, in frame order — the order the paint build met
/// them, which is why a re-skipped row above the window comes before a
/// revealed one below it.
#[test]
fn a_scroll_queues_both_directions_once_each_in_frame_order() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    let before: Vec<usize> = page.changes().into_iter().map(|(row, _)| row).collect();
    assert_eq!(before, admitted_rows(0.0, VIEWPORT_HEIGHT));

    assert_eq!(page.scroll_to(150.0).y, 150.0);
    assert!(page.doc.dom.render(), "the scroll left a frame owed");

    let after = admitted_rows(150.0, VIEWPORT_HEIGHT);
    let expected: Vec<(usize, bool)> = (0..ROWS)
        .filter(|row| before.contains(row) != after.contains(row))
        .map(|row| (row, !after.contains(&row)))
        .collect();
    assert!(
        expected.iter().any(|&(_, skipped)| skipped)
            && expected.iter().any(|&(_, skipped)| !skipped),
        "the case is only worth anything if both directions happened: {expected:?}",
    );
    assert_eq!(page.changes(), expected);
}

/// A commit that determined nothing new queues nothing — including the
/// commit a mutation forces on a page whose rows all stay where they were.
#[test]
fn a_commit_with_no_skipping_change_queues_nothing() {
    let mut page = Page::new(".marked { opacity: 0.5; }");
    assert!(page.doc.dom.render());
    assert!(!page.changes().is_empty(), "the first commit revealed rows");

    // A visual mutation with no bearing on any border box: every row is
    // re-determined into the state it already had.
    page.doc.add_class(page.rows[0], "marked");
    assert!(page.doc.dom.render(), "the mutation commits a frame");
    assert!(
        page.changes().is_empty(),
        "re-determining an element into the state it already had is not a change",
    );

    // And an idle render commits nothing at all.
    assert!(!page.doc.dom.render());
    assert!(page.changes().is_empty());
}

/// A nested `auto` box is determined by a later pass of the very same
/// commit, and its change rides the same drain as its ancestor's — one
/// entry each, because a pass never re-determines what this commit already
/// did.
#[test]
fn a_nested_auto_box_queues_its_change_in_the_commit_that_revealed_it() {
    let mut page = Page::new(
        ".inner { display: flex; width: 200px; content-visibility: auto;
                  contain-intrinsic-size: 200px 20px; }",
    );
    let inner = page.doc.el(page.rows[0], "view.inner");
    let inner_label = page.doc.el(inner, "text.label");
    let run = page.doc.dom.create_text_node("y", ());
    page.doc.dom.append_child(inner_label, run);

    assert!(page.doc.dom.render());
    let queued = page.doc.dom.take_content_visibility_changes();
    assert_eq!(
        queued.iter().filter(|change| change.node == inner).count(),
        1,
        "the nested box changed once, in the commit that revealed its row: {queued:?}",
    );
    assert!(
        queued
            .iter()
            .all(|change| !change.skipped && change.node != page.rows[ROWS - 1]),
        "and nothing below the window changed state: {queued:?}",
    );
    assert!(page.doc.dom.take_content_visibility_changes().is_empty());
}

/// A row freed between the commit and the drain leaves its change queued,
/// naming an id that now resolves to nothing. That is the runtime's to drop
/// at dispatch — the same rule the retained frame's ids follow — rather than
/// something this queue prunes.
#[test]
fn a_freed_row_leaves_a_change_naming_an_id_that_resolves_to_nothing() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    let revealed = page.rows[0];
    page.doc.dom.remove_element(revealed);
    page.doc.dom.drop_subtree(revealed);

    let queued = page.doc.dom.take_content_visibility_changes();
    assert!(
        queued.iter().any(|change| change.node == revealed),
        "the change the commit made is still queued: {queued:?}",
    );
    assert!(
        page.doc.dom.get(revealed).is_none(),
        "and its id names nothing, which is how the runtime drops it",
    );
}

// ---------------------------------------------------------------------------
// css-contain-2 §4.4, delivered: `Document::dispatch_content_visibility_changes`
// ---------------------------------------------------------------------------
//
// The event is fired **Rust-side only**. Its listeners are the engine's own
// components — a defined `CustomElement`, which is what `<list>` will be —
// reached through `Document::dispatch_element_event`, and nothing about it
// touches a script realm. What the cases below pin is the walk: the standard's
// two passes, one delivery per node, `stopPropagation`, and what a target
// freed or a tree mutated in the middle of one does.

/// What every component here writes its deliveries into.
type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.lock().expect("the log is never poisoned"))
}

type Action = Box<dyn Fn(&mut Document<()>, NodeId, &mut ElementEvent) + Send + Sync>;

/// An engine component that records every event it hears as
/// `who:phase:target:state`, naming the target by its `id` attribute.
///
/// `on_event` is how a case makes a handler *do* something — stop the walk,
/// mutate the tree — from inside the delivery.
struct Recorder {
    who: &'static str,
    log: Log,
    connections: bool,
    on_event: Option<Action>,
}

impl Recorder {
    fn new(who: &'static str, log: &Log) -> Self {
        Self {
            who,
            log: Arc::clone(log),
            connections: false,
            on_event: None,
        }
    }

    /// Also records `connected_callback`, which is what pins where the
    /// reaction a handler's own mutation raised runs.
    fn logging_connections(mut self) -> Self {
        self.connections = true;
        self
    }

    fn on_event(mut self, action: Action) -> Self {
        self.on_event = Some(action);
        self
    }

    fn record(&self, entry: String) {
        self.log
            .lock()
            .expect("the log is never poisoned")
            .push(entry);
    }
}

impl CustomElement<()> for Recorder {
    fn connected_callback(&self, _document: &mut Document<()>, _element: NodeId) {
        if self.connections {
            self.record(format!("{}:connected", self.who));
        }
    }

    fn handle_event(&self, document: &mut Document<()>, element: NodeId, event: &mut ElementEvent) {
        assert_eq!(
            event.current_target(),
            element,
            "the element a hook is called about is the event's currentTarget",
        );
        assert_eq!(
            event.kind().name(),
            "contentvisibilityautostatechange",
            "the only kind this suite fires",
        );
        let ElementEventKind::ContentVisibilityAutoStateChange { skipped } = event.kind() else {
            panic!("an engine event this suite never fires: {:?}", event.kind())
        };
        let target = document
            .get(event.target())
            .and_then(|node| node.attribute("id"))
            .expect("every target here is a live element with an id")
            .to_owned();
        let phase = match event.phase() {
            EventPhase::Capturing => "capture",
            EventPhase::AtTarget => "at-target",
            EventPhase::Bubbling => "bubble",
        };
        let state = if skipped { "skipped" } else { "shown" };
        self.record(format!("{}:{phase}:{target}:{state}", self.who));
        if let Some(action) = &self.on_event {
            action(document, element, event);
        }
    }
}

/// A page whose scroller and rows both record, with `row` given `action`.
fn recording_page(log: &Log, action: Option<Action>) -> Page {
    let scroller = Recorder::new("scroller", log);
    let mut row = Recorder::new("row", log);
    if let Some(action) = action {
        row = row.on_event(action);
    }
    let page = Page::with_components("", move |document| {
        document.define(SCROLLER_TAG, Box::new(scroller));
        document.define(ROW_TAG, Box::new(row));
    });
    assert!(take(log).is_empty(), "building the page fires no event");
    page
}

/// The three deliveries one row's change owes, in order: the scroller hears
/// it inbound, the row itself hears it at-target, and the scroller hears it
/// again outbound.
fn whole_path(row: usize, skipped: bool) -> Vec<String> {
    let state = if skipped { "skipped" } else { "shown" };
    vec![
        format!("scroller:capture:row{row}:{state}"),
        format!("row:at-target:row{row}:{state}"),
        format!("scroller:bubble:row{row}:{state}"),
    ]
}

/// The first commit's changes reach the components on the path, in the
/// standard's order: capture root-inward, the target once, bubble outward.
///
/// One delivery at the target, not two — the standard visits it in both
/// passes only because each pass runs a different registration set, and a
/// component has one `handle_event`.
#[test]
fn every_revealed_row_reaches_its_own_component_and_the_scroller_above_it() {
    let log = log();
    let mut page = recording_page(&log, None);
    assert!(page.doc.dom.render(), "the first render commits a frame");
    assert!(
        take(&log).is_empty(),
        "the commit decides the changes and delivers nothing: the dispatch is the host's call",
    );

    assert!(page.doc.dom.has_pending_content_visibility_changes());
    page.doc.dom.dispatch_content_visibility_changes();
    assert!(
        !page.doc.dom.has_pending_content_visibility_changes(),
        "the dispatch is what empties the queue",
    );

    let expected: Vec<String> = admitted_rows(0.0, VIEWPORT_HEIGHT)
        .into_iter()
        .flat_map(|row| whole_path(row, false))
        .collect();
    assert_eq!(
        take(&log),
        expected,
        "every revealed row, in frame order, each over the whole path",
    );
    assert_eq!(
        (
            EventPhase::Capturing.value(),
            EventPhase::AtTarget.value(),
            EventPhase::Bubbling.value(),
        ),
        (1, 2, 3),
        "the phases report the DOM constants",
    );

    page.doc.dom.dispatch_content_visibility_changes();
    assert!(
        take(&log).is_empty(),
        "and a second dispatch has nothing left to deliver",
    );
}

/// A scroll past the encode window changes rows in both directions, and each
/// one is delivered once, in frame order — a re-skipped row above the window
/// before a revealed one below it.
#[test]
fn a_scroll_delivers_both_directions_once_each_in_frame_order() {
    let log = log();
    let mut page = recording_page(&log, None);
    assert!(page.doc.dom.render());
    page.doc.dom.dispatch_content_visibility_changes();
    let before = admitted_rows(0.0, VIEWPORT_HEIGHT);
    assert_eq!(take(&log).len(), before.len() * 3);

    assert_eq!(page.scroll_to(150.0).y, 150.0);
    assert!(page.doc.dom.render(), "the scroll left a frame owed");
    page.doc.dom.dispatch_content_visibility_changes();

    let after = admitted_rows(150.0, VIEWPORT_HEIGHT);
    let expected: Vec<String> = (0..ROWS)
        .filter(|row| before.contains(row) != after.contains(row))
        .flat_map(|row| whole_path(row, !after.contains(&row)))
        .collect();
    assert!(
        expected.iter().any(|entry| entry.ends_with("skipped"))
            && expected.iter().any(|entry| entry.ends_with("shown")),
        "the case is only worth anything if both directions happened: {expected:?}",
    );
    assert_eq!(take(&log), expected);
}

/// A row that stops propagation is the last node the event reaches: the
/// scroller's bubble delivery never happens, while the capture delivery that
/// already ran stands.
#[test]
fn a_row_that_stops_propagation_keeps_the_scroller_from_hearing_the_bubble() {
    let log = log();
    let mut page = recording_page(
        &log,
        Some(Box::new(|_document, _element, event| {
            event.stop_propagation();
            assert!(event.propagation_stopped());
        })),
    );
    assert!(page.doc.dom.render());
    page.doc.dom.dispatch_content_visibility_changes();

    let expected: Vec<String> = admitted_rows(0.0, VIEWPORT_HEIGHT)
        .into_iter()
        .flat_map(|row| {
            vec![
                format!("scroller:capture:row{row}:shown"),
                format!("row:at-target:row{row}:shown"),
            ]
        })
        .collect();
    assert_eq!(take(&log), expected);
}

/// A row freed between the commit that queued its change and the dispatch
/// delivers nothing at all — not to itself, and not to the scroller above
/// it, because a dead target resolves to no path.
#[test]
fn a_row_freed_before_the_dispatch_delivers_nothing() {
    let log = log();
    let mut page = recording_page(&log, None);
    assert!(page.doc.dom.render());
    let freed = page.rows[0];
    page.doc.dom.remove_element(freed);
    page.doc.dom.drop_subtree(freed);

    page.doc.dom.dispatch_content_visibility_changes();
    let delivered = take(&log);
    assert!(
        !delivered.iter().any(|entry| entry.contains(":row0:")),
        "nothing was delivered for the freed row: {delivered:?}",
    );
    assert_eq!(
        delivered,
        admitted_rows(0.0, VIEWPORT_HEIGHT)
            .into_iter()
            .filter(|&row| row != 0)
            .flat_map(|row| whole_path(row, false))
            .collect::<Vec<_>>(),
        "and every other row's change was delivered as usual",
    );
}

/// A document that defines nothing pays the queue drain and the one check
/// that says nobody can listen — no path is built and no handler exists to
/// call.
#[test]
fn a_document_with_no_definitions_drains_the_queue_and_does_nothing_else() {
    let mut page = Page::new("");
    assert!(page.doc.dom.render());
    assert!(
        page.doc.dom.has_pending_content_visibility_changes(),
        "the commit queued the rows it revealed",
    );

    page.doc.dom.dispatch_content_visibility_changes();
    assert!(
        !page.doc.dom.has_pending_content_visibility_changes(),
        "the queue is the record of one commit, so it is drained whether or not anyone listens",
    );
    assert!(page.doc.dom.take_content_visibility_changes().is_empty());
}

/// A handler may mutate the tree it is being called about: the write lands in
/// the document, the rest of the walk runs, and the next render commits it.
///
/// The `connected_callback` of a child the handler appends runs **before the
/// next step** — the mutation that raised it drained it, as every mutation in
/// this crate does, and the dispatch opens a scope of its own around each
/// handler so nothing a handler raised can outlive its step.
#[test]
fn a_handler_that_mutates_the_tree_is_drained_before_the_next_step() {
    let log = log();
    let item = Recorder::new("item", &log).logging_connections();
    let scroller = Recorder::new("scroller", &log);
    let row = Recorder::new("row", &log).on_event(Box::new(|document, element, event| {
        // The first row only, so what the log shows is one step's worth.
        if document.get(element).and_then(|node| node.attribute("id")) != Some("row0") {
            return;
        }
        document.set_inline_style(element, "width: 120px");
        let child = document.create_element("x-item", ());
        document.append_child(event.current_target(), child);
    }));
    let mut page = Page::with_components("", move |document| {
        document.define(SCROLLER_TAG, Box::new(scroller));
        document.define(ROW_TAG, Box::new(row));
        document.define("x-item", Box::new(item));
    });
    assert!(page.doc.dom.render());
    assert!(take(&log).is_empty());

    page.doc.dom.dispatch_content_visibility_changes();
    let delivered = take(&log);
    assert_eq!(
        delivered[..4],
        [
            "scroller:capture:row0:shown".to_owned(),
            "row:at-target:row0:shown".to_owned(),
            "item:connected".to_owned(),
            "scroller:bubble:row0:shown".to_owned(),
        ],
        "the child the handler appended was connected before the walk moved on: {delivered:?}",
    );
    assert_eq!(
        delivered.len(),
        admitted_rows(0.0, VIEWPORT_HEIGHT).len() * 3 + 1,
        "and every other row was delivered as usual: {delivered:?}",
    );

    assert!(
        page.doc.dom.render(),
        "the handler's write left a frame owed",
    );
    assert_eq!(
        page.row_rect(0),
        (0.0, 0.0, 120.0, ROW_HEIGHT),
        "which the next commit published",
    );
    assert!(
        !page.doc.dom.has_pending_content_visibility_changes(),
        "and nothing about that commit changed a skipping state",
    );
}

// The last remembered size
// ([css-sizing-4 §5.2.1](https://drafts.csswg.org/css-sizing-4/#last-remembered)),
// plus csswg-drafts#8407: `content-visibility: auto` makes `contain-intrinsic-*`
// behave as if `auto` were specified.
//
// The recording moment here is the commit's own layout run — this engine has no
// ResizeObserver — so every case below goes through `render()`, and every
// assertion is on a box's used height: the estimate before anything is
// remembered, the real content box afterwards.
// ---------------------------------------------------------------------------

/// The real content height of a `.target`/`.box`, well past its estimate.
const TALL: f32 = 50.0;
/// What `contain-intrinsic-size` carries before anything is remembered.
const ESTIMATE: f32 = 10.0;
/// A fixed block above the target, tall enough that the target starts outside
/// the encode window and one scroll to the end is enough to reach it.
const SPACER: f32 = 400.0;

/// One `content-visibility: auto` target at the bottom of a scroller, so the
/// skip/reveal/skip cycle is driven by scrolling rather than by style.
struct Remembered {
    doc: Doc,
    scroller: NodeId,
    target: NodeId,
    label: NodeId,
}

impl Remembered {
    /// `containment` is the target's own `contain-intrinsic-*` declarations.
    fn new(containment: &str) -> Self {
        let mut doc = Doc::with_device(device(200.0, VIEWPORT_HEIGHT));
        doc.add_css(&format!(
            "page {{ display: flex; width: 200px; height: 100vh;
                     align-items: flex-start; font-family: Ahem; }}
             .scroller {{ display: flex; flex-direction: column; overflow: hidden;
                          width: 200px; height: 100vh; align-items: flex-start; }}
             .spacer {{ display: flex; width: 200px; height: {SPACER}px;
                        flex-shrink: 0; }}
             .target {{ display: flex; width: 200px; flex-shrink: 0;
                        content-visibility: auto; {containment} }}
             .tall {{ display: -lynx-text; font-size: {TALL}px; }}"
        ));
        assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = doc.root;
        let scroller = doc.el(root, "view.scroller");
        doc.el(scroller, "view.spacer");
        let target = doc.el(scroller, "view.target");
        let label = doc.el(target, "text.tall");
        let run = doc.dom.create_text_node("x", ());
        doc.dom.append_child(label, run);
        Self {
            doc,
            scroller,
            target,
            label,
        }
    }

    fn target_height(&self) -> f32 {
        rect(&self.doc.dom, self.target).3
    }

    fn scroll_range(&self) -> f32 {
        self.doc
            .dom
            .rounded_layout(self.scroller)
            .expect("node id is live")
            .content_size
            .height
    }

    fn skipping(&self) -> bool {
        self.doc.dom.text_block_size(self.label).is_none()
    }

    fn scroll_to(&mut self, offset: f32) -> f32 {
        self.doc
            .dom
            .scroll_to(self.scroller, Vector2D::new(0.0, offset))
            .y
    }
}

/// The whole cycle: skip at the estimate, reveal and record, skip again at
/// what was recorded.
#[test]
fn a_revealed_box_is_remembered_and_re_used_the_next_time_it_skips() {
    let mut page =
        Remembered::new("contain-intrinsic-width: 200px; contain-intrinsic-height: auto 10px;");
    assert!(page.doc.dom.render());
    assert!(
        page.skipping(),
        "the target starts well outside the encode window",
    );
    assert_eq!(
        page.target_height(),
        ESTIMATE,
        "with nothing remembered, `auto <length>` is the <length>",
    );
    assert_eq!(page.scroll_range(), SPACER + ESTIMATE);

    // Scrolling to the end brings it inside the window: it is revealed and laid
    // out from its real contents, which is the moment its inner size is
    // recorded.
    assert_eq!(page.scroll_to(1000.0), SPACER + ESTIMATE - VIEWPORT_HEIGHT);
    assert!(page.doc.dom.render());
    assert!(!page.skipping(), "the scroll revealed it");
    assert_eq!(page.target_height(), TALL, "at its real content height");
    assert_eq!(page.scroll_range(), SPACER + TALL);

    // And back out: it skips again, now from what it last rendered at.
    assert_eq!(page.scroll_to(0.0), 0.0);
    assert!(page.doc.dom.render());
    assert!(page.skipping(), "outside the window again");
    assert_eq!(
        page.target_height(),
        TALL,
        "the last remembered size, not the 10px estimate",
    );
    assert_eq!(
        page.scroll_range(),
        SPACER + TALL,
        "so the scroll range stays where the reveal put it",
    );
}

/// csswg-drafts#8407: the same cycle for a box whose `contain-intrinsic-size`
/// carries no `auto` keyword at all. `content-visibility: auto` supplies it.
#[test]
fn content_visibility_auto_makes_a_plain_length_behave_as_auto() {
    let mut page =
        Remembered::new("contain-intrinsic-width: 200px; contain-intrinsic-height: 10px;");
    assert!(page.doc.dom.render());
    assert!(page.skipping());
    assert_eq!(page.target_height(), ESTIMATE);

    assert_eq!(page.scroll_to(1000.0), SPACER + ESTIMATE - VIEWPORT_HEIGHT);
    assert!(page.doc.dom.render());
    assert_eq!(page.target_height(), TALL);

    assert_eq!(page.scroll_to(0.0), 0.0);
    assert!(page.doc.dom.render());
    assert!(page.skipping());
    assert_eq!(
        page.target_height(),
        TALL,
        "#8407: `10px` behaved as `auto 10px`, so the box remembered its size",
    );
}

/// A plain document with no scroller: `content-visibility` is set by inline
/// style, so these cases are about the remembered size itself rather than about
/// relevance.
fn flat_doc() -> Doc {
    let mut doc = Doc::with_device(device(200.0, 400.0));
    doc.add_css(&format!(
        "page {{ display: flex; flex-direction: column; align-items: flex-start;
                 width: 200px; height: 400px; font-family: Ahem; }}
         .box {{ display: flex; width: 200px; flex-shrink: 0;
                 contain-intrinsic-width: 200px;
                 contain-intrinsic-height: auto {ESTIMATE}px; }}
         .tall {{ display: -lynx-text; font-size: {TALL}px; }}"
    ));
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    doc
}

/// Appends a `.box` whose real content is [`TALL`] high, with `inline` as its
/// inline style.
fn tall_box(doc: &mut Doc, inline: &str) -> (NodeId, NodeId) {
    let root = doc.root;
    let boxed = doc.el(root, "view.box");
    if !inline.is_empty() {
        doc.set_inline(boxed, inline);
    }
    let label = doc.el(boxed, "text.tall");
    let run = doc.dom.create_text_node("x", ());
    doc.dom.append_child(label, run);
    (boxed, label)
}

fn height(doc: &Doc, id: NodeId) -> f32 {
    rect(&doc.dom, id).3
}

fn width(doc: &Doc, id: NodeId) -> f32 {
    rect(&doc.dom, id).2
}

/// `content-visibility: hidden` reads the remembered size exactly as `auto`
/// does — and a box no rendering update ever laid out has none to read.
#[test]
fn a_box_that_was_rendered_keeps_its_size_when_it_starts_skipping() {
    let mut doc = flat_doc();
    let (rendered, rendered_label) = tall_box(&mut doc, "");
    let (never, never_label) = tall_box(&mut doc, "content-visibility: hidden");

    assert!(doc.dom.render());
    assert_eq!(height(&doc, rendered), TALL, "laid out from its contents");
    assert!(doc.dom.text_block_size(rendered_label).is_some());
    assert_eq!(
        height(&doc, never),
        ESTIMATE,
        "a box that never rendered has nothing to remember",
    );
    assert!(doc.dom.text_block_size(never_label).is_none());

    doc.set_inline(rendered, "content-visibility: hidden");
    assert!(doc.dom.render(), "a content-visibility change relayouts");
    assert!(
        doc.dom.text_block_size(rendered_label).is_none(),
        "it skips its contents now",
    );
    assert_eq!(
        height(&doc, rendered),
        TALL,
        "at the size it last rendered at, not at its 10px estimate",
    );
    assert_eq!(
        height(&doc, never),
        ESTIMATE,
        "while the box beside it still has nothing remembered",
    );
}

/// `auto none` is "no explicit intrinsic inner size" until there is a
/// remembered one, and the remembered one once there is.
#[test]
fn auto_none_lays_out_as_if_empty_until_something_is_remembered() {
    let mut doc = flat_doc();
    let (empty, _) = tall_box(
        &mut doc,
        "contain-intrinsic-height: auto none; content-visibility: hidden",
    );
    let (measured, _) = tall_box(&mut doc, "contain-intrinsic-height: auto none");

    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, empty),
        0.0,
        "`auto none` with nothing remembered is no explicit size at all",
    );
    assert_eq!(height(&doc, measured), TALL);

    doc.set_inline(
        measured,
        "contain-intrinsic-height: auto none; content-visibility: hidden",
    );
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, measured),
        TALL,
        "and the remembered size once there is one",
    );
    assert_eq!(height(&doc, empty), 0.0, "the other still has none");
}

/// The `auto` clause is conditioned on "is currently skipping its contents":
/// size containment on its own never reads a remembered size.
#[test]
fn a_size_contained_box_that_is_not_skipping_uses_the_length() {
    let mut doc = flat_doc();
    let (boxed, label) = tall_box(&mut doc, "");
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, boxed),
        TALL,
        "recorded while it had no size containment",
    );

    doc.set_inline(boxed, "contain: size");
    assert!(doc.dom.render(), "a contain change relayouts");
    assert!(
        doc.dom.text_block_size(label).is_some(),
        "`contain: size` still lays its contents out; it just does not size from them",
    );
    assert_eq!(
        height(&doc, boxed),
        ESTIMATE,
        "and it is not skipping, so the <length> stands",
    );

    // Nor did that pass overwrite what the box remembers: a size-contained box
    // was laid out as if it had no contents, so its inner size is the estimate
    // rather than anything its contents produced. Skipping proves it — the
    // real measurement is still there.
    doc.set_inline(boxed, "contain: size; content-visibility: hidden");
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, boxed),
        TALL,
        "the measurement from before size containment, not the estimate",
    );
}

/// Containment is per axis, and so is the recording rule it gates: under
/// `contain: inline-size` the height this run produced *is* the contents'
/// own and is recorded, while the width — the substituted estimate — leaves
/// what the box last measured alone.
#[test]
fn inline_size_containment_only_stops_the_width_from_being_recorded() {
    let mut doc = flat_doc();
    let content_sized = "width: auto; contain-intrinsic-width: auto 10px;";
    let (boxed, _) = tall_box(&mut doc, content_sized);
    assert!(doc.dom.render());
    assert_eq!(
        (width(&doc, boxed), height(&doc, boxed)),
        (TALL, TALL),
        "both axes come from the one Ahem glyph, and both are recorded",
    );

    doc.set_inline(boxed, &format!("{content_sized} contain: inline-size"));
    assert!(doc.dom.render(), "a contain change relayouts");
    assert_eq!(
        (width(&doc, boxed), height(&doc, boxed)),
        (ESTIMATE, TALL),
        "the width is the estimate; the height is still the contents'",
    );

    doc.set_inline(
        boxed,
        &format!("{content_sized} contain: inline-size; content-visibility: hidden"),
    );
    assert!(doc.dom.render());
    assert_eq!(
        (width(&doc, boxed), height(&doc, boxed)),
        (TALL, TALL),
        "and the run above recorded neither the estimate over the width it \
         had measured, nor anything but the contents for the height",
    );
}

/// "if an element has a last remembered size but does not have auto keyword in
/// contain-intrinsic-size property, remove its last remembered size."
#[test]
fn removing_the_auto_keyword_removes_the_remembered_size() {
    let mut doc = flat_doc();
    let (dropped, _) = tall_box(&mut doc, "");
    let (kept, _) = tall_box(&mut doc, "");
    assert!(doc.dom.render());
    assert_eq!(height(&doc, dropped), TALL);
    assert_eq!(height(&doc, kept), TALL);

    // Stylo gives a `contain-intrinsic-size` change box-rebuilding damage, so
    // the box reaches a committing run and the removal happens in it.
    doc.set_inline(dropped, "contain-intrinsic-height: 10px");
    assert!(
        doc.dom.render(),
        "dropping the auto keyword relayouts the box",
    );
    assert_eq!(
        height(&doc, dropped),
        TALL,
        "still rendered, so still sized by its contents",
    );

    doc.set_inline(
        dropped,
        "contain-intrinsic-height: auto 10px; content-visibility: hidden",
    );
    doc.set_inline(kept, "content-visibility: hidden");
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, dropped),
        ESTIMATE,
        "re-adding auto starts from the <length> again",
    );
    assert_eq!(
        height(&doc, kept),
        TALL,
        "while the box that never lost auto still has what it remembered",
    );
}

/// The last remembered size is state attached to the *element*, so a freed
/// arena key must not hand it to the key's next occupant.
#[test]
fn a_freed_node_takes_its_remembered_size_with_it() {
    let mut doc = flat_doc();
    // One node, so the key this frees is the key the next element takes: the
    // slab hands back the most recently freed key first.
    let root = doc.root;
    let recorder = doc.el(root, "view.box");
    doc.set_inline(recorder, "height: 50px");
    assert!(doc.dom.render());
    assert_eq!(height(&doc, recorder), TALL, "its recorded inner height");

    doc.dom.remove_element(recorder);
    doc.dom.drop_subtree(recorder);

    let recycled = doc.el(root, "view.box");
    doc.set_inline(recycled, "content-visibility: hidden");
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, recycled),
        ESTIMATE,
        "a recycled slot remembers nothing",
    );
}

/// The removal rule has no "is rendered" condition, so it has to reach a box
/// that is already skipping — where the run that serves its own size is the
/// only run it gets.
#[test]
fn a_box_that_loses_the_auto_keyword_while_it_skips_forgets_too() {
    let mut doc = flat_doc();
    let (boxed, _) = tall_box(&mut doc, "");
    assert!(doc.dom.render());
    assert_eq!(height(&doc, boxed), TALL);

    doc.set_inline(boxed, "content-visibility: hidden");
    assert!(doc.dom.render());
    assert_eq!(height(&doc, boxed), TALL, "skipping at what it remembered");

    doc.set_inline(
        boxed,
        "content-visibility: hidden; contain-intrinsic-height: 10px",
    );
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, boxed),
        ESTIMATE,
        "no auto keyword, so the length"
    );

    doc.set_inline(
        boxed,
        "content-visibility: hidden; contain-intrinsic-height: auto 10px",
    );
    assert!(doc.dom.render());
    assert_eq!(
        height(&doc, boxed),
        ESTIMATE,
        "and the earlier removal means re-adding auto starts from the length",
    );
}
