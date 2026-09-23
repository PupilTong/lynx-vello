//! Compose-versus-commit oracle for exported curves.
//!
//! A frame committed at one instant and composed at a later one must equal
//! the frame the main thread commits at that later instant: every item and
//! clip at the same place, every animated effect layer at the same alpha,
//! and every hit test answering the same element. `early` is committed with
//! the curve running; for each later `t` the document is advanced and
//! committed again as `late`, and `early` sampled at `Some(t)` is compared
//! with `late` sampled at `None` — the reference involves no curve at all.

use euclid::default::Vector2D;

use crate::paint::convert::item_affine;
use crate::test_common::Doc;
use crate::vello::kurbo::{Affine, Point};
use crate::visual::{CommittedFrame, ScrollSlot};
use crate::{FontBlob, NodeId, Point2D};

const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

/// The Lynx UA rules the fixtures depend on: every box clips (`<text>`
/// included, as `overflow: clip`) except `page` and `view`, which the default
/// page config resets to `visible`.
const UA: &str = "page, view, text { display: flex; box-sizing: border-box;
                                     position: relative; overflow: clip;
                                     min-width: 0; min-height: 0; }
                  page, view { overflow: visible; }
                  text { display: -lynx-text; }";

const KEYFRAMES: &str = "
    @keyframes translate { from { transform: translate(0px, 0px); }
                           to { transform: translate(120px, 40px); } }
    @keyframes rotate { from { transform: rotate(0deg); } to { transform: rotate(90deg); } }
    @keyframes scale { from { transform: scale(1); } to { transform: scale(1.5); } }
    @keyframes opacity { from { opacity: 1; } to { opacity: 0.2; } }";

const CURVES: [&str; 4] = ["translate", "rotate", "scale", "opacity"];

/// Where the timeline stands when `early` is committed, and the later
/// instants it is composed at.
const EARLY: f64 = 0.1;
const LATER: [f64; 2] = [0.35, 0.8];

/// Composed positions agree to this many CSS px.
const TOLERANCE: f64 = 1e-3;

/// One fixture: its document, the scroll offsets both frames compose at, the
/// number of curves it exports, and elements the hit sweep must reach.
struct Fixture {
    doc: Doc,
    offsets: Vec<(NodeId, Vector2D<f32>)>,
    exported: usize,
    probes: Vec<NodeId>,
}

impl Fixture {
    fn new(css: &str) -> Self {
        let mut doc = Doc::new();
        doc.add_ua_css(UA);
        doc.add_css(&format!(
            "page {{ width: 800px; height: 600px; flex-direction: column; }} {KEYFRAMES} {css}"
        ));
        doc.dom.register_fonts(FontBlob::from_static(AHEM));
        Self {
            doc,
            offsets: Vec::new(),
            exported: 1,
            probes: Vec::new(),
        }
    }

    fn el(&mut self, parent: NodeId, spec: &str) -> NodeId {
        self.doc.el(parent, spec)
    }

    fn animate(&mut self, id: NodeId, curve: &str) {
        self.doc
            .set_inline(id, &format!("animation: {curve} 1s linear infinite"));
    }

    fn text(&mut self, parent: NodeId, text: &str) {
        let node = self.doc.dom.create_text_node(text, ());
        self.doc.dom.append_child(parent, node);
    }

    fn offset_of(&self) -> impl Fn(&ScrollSlot) -> Option<Vector2D<f32>> + '_ {
        |slot| {
            self.offsets
                .iter()
                .find(|(node, _)| *node == slot.node)
                .map(|(_, offset)| *offset)
        }
    }

    /// Commits `early` with every curve running, then checks it against a
    /// fresh commit at each of [`LATER`].
    fn check(mut self, label: &str) {
        let dom = &mut self.doc.dom;
        dom.render();
        dom.advance_animations(0.0);
        assert!(
            dom.advance_animations(EARLY).needs_next_frame,
            "{label}: the fixture animates"
        );
        dom.render();
        let early = dom.committed_frame().expect("a frame is committed");
        assert_eq!(
            early.animation_slots().len(),
            self.exported,
            "{label}: every animated element exports",
        );
        assert!(
            !early.needs_main_ticks(),
            "{label}: nothing is left to main-thread ticks"
        );
        for t in LATER {
            self.doc.dom.advance_animations(t);
            self.doc.dom.render();
            let late = self
                .doc
                .dom
                .committed_frame()
                .expect("a frame is committed");
            let label = format!("{label} at t = {t}");
            self.compare_geometry(&early, &late, t, &label);
            self.compare_hits(&early, &late, t, &label);
        }
    }

    /// Every paired item's and clip's composed box, and every exported
    /// opacity, agree.
    fn compare_geometry(&self, early: &CommittedFrame, late: &CommittedFrame, t: f64, label: &str) {
        let offset_of = self.offset_of();
        let early_animations = early.order.sample_animations(Some(t));
        let early_stickies = early.order.sample_stickies(1.0, &offset_of);
        let early_samples =
            early
                .order
                .space_samples(&early_animations, &early_stickies, 1.0, &offset_of);
        let late_animations = late.order.sample_animations(None);
        let late_stickies = late.order.sample_stickies(1.0, &offset_of);
        let late_samples =
            late.order
                .space_samples(&late_animations, &late_stickies, 1.0, &offset_of);

        let (early_items, late_items) = (early.order.items(), late.order.items());
        assert_eq!(early_items.len(), late_items.len(), "{label}: item count");
        for (a, b) in early_items.iter().zip(late_items) {
            assert_eq!((a.node, a.kind), (b.node, b.kind), "{label}: paint order");
            let (Some(a_local), Some(b_local)) = (
                item_affine(&a.transform, a.size),
                item_affine(&b.transform, b.size),
            ) else {
                continue;
            };
            let box_ = [0.0, 0.0, f64::from(a.size.width), f64::from(a.size.height)];
            assert_same_box(
                early_samples.css(a.space) * a_local,
                late_samples.css(b.space) * b_local,
                box_,
                &format!("{label}: item {:?} {:?}", a.node, a.kind),
            );
        }

        let (early_clips, late_clips) = (early.order.clips(), late.order.clips());
        assert_eq!(early_clips.len(), late_clips.len(), "{label}: clip count");
        for (a, b) in early_clips.iter().zip(late_clips) {
            assert_eq!(a.node, b.node, "{label}: clip order");
            let (Some(a_local), Some(b_local)) = (
                item_affine(&a.transform, a.rect.size),
                item_affine(&b.transform, b.rect.size),
            ) else {
                continue;
            };
            let rect = a.rect;
            let box_ = [
                f64::from(rect.origin.x),
                f64::from(rect.origin.y),
                f64::from(rect.origin.x + rect.size.width),
                f64::from(rect.origin.y + rect.size.height),
            ];
            assert_same_box(
                early_samples.css(a.space) * a_local,
                late_samples.css(b.space) * b_local,
                box_,
                &format!("{label}: clip of {:?}", a.node),
            );
        }

        for (index, slot) in (0_u32..).zip(early.animation_slots()) {
            let Some(alpha) = early_animations.get(index).alpha else {
                continue;
            };
            let committed = self
                .doc
                .dom
                .paint_style(slot.node)
                .expect("an animated element is styled")
                .get_effects()
                .opacity;
            assert!(
                (alpha - committed).abs() < 1e-4,
                "{label}: sampled opacity {alpha}, committed {committed}",
            );
        }
    }

    /// `early` hit at `Some(t)` answers what `late` answers at `None` at
    /// every grid point clear of the edges `late` draws.
    fn compare_hits(&self, early: &CommittedFrame, late: &CommittedFrame, t: f64, label: &str) {
        let offset_of = self.offset_of();
        let late_hit = |x: f32, y: f32| late.hit(Point2D::new(x, y), &offset_of, None);
        let mut reached = Vec::new();
        let mut compared = 0_usize;
        for row in 0_u16..70 {
            for column in 0_u16..70 {
                let (x, y) = (f32::from(column) * 7.0 + 0.25, f32::from(row) * 7.0 + 0.25);
                let expected = late_hit(x, y);
                // Four neighbors half a pixel away answering alike put the
                // point at least 0.35 px from any straight edge.
                let stable = [(0.5, 0.0), (-0.5, 0.0), (0.0, 0.5), (0.0, -0.5)]
                    .into_iter()
                    .all(|(dx, dy)| late_hit(x + dx, y + dy) == expected);
                if !stable {
                    continue;
                }
                let got = early.hit(Point2D::new(x, y), &offset_of, Some(t));
                assert_eq!(got, expected, "{label}: hit at ({x}, {y})");
                compared += 1;
                if let Some(target) = got
                    && !reached.contains(&target.node)
                {
                    reached.push(target.node);
                }
            }
        }
        assert!(
            compared > 1000,
            "{label}: the sweep compared {compared} points"
        );
        for probe in &self.probes {
            assert!(
                reached.contains(probe),
                "{label}: the sweep never reached {probe:?}"
            );
        }
    }
}

/// `a` and `b` put `box_`'s corners within [`TOLERANCE`] of each other.
fn assert_same_box(a: Affine, b: Affine, [x0, y0, x1, y1]: [f64; 4], label: &str) {
    for corner in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        let corner = Point::new(corner.0, corner.1);
        let distance = (a * corner - b * corner).hypot();
        assert!(
            distance < TOLERANCE,
            "{label}: corner {corner:?} composes {:?}, committed {:?}",
            a * corner,
            b * corner,
        );
    }
}

/// A card holding a `<text>` whose run overflows it: the text's UA
/// `overflow: clip` clips the run inside the moving card.
#[test]
fn a_card_with_a_text_child_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".card { width: 220px; height: 120px; margin: 60px 0 0 80px; padding: 20px;
                     background-color: teal; }
             .label { width: 90px; height: 20px; font-family: Ahem; font-size: 20px; }",
        );
        let root = fixture.doc.root;
        let card = fixture.el(root, "view.card");
        let label = fixture.el(card, "text.label");
        fixture.text(label, "hellohello");
        fixture.animate(card, curve);
        fixture.probes = vec![card, label];
        fixture.check(&format!("text card, {curve}"));
    }
}

/// An `overflow: hidden` rounded card — a programmatic scroll container
/// under the Lynx grammar — scrolled by an offset override, with content
/// overflowing it and a box over its rounded corner.
#[test]
fn an_overflow_hidden_rounded_card_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".card { width: 200px; height: 140px; margin: 80px 0 0 120px; overflow: hidden;
                     border-radius: 36px; background-color: teal; }
             .wide { width: 300px; height: 70px; flex-shrink: 0; background-color: orange; }
             .corner { position: absolute; left: 0; top: 0; width: 60px; height: 60px;
                       background-color: navy; }",
        );
        let root = fixture.doc.root;
        let card = fixture.el(root, "view.card");
        let wide = fixture.el(card, "view.wide");
        let corner = fixture.el(card, "view.corner");
        fixture.animate(card, curve);
        fixture.offsets.push((card, Vector2D::new(24.0, 0.0)));
        fixture.probes = vec![card, wide, corner];
        fixture.check(&format!("rounded hidden card, {curve}"));
    }
}

/// An animated card holding a scroller at a non-zero offset override: the
/// scroll node sits inside the animation node.
#[test]
fn a_scroller_inside_an_animated_card_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".card { width: 240px; height: 220px; margin: 60px 0 0 100px; padding: 20px;
                     background-color: teal; }
             .scroller { overflow: scroll; width: 200px; height: 150px;
                         flex-direction: column; }
             .row { height: 40px; flex-shrink: 0; margin-bottom: 6px;
                    background-color: orange; }",
        );
        let root = fixture.doc.root;
        let card = fixture.el(root, "view.card");
        let scroller = fixture.el(card, "view.scroller");
        let rows: Vec<_> = (0..8).map(|_| fixture.el(scroller, "view.row")).collect();
        fixture.animate(card, curve);
        fixture.offsets.push((scroller, Vector2D::new(0.0, 37.0)));
        fixture.probes = vec![card, rows[1], rows[3]];
        fixture.check(&format!("scroller in animated card, {curve}"));
    }
}

/// A sticky header inside an animated card, stuck by the outer scroller's
/// offset: the sticky node sits inside the animation node.
#[test]
fn a_sticky_box_inside_an_animated_card_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".scroller { overflow: scroll; width: 320px; height: 320px; margin: 60px 0 0 100px;
                         flex-direction: column; }
             .spacer { height: 50px; flex-shrink: 0; }
             .card { height: 400px; flex-shrink: 0; flex-direction: column;
                     background-color: teal; }
             .sticky { position: sticky; top: 0px; height: 40px; flex-shrink: 0;
                       background-color: orange; }
             .tail { height: 400px; flex-shrink: 0; }",
        );
        let root = fixture.doc.root;
        let scroller = fixture.el(root, "view.scroller");
        fixture.el(scroller, "view.spacer");
        let card = fixture.el(scroller, "view.card");
        let sticky = fixture.el(card, "view.sticky");
        fixture.el(scroller, "view.tail");
        fixture.animate(card, curve);
        fixture.offsets.push((scroller, Vector2D::new(0.0, 120.0)));
        fixture.probes = vec![card, sticky];
        fixture.check(&format!("sticky in animated card, {curve}"));
    }
}

/// An animated box inside a stuck sticky header: the animation node sits
/// inside the sticky node.
#[test]
fn an_animated_box_inside_a_sticky_box_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".scroller { overflow: scroll; width: 320px; height: 320px; margin: 60px 0 0 100px;
                         flex-direction: column; }
             .spacer { height: 30px; flex-shrink: 0; }
             .sticky { position: sticky; top: 0px; height: 140px; flex-shrink: 0;
                       padding: 20px; background-color: orange; }
             .card { width: 120px; height: 80px; background-color: teal; }
             .tail { height: 800px; flex-shrink: 0; }",
        );
        let root = fixture.doc.root;
        let scroller = fixture.el(root, "view.scroller");
        fixture.el(scroller, "view.spacer");
        let sticky = fixture.el(scroller, "view.sticky");
        let card = fixture.el(sticky, "view.card");
        fixture.el(scroller, "view.tail");
        fixture.animate(card, curve);
        fixture.offsets.push((scroller, Vector2D::new(0.0, 100.0)));
        fixture.probes = vec![sticky, card];
        fixture.check(&format!("animated in sticky, {curve}"));
    }
}

/// A rotating card holding an animated child: two animation nodes on one
/// path, the inner one running each curve.
#[test]
fn nested_animated_elements_compose_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".outer { width: 240px; height: 200px; margin: 80px 0 0 120px; padding: 30px;
                      background-color: teal; }
             .inner { width: 100px; height: 80px; background-color: orange; }",
        );
        let root = fixture.doc.root;
        let outer = fixture.el(root, "view.outer");
        let inner = fixture.el(outer, "view.inner");
        fixture.animate(outer, "rotate");
        fixture.animate(inner, curve);
        fixture.exported = 2;
        fixture.probes = vec![outer, inner];
        fixture.check(&format!("nested, inner {curve}"));
    }
}
