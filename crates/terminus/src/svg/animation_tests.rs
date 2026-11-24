//! `transform` and `opacity` animations of the elements below an `svg`
//! element in a document, run by the painter (`visual::svg`): which
//! elements get an animation slot, which items compose in its space, what
//! the composed frame draws at an instant against what the main thread
//! draws there, and that a main-thread tick records the root again.

use euclid::default::Vector2D;

use crate::kurbo::{Affine, Point};
use crate::paint::display_list::{DisplayList, Item as ListItem};
use crate::test_common::Doc;
use crate::tree::document::NodeId;
use crate::tree::xml::{XmlMarkup, XmlNamespace};
use crate::visual::transform::planar;
use crate::visual::{CommittedFrame, LayerKind, PaintItemKind, ScrollSlot, SpaceKind};

const NAMESPACE: &str = "@namespace svg url(http://www.w3.org/2000/svg);";

const PAGE: &str = "page { display: flex; align-items: flex-start; width: 400px; height: 300px; }";

const SPIN: &str = "@keyframes spin { from { transform: rotate(0deg); }
                                      to { transform: rotate(360deg); } }";

/// The same turn, with a `fill` keyframe that changes nothing drawn: the
/// export refuses a curve that animates anything but `transform` and
/// `opacity`, so the main thread runs it.
const SPIN_ON_MAIN: &str = "@keyframes spin { from { transform: rotate(0deg); fill: red; }
                                              to { transform: rotate(360deg); fill: red; } }";

/// A page whose `svg` root, `markup`, sits under `parent` (the document
/// element when `None`), styled by `css`; the clock started at 0 and
/// ticked to 0.25, so every animation runs, and committed.
fn page(css: &str, markup: &str, parent: Option<&str>) -> (Doc, NodeId) {
    let mut doc = Doc::with_css(&format!("{NAMESPACE} {PAGE} {css}"));
    doc.dom.add_svg_user_agent_stylesheet();
    let parent = match parent {
        Some(spec) => doc.el(doc.root, spec),
        None => doc.root,
    };
    let markup =
        XmlMarkup::parse(markup.as_bytes(), Some(XmlNamespace::Svg)).expect("the markup parses");
    let root = doc.dom.append_svg_markup(parent, &markup, || ());
    doc.dom.commit();
    // The flush created the animations; the tick at 0 starts them on the
    // timeline's origin, and the one at 0.25 runs them.
    doc.dom.advance_animations(0.0);
    let tick = doc.dom.advance_animations(0.25);
    assert!(tick.needs_next_frame, "the fixture animates");
    doc.dom.commit();
    (doc, root)
}

fn by_id(doc: &Doc, id: &str) -> NodeId {
    doc.dom
        .query_selector(doc.root, &format!("#{id}"))
        .expect("parses")
        .expect("an element with that id")
}

/// The slot index of `node`'s curve in `frame`.
fn slot_of(frame: &CommittedFrame, node: NodeId) -> u32 {
    let index = frame
        .animation_slots()
        .iter()
        .position(|slot| slot.node == node)
        .expect("the element exports a curve");
    u32::try_from(index).expect("few slots")
}

/// The kinds on the space path of `node`'s recorded items, innermost
/// first; every item of one element composes in the same space.
fn path_of(frame: &CommittedFrame, node: NodeId) -> Vec<SpaceKind> {
    let spaces: Vec<_> = frame
        .order
        .items()
        .iter()
        .filter(|item| item.node == node && matches!(item.kind, PaintItemKind::Svg { .. }))
        .map(|item| item.space)
        .collect();
    assert!(!spaces.is_empty(), "the element draws an item");
    assert!(spaces.windows(2).all(|pair| pair[0] == pair[1]));
    crate::visual::space::path(frame.order.spaces(), spaces[0]).collect()
}

/// Where each recorded item lands at `now` with every scroller at
/// `offset`: its world matrix under its space's sampled map, by element.
fn placed(frame: &CommittedFrame, now: f64, offset: Vector2D<f32>) -> Vec<(NodeId, Affine)> {
    let offset_of = |_: &ScrollSlot| Some(offset);
    let animations = frame.order.sample_animations(Some(now), &offset_of);
    let stickies = frame.order.sample_stickies(1.0, &offset_of);
    let anchored = frame
        .order
        .sample_anchored(&animations, &stickies, 1.0, &offset_of);
    let samples = frame
        .order
        .space_samples(&animations, &stickies, &anchored, 1.0, &offset_of);
    frame
        .order
        .items()
        .iter()
        .filter(|item| matches!(item.kind, PaintItemKind::Svg { .. }))
        .map(|item| {
            (
                item.node,
                samples.css(item.space) * planar(&frame.order.matrix(item.transform)),
            )
        })
        .collect()
}

fn same(a: Affine, b: Affine, tolerance: f64) -> bool {
    a.as_coeffs()
        .iter()
        .zip(b.as_coeffs())
        .all(|(a, b)| (a - b).abs() <= tolerance)
}

/// The composed frame's fills and layer alphas, in order: what a drawing
/// at one instant is compared by.
fn drawn(list: &DisplayList) -> (Vec<Affine>, Vec<f32>) {
    let mut fills = Vec::new();
    let mut alphas = Vec::new();
    for item in list.items() {
        match item {
            ListItem::Fill(draw) => fills.push(list.transform(draw.transform)),
            ListItem::PushLayer { alpha, .. } => alphas.push(*alpha),
            _ => {}
        }
    }
    (fills, alphas)
}

/// `frame` composed at `now` with every scroller at `offset`.
fn composed(frame: &CommittedFrame, now: f64, offset: Vector2D<f32>) -> DisplayList {
    let mut list = DisplayList::new();
    frame.compose_into(&mut list, &[], &[], &[], &|_| Some(offset), Some(now));
    list
}

/// What the main thread draws at `now`: a page built from `css` (with the
/// keyframes `main` in place of the exported ones) and `markup`, ticked to
/// `now`, its scrollers scrolled to `offset`, and committed.
fn on_main(
    css: &str,
    main: &str,
    markup: &str,
    parent: Option<&str>,
    now: f64,
    offset: Vector2D<f32>,
) -> DisplayList {
    let (mut doc, _) = page(&format!("{css} {main}"), markup, parent);
    let frame = doc.dom.commit();
    assert!(frame.animation_slots().is_empty(), "nothing exports");
    doc.dom.advance_animations(now);
    if offset != Vector2D::zero() {
        let scroller = doc
            .dom
            .query_selector(doc.root, ".scroller")
            .expect("parses")
            .expect("a scroller");
        doc.dom.scroll_to(scroller, offset);
    }
    let frame = doc.dom.commit();
    composed(&frame, now, offset)
}

/// Asserts that the compositor's drawing of `css` (with the exported
/// keyframes `exported`) at each of `times` is the main thread's drawing
/// at that instant: every fill's transform within `TOLERANCE` device px
/// per coefficient, every layer alpha within 1e-4.
fn assert_draws_as_main(
    (css, exported, main): (&str, &str, &str),
    markup: &str,
    parent: Option<&str>,
    times: &[f64],
    offset: Vector2D<f32>,
) {
    /// The frame's matrices are f32: a coefficient of a few hundred px
    /// carries about 3e-5 of rounding on each side, and the compositor's
    /// delta one more product.
    const TOLERANCE: f64 = 1e-3;
    let (mut doc, _) = page(&format!("{css} {exported}"), markup, parent);
    let frame = doc.dom.commit();
    assert!(!frame.animation_slots().is_empty(), "the curves export");
    for &now in times {
        let (fills, alphas) = drawn(&composed(&frame, now, offset));
        let (main_fills, main_alphas) = drawn(&on_main(css, main, markup, parent, now, offset));
        assert_eq!(fills.len(), main_fills.len(), "{now}: the same fills");
        for (fill, main) in fills.iter().zip(&main_fills) {
            assert!(
                same(*fill, *main, TOLERANCE),
                "{now}: {fill:?} against {main:?}"
            );
        }
        assert_eq!(alphas.len(), main_alphas.len(), "{now}: the same layers");
        for (alpha, main) in alphas.iter().zip(&main_alphas) {
            assert!((alpha - main).abs() < 1e-4, "{now}: {alpha} against {main}");
        }
    }
}

const SPINNER: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
    <g id="g" class="spin"><rect id="a" x="10" y="10" width="20" height="20" fill="red"/>
      <rect id="b" x="30" y="10" width="10" height="10" fill="green"/></g>
    <rect id="c" x="60" y="60" width="10" height="10" fill="blue"/></svg>"#;

#[test]
fn a_spinning_group_runs_in_the_painter() {
    let css = format!("{SPIN} svg|g.spin {{ animation: spin 1s linear infinite; }}");
    let (mut doc, root) = page(&css, SPINNER, None);
    let frame = doc.dom.commit();
    let group = by_id(&doc, "g");
    let slot = slot_of(&frame, group);
    assert_eq!(frame.animation_slots().len(), 1, "one slot, the group's");
    assert!(!frame.needs_main_ticks(), "nothing else needs a tick");
    assert!(frame.animations_active());
    let (a, b, c) = (by_id(&doc, "a"), by_id(&doc, "b"), by_id(&doc, "c"));
    for shape in [a, b] {
        assert_eq!(path_of(&frame, shape)[0], SpaceKind::Animation(slot));
    }
    assert!(
        path_of(&frame, c)
            .iter()
            .all(|kind| !matches!(kind, SpaceKind::Animation(_))),
        "a sibling of the group does not move with it"
    );

    // Two instants: the group's items move, the sibling does not.
    let zero = Vector2D::zero();
    let (early, late) = (placed(&frame, 0.4, zero), placed(&frame, 0.7, zero));
    for ((node, early), (_, late)) in early.iter().zip(&late) {
        let moved = !same(*early, *late, 1e-6);
        assert_eq!(moved, *node != c, "{node:?}");
    }

    // While the main thread does not tick, nothing records.
    let mut recorded = doc.dom.svg_records(root);
    doc.dom.commit();
    assert_eq!(doc.dom.svg_records(root), recorded, "no tick, no record");

    // A tick the main thread runs all the same restyles the group: the root
    // records again, and the next commit exports the curve again over the
    // new recording.
    for now in [0.5, 0.75, 1.5] {
        doc.dom.advance_animations(now);
        let frame = doc.dom.commit();
        assert!(doc.dom.svg_records(root) > recorded, "{now}: records again");
        recorded = doc.dom.svg_records(root);
        assert!(!frame.needs_main_ticks(), "{now}");
        assert_eq!(frame.animation_slots().len(), 1, "{now}");
        let slot = slot_of(&frame, group);
        for shape in [a, b] {
            assert_eq!(
                path_of(&frame, shape)[0],
                SpaceKind::Animation(slot),
                "{now}"
            );
        }
    }
}

/// About the viewport's centre (a `view-box` reference), so the turn keeps
/// every shape inside the root: one the destination clip hides is culled by
/// a commit and kept by the compositor, which draws it clipped.
#[test]
fn a_spinning_group_draws_as_the_main_thread_does() {
    let css = "svg|g.spin { transform-origin: 50px 50px; animation: spin 1s linear infinite; }";
    assert_draws_as_main(
        (css, SPIN, SPIN_ON_MAIN),
        SPINNER,
        None,
        &[0.25, 0.4, 0.9, 1.6],
        Vector2D::zero(),
    );
}

/// `transform-box: fill-box` with `transform-origin: center` turns the
/// shape about its own centre.
#[test]
fn a_fill_box_turn_keeps_the_centre_of_the_shape() {
    let css = "svg|rect.spin { transform-box: fill-box; transform-origin: center;
                               animation: spin 1s linear infinite; }";
    let markup = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
        <rect id="r" class="spin" x="20" y="30" width="40" height="10" fill="red"/></svg>"#;
    let (mut doc, _) = page(&format!("{css} {SPIN}"), markup, None);
    let frame = doc.dom.commit();
    let rect = by_id(&doc, "r");
    slot_of(&frame, rect);
    // The item's matrix places its ink rectangle, whose origin is the
    // rect's own (no stroke): its centre is (20, 5) in it.
    let centre = |now: f64| {
        let (_, matrix) = placed(&frame, now, Vector2D::zero())
            .into_iter()
            .find(|(node, _)| *node == rect)
            .expect("the rect's item");
        matrix * Point::new(20.0, 5.0)
    };
    let committed = centre(0.25);
    for now in [0.3, 0.5, 0.8, 1.1] {
        assert!(
            centre(now).distance(committed) < 1e-3,
            "{now}: {:?} against {committed:?}",
            centre(now)
        );
    }
    assert_draws_as_main(
        (css, SPIN, SPIN_ON_MAIN),
        markup,
        None,
        &[0.3, 0.5, 0.8, 1.1],
        Vector2D::zero(),
    );
}

const FADE: &str = "@keyframes fade { from { opacity: 1; } 50% { opacity: 1; }
                                      to { opacity: 0; } }";
const FADE_ON_MAIN: &str = "@keyframes fade { from { opacity: 1; fill: red; }
                                              50% { opacity: 1; fill: red; }
                                              to { opacity: 0; fill: red; } }";

/// An opacity curve on a shape and on a group, committed while both are
/// still at 1: each records a layer at alpha 1 that the curve fades.
#[test]
fn an_opacity_curve_fades_the_layer_recorded_at_one() {
    let css = "svg|rect.fade, svg|g.fade { animation: fade 1s linear infinite; }";
    let markup = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
        <rect id="r" class="fade" width="20" height="20" fill="red"/>
        <g id="g" class="fade"><rect x="40" width="20" height="20" fill="blue"/></g></svg>"#;
    let (mut doc, root) = page(&format!("{css} {FADE}"), markup, None);
    let frame = doc.dom.commit();
    assert!(!frame.needs_main_ticks());
    for id in ["r", "g"] {
        let node = by_id(&doc, id);
        let slot = slot_of(&frame, node);
        let layer = frame
            .order
            .layers()
            .iter()
            .find(|layer| layer.node == node && matches!(layer.kind, LayerKind::Svg { .. }))
            .expect("a layer at opacity 1");
        assert_eq!(
            crate::visual::space::path(frame.order.spaces(), layer.space).next(),
            Some(SpaceKind::Animation(slot)),
            "{id}: the layer composes in the element's space",
        );
        let at = |now: f64| {
            frame.animation_slots()[slot as usize]
                .sample(
                    Some(now),
                    &frame.committed_offsets(),
                    &mut stylo::properties::animated_properties::AnimationValueMap::default(),
                )
                .alpha
                .expect("an opacity curve")
        };
        assert!((at(0.25) - 1.0).abs() < 1e-6, "{id}");
        assert!((at(0.75) - 0.5).abs() < 1e-4, "{id}");
    }
    let (_, alphas) = drawn(&composed(&frame, 0.75, Vector2D::zero()));
    assert_eq!(alphas.len(), 2, "{alphas:?}");
    assert!(alphas.iter().all(|alpha| (alpha - 0.5).abs() < 1e-4));

    let recorded = doc.dom.svg_records(root);
    doc.dom.advance_animations(0.8);
    let frame = doc.dom.commit();
    assert!(!frame.needs_main_ticks());
    assert!(doc.dom.svg_records(root) > recorded, "a tick records again");
    assert_eq!(frame.animation_slots().len(), 2, "and both curves export");
    assert_draws_as_main(
        (css, FADE, FADE_ON_MAIN),
        markup,
        None,
        &[0.25, 0.6, 0.9],
        Vector2D::zero(),
    );
}

/// An animation the export refuses keeps the main-thread path: frames are
/// asked of the main thread, and every tick records the root again.
#[test]
fn a_refused_export_records_the_root_each_tick() {
    let css = format!("{SPIN_ON_MAIN} svg|g.spin {{ animation: spin 1s linear infinite; }}");
    let (mut doc, root) = page(&css, SPINNER, None);
    let frame = doc.dom.commit();
    assert!(frame.animation_slots().is_empty());
    assert!(frame.needs_main_ticks());
    let mut recorded = doc.dom.svg_records(root);
    for now in [0.5, 0.75] {
        doc.dom.advance_animations(now);
        let frame = doc.dom.commit();
        assert!(frame.needs_main_ticks(), "{now}");
        assert!(doc.dom.svg_records(root) > recorded, "{now}: records again");
        recorded = doc.dom.svg_records(root);
    }
}

/// A turning group inside a turning group: two slots, the inner's space
/// inside the outer's, composed as the main thread draws them.
#[test]
fn nested_turns_compose_inner_inside_outer() {
    let css = "svg|g.spin { animation: spin 1s linear infinite; }
               svg|g.slow { animation: spin 3s linear infinite; }";
    let markup = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
        <g id="outer" class="slow" transform="translate(50 50)">
          <rect x="-40" y="-40" width="10" height="10" fill="red"/>
          <g id="inner" class="spin"><rect id="leaf" x="5" y="5" width="20" height="10" fill="blue"/></g>
        </g></svg>"#;
    let (mut doc, _) = page(&format!("{css} {SPIN}"), markup, None);
    let frame = doc.dom.commit();
    assert!(!frame.needs_main_ticks());
    let (outer, inner) = (by_id(&doc, "outer"), by_id(&doc, "inner"));
    let (outer, inner) = (slot_of(&frame, outer), slot_of(&frame, inner));
    let path = path_of(&frame, by_id(&doc, "leaf"));
    assert_eq!(
        path[..2],
        [SpaceKind::Animation(inner), SpaceKind::Animation(outer)]
    );
    assert_draws_as_main(
        (css, SPIN, SPIN_ON_MAIN),
        markup,
        None,
        &[0.25, 0.5, 1.2, 2.9],
        Vector2D::zero(),
    );
}

/// A turning group in a root inside a scroller: the animation space is
/// inside the scroller's, so the turn applies before the scroll.
#[test]
fn a_turn_inside_a_scroller_composes_inside_the_scroll() {
    let css = ".scroller { display: flex; align-items: flex-start; width: 200px; height: 100px;
                           overflow: scroll; }
               svg { flex-shrink: 0; }
               svg|g.spin { animation: spin 1s linear infinite; }";
    let markup = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="300">
        <g class="spin" transform-origin="50 150" style="transform-origin: 50px 150px">
        <rect id="r" x="20" y="120" width="40" height="20" fill="red"/></g></svg>"#;
    let (mut doc, _) = page(&format!("{css} {SPIN}"), markup, Some("view.scroller"));
    let frame = doc.dom.commit();
    assert!(!frame.needs_main_ticks());
    let path = path_of(&frame, by_id(&doc, "r"));
    let turn = path
        .iter()
        .position(|kind| matches!(kind, SpaceKind::Animation(_)))
        .expect("an animation node");
    let scroll = path
        .iter()
        .position(|kind| matches!(kind, SpaceKind::Scroll(_)))
        .expect("a scroll node");
    assert!(turn < scroll, "the turn is inside the scroll: {path:?}");
    assert_draws_as_main(
        (css, SPIN, SPIN_ON_MAIN),
        markup,
        Some("view.scroller"),
        &[0.4, 0.8],
        Vector2D::new(0.0, 60.0),
    );
}

/// A `use` instance is an element of its own in the `use`'s shadow root,
/// with its own animation set: drawn once, it gets a slot of its own, as
/// does the drawn original it was cloned from.
#[test]
fn a_use_instance_and_its_original_each_get_a_slot() {
    let css = "svg|g.spin { transform-origin: 20px 20px; animation: spin 1s linear infinite; }";
    let markup = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
        <g id="t" class="spin"><rect x="10" y="10" width="20" height="20" fill="red"/></g>
        <use href="#t" x="50" y="50"/></svg>"##;
    let (mut doc, root) = page(&format!("{css} {SPIN}"), markup, None);
    let frame = doc.dom.commit();
    assert_eq!(
        frame.animation_slots().len(),
        2,
        "the original and the instance"
    );
    assert!(!frame.needs_main_ticks());
    let recorded = doc.dom.svg_records(root);
    doc.dom.advance_animations(0.6);
    let frame = doc.dom.commit();
    assert!(!frame.needs_main_ticks());
    assert!(doc.dom.svg_records(root) > recorded, "a tick records again");
    assert_eq!(frame.animation_slots().len(), 2, "and both curves export");
    assert_draws_as_main(
        (css, SPIN, SPIN_ON_MAIN),
        markup,
        None,
        &[0.3, 0.7],
        Vector2D::zero(),
    );
}
