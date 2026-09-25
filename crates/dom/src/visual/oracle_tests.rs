//! Compose-versus-commit oracle for exported curves.
//!
//! A frame committed at one instant and composed at a later one must equal
//! the frame the main thread commits at that later instant: every item and
//! clip at the same place, every animated effect layer at the same alpha,
//! and every hit test answering the same element. `early` is committed with
//! the curve running; for each later `t` the document is advanced and
//! committed again as `late`, and `early` sampled at `Some(t)` is compared
//! with `late` sampled at `None` — the reference involves no curve at all.
//! Every item `late` shows inside the viewport must also be one `early`'s
//! culled walk encodes, inside the rect of every group holding it: an encode
//! and a group rect serve their curves' whole domain.
//!
//! Before any geometry, the values themselves: `early`'s curves sample a
//! property at `t` exactly when `late`'s cascade takes it from the
//! animations origin, every sampled `opacity` and `transform` value is
//! bit-equal to the one `late` committed, and the world a sampled transform
//! folds to is `late`'s bit for bit relative to the parent's committed world.
//! Composed geometry agrees to f32 rounding. At `early` itself every delta is
//! the identity and every alpha the committed opacity. Past a curve's end
//! the frame hands back and the curve holds its last instant, which is all it
//! claims there. Timings are binary fractions, so a chain of main-thread
//! ticks and the sampler's one step from the commit accumulate the same
//! start times; for other durations they can differ in the last bit.

use euclid::default::Vector2D;
use stylo::properties::animated_properties::{AnimationValue, AnimationValueMap};
use stylo::properties::{LonghandId, OwnedPropertyDeclarationId, PropertyDeclarationId};
use stylo::rule_tree::CascadeOrigin;

use crate::paint::convert::item_affine;
use crate::test_common::Doc;
use crate::vello::kurbo::{Affine, Point, Rect};
use crate::visual::curves::Timeline;
use crate::visual::{CommittedFrame, PaintItemKind, ScrollOffsets, ScrollSlot, SpaceKind};
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
/// instants it is composed at — the last one in the next iteration, at a
/// progress before `EARLY`'s.
const EARLY: f64 = 0.1;
const LATER: [f64; 3] = [0.35, 0.8, 1.05];

/// Group rects hold composed positions to this many CSS px.
const TOLERANCE: f64 = 1e-3;

/// How far apart two composed positions near `magnitude` CSS px may be: the
/// worlds agree bit for bit, so what is left is the f32 rounding of each
/// frame's own bake, a couple of ulps of the coordinate.
fn composed_tolerance(magnitude: f64) -> f64 {
    1e-6 + 2.0 * f64::from(f32::EPSILON) * magnitude
}

/// One fixture: its document, the scroll offsets both frames compose at, the
/// number of curves it exports, elements the hit sweep must reach, and
/// whether `early`'s walk must cull something for the coverage check to
/// prove anything.
struct Fixture {
    doc: Doc,
    offsets: Vec<(NodeId, Vector2D<f32>)>,
    exported: usize,
    probes: Vec<NodeId>,
    culls: bool,
    /// Main-thread ticks, each rendered, between the restyles and `early`.
    ticks: Vec<f64>,
    /// When `early` is committed.
    early: f64,
    /// The instants `early` is composed at.
    later: Vec<f64>,
    /// Inline styles set once the timeline has started, each followed by a
    /// render and a tick at 0: how restyled animation lists start.
    restyles: Vec<(NodeId, String)>,
    /// The animations the first slot's curve must carry.
    entries: Option<usize>,
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
            culls: false,
            ticks: Vec::new(),
            early: EARLY,
            later: LATER.to_vec(),
            restyles: Vec::new(),
            entries: None,
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
    /// fresh commit at each of the later instants.
    fn check(mut self, label: &str) {
        let dom = &mut self.doc.dom;
        dom.render();
        dom.advance_animations(0.0);
        for (node, css) in std::mem::take(&mut self.restyles) {
            dom.set_inline_style(node, &css);
            dom.render();
            dom.advance_animations(0.0);
        }
        for &now in &self.ticks {
            dom.advance_animations(now);
            dom.render();
        }
        assert!(
            dom.advance_animations(self.early).needs_next_frame,
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
        if let Some(entries) = self.entries {
            assert_eq!(
                early.animation_slots()[0].curve.animations.len(),
                entries,
                "{label}: the curve's animations"
            );
        }
        self.compare_commit_instant(&early, label);
        let encoded = crate::paint::walker::encoded_items(&self.doc.dom, &early.order);
        assert!(
            !self.culls || encoded.contains(&false),
            "{label}: early's walk culls something"
        );
        let rects = crate::paint::walker::layer_rects(&self.doc.dom, &early.order);
        let expiry = early
            .animation_slots()
            .iter()
            .filter_map(|slot| slot.curve.expires_at)
            .reduce(f64::min);
        for t in std::iter::once(self.early).chain(self.later.clone()) {
            self.doc.dom.advance_animations(t);
            self.doc.dom.render();
            let late = self
                .doc
                .dom
                .committed_frame()
                .expect("a frame is committed");
            let label = format!("{label} at t = {t}");
            assert_eq!(
                early.animation_boundary_passed(t),
                expiry.is_some_and(|end| t >= end),
                "{label}: the hand-back instant"
            );
            if early.animation_boundary_passed(t) {
                // The main thread's commit takes over from here; until it is
                // adopted each curve holds its domain's last instant.
                assert!(late.animation_slots().len() <= self.exported, "{label}");
                Self::compare_hold(&early, t, &label);
                continue;
            }
            self.compare_values(&early, &late, Some(t), &label);
            self.compare_geometry(&early, &late, Some(t), &label);
            self.compare_hits(&early, &late, Some(t), &label);
            self.compare_coverage(&encoded, &late, &label);
            self.compare_groups(&rects, &early, &late, Some(t), &label);
        }
    }

    /// Commits `early` with `source` scrolled to `from` and every curve on a
    /// scroll timeline of it, then checks `early` composed with `source` at
    /// each of `offsets` against a fresh commit after the document scrolled
    /// there. The clock never moves.
    fn check_scroll(mut self, label: &str, source: NodeId, from: f32, offsets: &[f32]) {
        let dom = &mut self.doc.dom;
        dom.render();
        dom.advance_animations(0.0);
        dom.scroll_to(source, Vector2D::new(0.0, from));
        dom.note_scroll_windows_stale();
        dom.render();
        let early = dom.committed_frame().expect("a frame is committed");
        assert_eq!(
            early.animation_slots().len(),
            self.exported,
            "{label}: every animated element exports",
        );
        assert!(
            early
                .animation_slots()
                .iter()
                .all(|slot| slot.curve.reads_scroll()),
            "{label}: every curve reads its source's slot"
        );
        assert!(
            !early.needs_main_ticks() && !early.animations_active(),
            "{label}: nothing ticks"
        );
        assert!(
            !early.has_live_curves(),
            "{label}: no curve reads the clock"
        );
        self.compare_commit_instant(&early, label);
        let encoded = crate::paint::walker::encoded_items(&self.doc.dom, &early.order);
        let rects = crate::paint::walker::layer_rects(&self.doc.dom, &early.order);
        for &offset in offsets {
            let offset = Vector2D::new(0.0, offset);
            let dom = &mut self.doc.dom;
            dom.scroll_to(source, offset);
            // A scroll the main thread adopts re-cascades no painted curve;
            // the next commit's resolution re-samples it.
            dom.note_scroll_windows_stale();
            dom.render();
            let late = dom.committed_frame().expect("a frame is committed");
            self.offsets.retain(|(node, _)| *node != source);
            self.offsets.push((source, offset));
            let label = format!("{label} at offset {}", offset.y);
            self.compare_values(&early, &late, None, &label);
            self.compare_geometry(&early, &late, None, &label);
            self.compare_hits(&early, &late, None, &label);
            self.compare_coverage(&encoded, &late, &label);
            self.compare_groups(&rects, &early, &late, None, &label);
        }
    }

    /// Whether `node`'s committed style takes `property` from the animations
    /// or the transitions origin: whether the main thread's cascade sampled
    /// an animation or a transition for it.
    fn animated_by_cascade(&self, node: NodeId, property: LonghandId) -> bool {
        let dom = &self.doc.dom;
        let style = dom
            .paint_style(node)
            .expect("an animated element is styled");
        let guard = dom.style_engine().shared_lock().read();
        style.rules().self_and_ancestors().any(|rule| {
            matches!(
                rule.cascade_level().origin(),
                CascadeOrigin::Animations | CascadeOrigin::Transitions
            ) && rule.style_source().is_some_and(|source| {
                source
                    .read(&guard)
                    .contains(PropertyDeclarationId::Longhand(property))
            })
        })
    }

    /// At the commit instant and offsets every curve composes the committed
    /// frame: an identity delta and the committed opacity.
    fn compare_commit_instant(&self, early: &CommittedFrame, label: &str) {
        let mut values = AnimationValueMap::default();
        for slot in early.animation_slots() {
            let sample = slot.sample(Some(self.early), &early.committed_offsets(), &mut values);
            let error = sample
                .delta
                .as_coeffs()
                .iter()
                .zip(Affine::IDENTITY.as_coeffs())
                .map(|(got, want)| (got - want).abs())
                .fold(0.0, f64::max);
            assert!(
                error <= 1e-12,
                "{label}: {:?} commits a delta {:?}",
                slot.node,
                sample.delta
            );
            let committed = self
                .doc
                .dom
                .paint_style(slot.node)
                .expect("an animated element is styled")
                .get_effects()
                .opacity;
            if let Some(alpha) = sample.alpha {
                assert_eq!(
                    alpha.to_bits(),
                    committed.clamp(0.0, 1.0).to_bits(),
                    "{label}: {:?} commits alpha {alpha}",
                    slot.node,
                );
            }
        }
    }

    /// Past its domain's end a curve holds the domain's last instant.
    fn compare_hold(early: &CommittedFrame, t: f64, label: &str) {
        let (mut held, mut last) = (AnimationValueMap::default(), AnimationValueMap::default());
        for slot in early.animation_slots() {
            let Some(end) = slot.curve.expires_at else {
                continue;
            };
            if t < end {
                continue;
            }
            let offsets = early.committed_offsets();
            slot.curve.values_at(Some(t), &offsets, &mut held);
            slot.curve
                .values_at(Some(end.next_down()), &offsets, &mut last);
            assert_eq!(
                held, last,
                "{label}: {:?} holds its last instant",
                slot.node
            );
        }
    }

    /// `early`'s curves sample a property at `at` and the fixture's offsets
    /// exactly when `late`'s cascade animated it, every sampled value is the
    /// one `late` committed, bit for bit, and a sampled transform folds to
    /// `late`'s world exactly relative to the parent's committed world.
    fn compare_values(
        &self,
        early: &CommittedFrame,
        late: &CommittedFrame,
        at: Option<f64>,
        label: &str,
    ) {
        let offset_of = self.offset_of();
        let offsets = ScrollOffsets {
            slots: early.scroll_slots(),
            offset_of: &offset_of,
        };
        let mut values = AnimationValueMap::default();
        for slot in early.animation_slots() {
            let label = format!("{label}: {:?}", slot.node);
            slot.curve.values_at(at, &offsets, &mut values);
            for property in [LonghandId::Opacity, LonghandId::Transform] {
                let sampled = values.contains_key(&OwnedPropertyDeclarationId::Longhand(property));
                assert_eq!(
                    sampled,
                    self.animated_by_cascade(slot.node, property),
                    "{label}: whether {property:?} is animated",
                );
                assert!(!sampled || slot.curve.animates(property), "{label}");
            }
            let style = self
                .doc
                .dom
                .paint_style(slot.node)
                .expect("an animated element is styled");
            if let Some(sampled) =
                values.get(&OwnedPropertyDeclarationId::Longhand(LonghandId::Opacity))
            {
                let committed = AnimationValue::from_computed_values(
                    PropertyDeclarationId::Longhand(LonghandId::Opacity),
                    style,
                );
                assert_eq!(Some(sampled), committed.as_ref(), "{label}: opacity");
            }
            let Some(AnimationValue::Transform(sampled)) =
                values.get(&OwnedPropertyDeclarationId::Longhand(LonghandId::Transform))
            else {
                continue;
            };
            assert_eq!(sampled, &style.get_box().transform, "{label}: transform");
            let track = slot.curve.transform.as_ref().expect("a transform track");
            let own = |frame: &CommittedFrame| {
                let item = frame
                    .order
                    .items()
                    .iter()
                    .find(|item| item.node == slot.node && item.kind == PaintItemKind::ElementBox)
                    .expect("the element paints its box");
                (item.space, item.transform)
            };
            // An ancestor's curve moves the parent world the track holds
            // still; the geometry comparison covers that composition.
            let movers = super::space::path(early.order.spaces(), own(early).0)
                .filter(|kind| {
                    matches!(kind, SpaceKind::Animation(index)
                        if early.animation_slots()[*index as usize].curve.transform.is_some())
                })
                .count();
            if movers == 1 {
                assert_eq!(track.world(sampled), own(late).1, "{label}: world");
            }
        }
    }

    /// Every grid point inside the viewport where `late` shows an item lies
    /// inside `early`'s rect of every group holding that item, composed at
    /// `at`.
    fn compare_groups(
        &self,
        rects: &[Rect],
        early: &CommittedFrame,
        late: &CommittedFrame,
        at: Option<f64>,
        label: &str,
    ) {
        let offset_of = self.offset_of();
        let early_animations = early.order.sample_animations(at, &offset_of);
        let early_stickies = early.order.sample_stickies(1.0, &offset_of);
        let early_samples =
            early
                .order
                .space_samples(&early_animations, &early_stickies, 1.0, &offset_of);
        let late_animations = late.order.sample_animations(None, &offset_of);
        let late_stickies = late.order.sample_stickies(1.0, &offset_of);
        let late_samples =
            late.order
                .space_samples(&late_animations, &late_stickies, 1.0, &offset_of);
        let layers = early.order.layers();
        for (index, item) in late.order.items().iter().enumerate() {
            let groups: Vec<(Affine, Rect)> = layers
                .iter()
                .zip(rects)
                .filter(|(layer, _)| layer.items.contains(&index))
                .map(|(layer, rect)| (early_samples.css(layer.space).inverse(), *rect))
                .collect();
            if groups.is_empty() {
                continue;
            }
            // The whole 800 × 600 viewport, 7 px apart.
            for row in 0_u16..86 {
                for column in 0_u16..115 {
                    let point = Point2D::new(f32::from(column) * 7.0, f32::from(row) * 7.0);
                    if late.order.item_hit(item, point, &late_samples).is_none() {
                        continue;
                    }
                    for (unmap, rect) in &groups {
                        let local = *unmap * Point::new(f64::from(point.x), f64::from(point.y));
                        assert!(
                            local.x >= rect.x0 - TOLERANCE
                                && local.x <= rect.x1 + TOLERANCE
                                && local.y >= rect.y0 - TOLERANCE
                                && local.y <= rect.y1 + TOLERANCE,
                            "{label}: {:?} {:?} shows at {point:?}, outside its group's \
                             rect {rect:?} at {local:?}",
                            item.node,
                            item.kind,
                        );
                    }
                }
            }
        }
    }

    /// Every item `late` shows at a grid point inside the viewport is one
    /// `early` encodes. Items pair by index, as [`Self::compare_geometry`]
    /// checked.
    fn compare_coverage(&self, encoded: &[bool], late: &CommittedFrame, label: &str) {
        let offset_of = self.offset_of();
        let animations = late.order.sample_animations(None, &offset_of);
        let stickies = late.order.sample_stickies(1.0, &offset_of);
        let samples = late
            .order
            .space_samples(&animations, &stickies, 1.0, &offset_of);
        for (index, item) in late.order.items().iter().enumerate() {
            if encoded[index] {
                continue;
            }
            // The whole 800 × 600 viewport, 7 px apart.
            for row in 0_u16..86 {
                for column in 0_u16..115 {
                    let point = Point2D::new(f32::from(column) * 7.0, f32::from(row) * 7.0);
                    assert!(
                        late.order.item_hit(item, point, &samples).is_none(),
                        "{label}: {:?} {:?} shows at {point:?} but early culled it",
                        item.node,
                        item.kind,
                    );
                }
            }
        }
    }

    /// Every paired item's and clip's composed box, and every exported
    /// opacity, agree.
    fn compare_geometry(
        &self,
        early: &CommittedFrame,
        late: &CommittedFrame,
        at: Option<f64>,
        label: &str,
    ) {
        let offset_of = self.offset_of();
        let early_animations = early.order.sample_animations(at, &offset_of);
        let early_stickies = early.order.sample_stickies(1.0, &offset_of);
        let early_samples =
            early
                .order
                .space_samples(&early_animations, &early_stickies, 1.0, &offset_of);
        let late_animations = late.order.sample_animations(None, &offset_of);
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
                (early_samples.css(a.space), a_local),
                (late_samples.css(b.space), b_local),
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
                (early_samples.css(a.space), a_local),
                (late_samples.css(b.space), b_local),
                box_,
                &format!("{label}: clip of {:?}", a.node),
            );
        }

        for (index, slot) in (0_u32..).zip(early.animation_slots()) {
            let Some(alpha) = early_animations.get(index).alpha else {
                assert!(
                    !self.animated_by_cascade(slot.node, LonghandId::Opacity),
                    "{label}: {:?} animates opacity but samples no alpha",
                    slot.node
                );
                continue;
            };
            let committed = self
                .doc
                .dom
                .paint_style(slot.node)
                .expect("an animated element is styled")
                .get_effects()
                .opacity;
            // Paint clamps the committed opacity.
            assert_eq!(
                alpha.to_bits(),
                committed.clamp(0.0, 1.0).to_bits(),
                "{label}: sampled opacity {alpha}, committed {committed}",
            );
        }
    }

    /// `early` hit at `at` answers what `late` answers at `None` at every
    /// grid point clear of the edges `late` draws.
    fn compare_hits(
        &self,
        early: &CommittedFrame,
        late: &CommittedFrame,
        at: Option<f64>,
        label: &str,
    ) {
        let offset_of = self.offset_of();
        let late_hit = |x: f32, y: f32| late.hit(Point2D::new(x, y), &offset_of, None);
        let mut reached = Vec::new();
        let mut compared = 0_usize;
        // The whole 800 × 600 viewport, 7 px apart.
        for row in 0_u16..86 {
            for column in 0_u16..115 {
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
                let got = early.hit(Point2D::new(x, y), &offset_of, at);
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

/// `a` and `b`, each a space's map after a baked local map, put `box_`'s
/// corners within [`composed_tolerance`] of each other, at the magnitude
/// either bake rounded them at.
fn assert_same_box(
    (a_space, a_local): (Affine, Affine),
    (b_space, b_local): (Affine, Affine),
    [x0, y0, x1, y1]: [f64; 4],
    label: &str,
) {
    let (a, b) = (a_space * a_local, b_space * b_local);
    for corner in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        let corner = Point::new(corner.0, corner.1);
        let distance = (a * corner - b * corner).hypot();
        let magnitude = [a_local * corner, b_local * corner, b * corner]
            .into_iter()
            .map(|point| point.to_vec2().hypot())
            .fold(0.0, f64::max);
        assert!(
            distance < composed_tolerance(magnitude),
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

/// A 1600 px arm along the viewport's top edge turning a quarter about a
/// point 400 px in: cells past the right edge at the commit swing down into
/// view later, and the far end, which only enters below the bottom edge, is
/// culled.
#[test]
fn a_rotating_arm_encodes_every_cell_its_turn_shows() {
    let mut fixture = Fixture::new(
        ".arm { width: 1600px; height: 40px; flex-shrink: 0; transform-origin: 400px 20px; }
         .cell { width: 40px; height: 40px; flex-shrink: 0; background-color: teal; }",
    );
    let root = fixture.doc.root;
    let arm = fixture.el(root, "view.arm");
    let cells: Vec<_> = (0..40).map(|_| fixture.el(arm, "view.cell")).collect();
    fixture.animate(arm, "rotate");
    fixture.probes = vec![cells[10], cells[15]];
    fixture.culls = true;
    fixture.check("rotating arm");
}

/// A 4000 px card shrinking to half about its top inside a scroller composed
/// 600 px down: the rows that come into view need the scroll window crossed
/// before the curve's reach, and the far end, which no instant shows, is
/// culled.
#[test]
fn a_shrinking_card_in_a_scrolled_list_encodes_every_row_it_shows() {
    let mut fixture = Fixture::new(
        ".scroller { overflow: scroll; width: 300px; height: 600px; flex-shrink: 0;
                     flex-direction: column; }
         .card { width: 300px; height: 4000px; flex-shrink: 0; flex-direction: column;
                 transform-origin: 0px 0px; }
         .row { height: 40px; flex-shrink: 0; background-color: teal; }
         @keyframes shrink { from { transform: scale(1); } to { transform: scale(0.5); } }",
    );
    let root = fixture.doc.root;
    let scroller = fixture.el(root, "view.scroller");
    let card = fixture.el(scroller, "view.card");
    let rows: Vec<_> = (0..100).map(|_| fixture.el(card, "view.row")).collect();
    fixture.animate(card, "shrink");
    fixture.offsets.push((scroller, Vector2D::new(0.0, 600.0)));
    fixture.probes = vec![rows[27]];
    fixture.culls = true;
    fixture.check("shrinking card in a scrolled list");
}

/// A card sliding, turning or growing out of the static group holding it,
/// with a `<text>` inside it: the group's rect holds every place the card
/// can show.
#[test]
fn an_animated_card_inside_an_opacity_group_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".group { width: 160px; height: 120px; margin: 60px 0 0 80px; padding: 10px;
                      opacity: 0.5; background-color: navy; }
             .card { width: 120px; height: 80px; background-color: teal; }
             .label { width: 90px; height: 20px; font-family: Ahem; font-size: 20px; }",
        );
        let root = fixture.doc.root;
        let group = fixture.el(root, "view.group");
        let card = fixture.el(group, "view.card");
        let label = fixture.el(card, "text.label");
        fixture.text(label, "hellohello");
        fixture.animate(card, curve);
        fixture.probes = vec![group, card, label];
        fixture.check(&format!("card in an opacity group, {curve}"));
    }
}

/// The same inside a blurred group, whose rect is also its bake's.
#[test]
fn an_animated_card_inside_a_blurred_group_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".group { width: 160px; height: 120px; margin: 60px 0 0 80px; padding: 10px;
                      filter: blur(3px); background-color: navy; }
             .card { width: 120px; height: 80px; background-color: teal; }",
        );
        let root = fixture.doc.root;
        let group = fixture.el(root, "view.group");
        let card = fixture.el(group, "view.card");
        fixture.animate(card, curve);
        fixture.probes = vec![group, card];
        fixture.check(&format!("card in a blurred group, {curve}"));
    }
}

/// A fading card — a group its own opacity curve forces — holding an
/// animated child: two exports, the inner one inside the outer's group.
#[test]
fn an_animated_child_of_a_fading_card_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".outer { width: 160px; height: 120px; margin: 80px 0 0 120px; padding: 10px;
                      background-color: navy; }
             .inner { width: 120px; height: 80px; background-color: teal; }",
        );
        let root = fixture.doc.root;
        let outer = fixture.el(root, "view.outer");
        let inner = fixture.el(outer, "view.inner");
        fixture.animate(outer, "opacity");
        fixture.animate(inner, curve);
        fixture.exported = 2;
        fixture.probes = vec![outer, inner];
        fixture.check(&format!("child of a fading card, {curve}"));
    }
}

/// A `backdrop-filter` panel with an animated sibling behind it, one in
/// front of it, and an animated child of its own.
#[test]
fn a_backdrop_among_animated_boxes_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".behind { width: 200px; height: 120px; margin: 40px 0 0 60px; flex-shrink: 0;
                       background-color: orange; }
             .glass { position: absolute; left: 120px; top: 100px; width: 220px; height: 160px;
                      padding: 20px; backdrop-filter: blur(4px);
                      background-color: rgba(255, 255, 255, 0.3); }
             .inner { width: 100px; height: 60px; background-color: teal; }
             .front { position: absolute; left: 300px; top: 220px; width: 120px; height: 80px;
                      background-color: navy; }",
        );
        let root = fixture.doc.root;
        let behind = fixture.el(root, "view.behind");
        let glass = fixture.el(root, "view.glass");
        let inner = fixture.el(glass, "view.inner");
        let front = fixture.el(root, "view.front");
        for id in [behind, inner, front] {
            fixture.animate(id, curve);
        }
        fixture.exported = 3;
        fixture.probes = vec![behind, glass, inner, front];
        fixture.check(&format!("backdrop among animated boxes, {curve}"));
    }
}

/// A group committed mostly off the viewport's left edge that slides in by
/// its own curve: its rect is the viewport pulled back through the slide,
/// which must hold every place the slide later shows it.
#[test]
fn a_group_sliding_in_from_off_the_viewport_composes_as_committed() {
    for effect in ["opacity: 0.5;", "filter: blur(3px);"] {
        let mut fixture = Fixture::new(&format!(
            ".card {{ width: 200px; height: 120px; margin: 80px 0 0 100px; flex-shrink: 0;
                      background-color: teal; {effect} }}
             @keyframes enter {{ from {{ transform: translateX(-280px); }}
                                 to {{ transform: translateX(200px); }} }}"
        ));
        let root = fixture.doc.root;
        let card = fixture.el(root, "view.card");
        fixture.animate(card, "enter");
        fixture.probes = vec![card];
        fixture.check(&format!("group sliding in, {effect}"));
    }
}

/// A blurred `overflow: hidden` toast shrinking toward `scale(0)` holding a
/// sliding ticker: no viewport pulls back through the toast's curve, and
/// the toast's own clip holds the ticker's slide.
#[test]
fn a_ticker_in_a_shrinking_clipped_toast_composes_as_committed() {
    let mut fixture = Fixture::new(
        ".toast { width: 300px; height: 60px; margin: 100px 0 0 200px; overflow: hidden;
                  filter: blur(2px); background-color: navy; }
         .ticker { width: 1000px; height: 20px; flex-shrink: 0; background-color: teal; }
         @keyframes dismiss { from { transform: scale(1); } to { transform: scale(0); } }
         @keyframes tick { from { transform: translateX(0px); }
                           to { transform: translateX(-600px); } }",
    );
    let root = fixture.doc.root;
    let toast = fixture.el(root, "view.toast");
    let ticker = fixture.el(toast, "view.ticker");
    fixture.animate(toast, "dismiss");
    fixture.animate(ticker, "tick");
    fixture.exported = 2;
    fixture.probes = vec![toast, ticker];
    fixture.check("ticker in a shrinking clipped toast");
}

/// A card holding a fixed box under a child. Under a transform curve the
/// card's committed transform contains the box, so it moves with the curve;
/// under an opacity curve it escapes the card and stays put, fading inside
/// the card's group. The card's base transform is `none`; the curves start
/// at an identity function, so an exported transform curve here always
/// commits a non-empty list and the
/// `animates_transform` bit is not observable here (the delay case in
/// `visual/tests.rs` covers it). (`position: static` is outside the Lynx
/// grammar and the UA makes every `view` relative, so an absolute box never
/// escapes its parent here.)
#[test]
fn a_fixed_box_inside_an_animated_card_composes_as_committed() {
    let fixture = |curve: &str| {
        let mut fixture = Fixture::new(
            ".card { width: 220px; height: 140px; margin: 60px 0 0 80px; padding: 20px;
                     background-color: teal; }
             .mid { width: 60px; height: 40px; background-color: orange; }
             .pinned { position: fixed; left: 300px; top: 30px; width: 60px; height: 40px;
                       background-color: navy; }",
        );
        let root = fixture.doc.root;
        let card = fixture.el(root, "view.card");
        let mid = fixture.el(card, "view.mid");
        let pinned = fixture.el(mid, "view.pinned");
        fixture.animate(card, curve);
        fixture.probes = vec![card, mid, pinned];
        (fixture, card, pinned)
    };
    for curve in CURVES {
        let (mut probe, card, pinned) = fixture(curve);
        let dom = &mut probe.doc.dom;
        dom.render();
        dom.advance_animations(0.0);
        dom.advance_animations(EARLY);
        let frame = dom.build_paint_order();
        let item = frame
            .items()
            .iter()
            .find(|item| item.node == pinned)
            .expect("the fixed box paints");
        let rides = super::space::nearest_animation(frame.spaces(), item.space)
            .is_some_and(|slot| frame.animations()[slot as usize].node == card);
        assert_eq!(
            rides,
            curve != "opacity",
            "{curve}: the fixed box rides the card"
        );
        fixture(curve)
            .0
            .check(&format!("fixed box in an animated card, {curve}"));
    }
}

/// A card running `animation` beside `css`, composed at `later`.
fn card(css: &str, animation: &str, later: &[f64]) -> (Fixture, NodeId) {
    let mut fixture = Fixture::new(&format!(
        ".card {{ width: 120px; height: 80px; margin: 60px 0 0 80px; font-size: 20px;
                  background-color: teal; }} {css}"
    ));
    let root = fixture.doc.root;
    let card = fixture.el(root, "view.card");
    fixture
        .doc
        .set_inline(card, &format!("animation: {animation}"));
    fixture.probes = vec![card];
    fixture.later = later.to_vec();
    (fixture, card)
}

/// Mid-segment instants, keyframe and iteration boundaries of a 1 s curve.
const GRID: [f64; 7] = [0.25, 0.5, 0.75, 1.0, 1.375, 2.0, 2.625];

/// Two opacity animations: the later one in the set's order wins, as the
/// cascade inserts them.
#[test]
fn the_later_of_two_opacity_animations_wins_as_committed() {
    let (mut fixture, _) = card(
        "@keyframes dim { from { opacity: 0.4; } to { opacity: 0.9; } }",
        "opacity 1s linear infinite, dim 0.5s ease-in infinite",
        &GRID,
    );
    fixture.entries = Some(2);
    fixture.check("two opacity animations");
}

/// Timing functions stylo evaluates and the old exporter refused.
#[test]
fn step_and_square_bezier_easings_compose_as_committed() {
    for easing in ["steps(4, jump-both)", "square-bezier(0.3, 1.4)"] {
        let (fixture, _) = card("", &format!("translate 1s {easing} infinite"), &GRID);
        fixture.check(&format!("easing {easing}"));
    }
}

/// An interior keyframe with its own timing function: running forward, a
/// segment eases by its lower keyframe's function; running reversed, by its
/// upper keyframe's, so the overshooting ease moves from the second half to
/// the first on alternate iterations.
#[test]
fn an_interior_keyframe_easing_composes_as_committed() {
    for direction in ["normal", "alternate"] {
        let (fixture, _) = card(
            "@keyframes bend { from { transform: translate(0px, 0px); }
                              50% { transform: translate(60px, 20px);
                                    animation-timing-function: cubic-bezier(.2, 1.4, .6, -.4); }
                              to { transform: translate(120px, 40px); } }",
            &format!("bend 1s linear infinite {direction}"),
            &[0.25, 0.5, 0.625, 0.75, 1.125, 1.25, 1.5, 1.625, 1.875],
        );
        fixture.check(&format!("interior keyframe easing, {direction}"));
    }
}

/// `%` resolves against the border box and `em` against the font size the
/// keyframes were computed with.
#[test]
fn relative_lengths_compose_as_committed() {
    let (fixture, _) = card(
        "@keyframes shift { from { transform: translate(0px, 0px); }
                            to { transform: translate(50%, 1em); } }",
        "shift 1s linear infinite",
        &GRID,
    );
    fixture.check("% and em");
}

/// Lists stylo interpolates by matrix decomposition or by padding:
/// mismatched functions, `none` against a list, and `matrix()` keyframes.
#[test]
fn mismatched_and_matrix_lists_compose_as_committed() {
    for keyframes in [
        "from { transform: translateX(20px); } to { transform: rotate(90deg); }",
        "from { transform: none; } to { transform: translateX(100px) rotate(45deg); }",
        "from { transform: matrix(1, 0, 0, 1, 0, 0); }
         to { transform: matrix(1.2, 0.3, -0.2, 0.9, 40, 10); }",
    ] {
        let (fixture, _) = card(
            &format!("@keyframes swap {{ {keyframes} }}"),
            "swap 1s linear infinite",
            &GRID,
        );
        fixture.check(keyframes);
    }
}

/// A delayed animation filling backwards exports while still pending — the
/// driver anchored it — showing its first keyframe, not the base value,
/// until it starts.
#[test]
fn a_backwards_filled_delay_composes_as_committed() {
    let (mut fixture, _) = card(
        "@keyframes enter { from { transform: translate(30px, 10px); }
                            to { transform: translate(120px, 40px); } }",
        "enter 1s linear 0.5s infinite backwards",
        &[0.25, 0.5, 0.75, 1.5, 1.625],
    );
    fixture.entries = Some(1);
    fixture.check("backwards-filled delay");
}

/// A delayed animation without a backwards fill exports while pending too:
/// before its start nothing contributes, so the committed base value is
/// what both sides show; it runs once, and past its end the frame hands
/// back while the curve holds its last instant.
#[test]
fn an_unfilled_delay_composes_as_committed() {
    let (mut fixture, _) = card(
        "@keyframes enter { from { transform: translate(30px, 10px); }
                            to { transform: translate(120px, 40px); } }",
        "enter 1s linear 0.5s 1",
        &[0.25, 0.5, 0.75, 1.25, 1.5, 2.0],
    );
    fixture.entries = Some(1);
    fixture.check("unfilled delay");
}

/// An alternating curve crosses several iteration boundaries between the
/// commit and each sample.
#[test]
fn an_alternate_reverse_curve_crosses_iterations_as_committed() {
    let (fixture, _) = card(
        "",
        "rotate 0.25s ease-out infinite alternate-reverse",
        &[0.125, 0.25, 0.5, 0.625, 1.0, 1.1875, 2.25],
    );
    fixture.check("alternate-reverse");
}

/// A finished animation holding its end value beside a running one on the
/// same property: the driver puts the held one back last, so it wins. It
/// does so after the traversal of the tick the animation finishes on, whose
/// commit still takes the running one; a tick in between lets `early` commit
/// the order the curve clones.
#[test]
fn a_held_animation_beside_a_running_one_composes_as_committed() {
    let (mut fixture, _) = card(
        "@keyframes settle { from { transform: translateX(0px); }
                             to { transform: translateX(60px); } }",
        "settle 0.25s linear forwards, rotate 1s linear infinite",
        &[0.625, 1.0, 1.5],
    );
    fixture.ticks = vec![0.375];
    fixture.early = 0.5;
    fixture.entries = Some(2);
    fixture.check("held beside running");
}

/// A held `opacity` and a running `transform`: two animations contributing
/// different properties, both sampled.
#[test]
fn a_held_fade_beside_a_running_turn_composes_as_committed() {
    let (mut fixture, _) = card(
        "@keyframes dim { from { opacity: 1; } to { opacity: 0.4; } }",
        "dim 0.25s linear forwards, rotate 1s linear infinite",
        &[0.625, 1.0, 1.5],
    );
    fixture.early = 0.5;
    fixture.entries = Some(2);
    fixture.check("held fade beside running turn");
}

/// `animation-name: translate` restyled to `translate, opacity`: servo's
/// `maybe_start_animations` returns after updating the first, so `opacity`
/// never starts — on the main thread, and so in the curve.
#[test]
fn a_restyled_animation_list_mirrors_servo_as_committed() {
    let (mut fixture, card) = card("", "translate 1s linear infinite", &GRID);
    fixture.restyles.push((
        card,
        "animation: translate 1s linear infinite, opacity 1s linear infinite".into(),
    ));
    fixture.entries = Some(1);
    fixture.check("restyled list");
}

/// A paused animation beside a running one: the paused one holds its
/// progress on both sides.
#[test]
fn a_paused_animation_composes_as_committed() {
    let (mut fixture, _) = card(
        "",
        "translate 1s linear infinite paused, opacity 1s linear infinite running",
        &GRID,
    );
    fixture.entries = Some(2);
    fixture.check("paused beside running");
}

/// A card moving along an `offset-path` while its transform animates: the
/// motion-path sample is one of the fold's constant factors.
#[test]
fn a_transform_curve_on_a_motion_path_composes_as_committed() {
    for curve in ["translate", "rotate", "scale"] {
        let (fixture, _) = card(
            r#".card { offset-path: path("M 0 0 L 200 100"); offset-distance: 40%;
                       offset-rotate: auto; }"#,
            &format!("{curve} 1s linear infinite"),
            &GRID,
        );
        fixture.check(&format!("motion path, {curve}"));
    }
}

/// A planar curve on a child of a `perspective` parent: the parent's
/// perspective is a constant factor, and a planar list stays planar under it.
#[test]
fn a_planar_curve_under_a_perspective_parent_composes_as_committed() {
    for curve in CURVES {
        let mut fixture = Fixture::new(
            ".stage { width: 400px; height: 300px; margin: 40px 0 0 60px; perspective: 100px; }
             .card { width: 120px; height: 80px; margin: 40px 0 0 80px; background-color: teal; }",
        );
        let root = fixture.doc.root;
        let stage = fixture.el(root, "view.stage");
        let card = fixture.el(stage, "view.card");
        fixture.animate(card, curve);
        fixture.probes = vec![card];
        fixture.later = GRID.to_vec();
        fixture.check(&format!("under perspective, {curve}"));
    }
}

/// A `transform` transition started by a restyle: sampled through
/// `Transition::calculate_value`, it composes as committed until its end,
/// where the frame hands back. Its reach runs from its `from` to its `to`
/// over its timing function's eased range, and the coverage check holds
/// over the whole viewport.
#[test]
fn a_transform_transition_composes_as_committed() {
    let fixture = || {
        let (mut fixture, card) = card(
            ".card { transition: transform 1s ease-in-out; }",
            "none",
            &[0.25, 0.5, 0.75, 0.9375, 1.25],
        );
        fixture.restyles.push((
            card,
            "transform: translate(120px, 40px) rotate(30deg);".into(),
        ));
        fixture.entries = Some(0);
        fixture
    };
    let frame = commit_early(&mut fixture());
    let curve = &frame.animation_slots()[0].curve;
    assert_eq!(curve.transitions.len(), 1, "the transition exports");
    let track = curve.transform.as_ref().expect("a transform track");
    assert!(track.reach.is_bounded(), "a transition has a reach");
    assert_eq!(curve.expires_at, Some(1.0), "it hands back at its end");
    fixture().check("transform transition");
}

/// An `opacity` transition beside a `transform` animation on one element:
/// both export in one curve, each property from its own entry.
#[test]
fn an_opacity_transition_beside_a_transform_animation_composes_as_committed() {
    let (mut fixture, card) = card(
        ".card { transition: opacity 1s linear; }",
        "rotate 1s linear infinite",
        &[0.25, 0.5, 0.75, 1.25],
    );
    fixture.restyles.push((
        card,
        "opacity: 0.3; animation: rotate 1s linear infinite;".into(),
    ));
    fixture.entries = Some(1);
    fixture.check("opacity transition beside a transform animation");
}

/// A transition in its delay exports once a frame anchors its start: the
/// anchoring owes that frame a commit although no style moves until the
/// delay ends, and the commit hands the element to the painter.
#[test]
fn a_delayed_transition_exports_once_anchored() {
    let (mut fixture, card) = card(
        ".card { transform: translate(0px, 0px); transition: transform 1s linear 0.5s; }",
        "none",
        &[],
    );
    let dom = &mut fixture.doc.dom;
    dom.render();
    dom.advance_animations(0.0);
    dom.set_inline_style(card, "transform: translate(120px, 40px);");
    let fresh = dom.commit();
    assert!(
        fresh.animation_slots().is_empty() && fresh.needs_main_ticks(),
        "a fresh start does not export"
    );
    dom.advance_animations(0.1);
    assert!(dom.needs_render(), "the anchoring owes a commit");
    let anchored = dom.commit();
    assert_eq!(
        anchored.animation_slots().len(),
        1,
        "it exports in its delay"
    );
    assert!(!anchored.needs_main_ticks(), "and main stops ticking");
}

/// The value `frame`'s first curve samples for `property` at `t`: what the
/// painter shows then.
fn painted(frame: &CommittedFrame, property: LonghandId, t: f64) -> AnimationValue {
    let mut values = AnimationValueMap::default();
    frame.animation_slots()[0]
        .curve
        .values_at(Some(t), &frame.committed_offsets(), &mut values);
    values
        .get(&OwnedPropertyDeclarationId::Longhand(property))
        .cloned()
        .expect("the curve samples the property")
}

/// A tap mid-flight whose listener restyles the card back: the job that
/// runs it syncs the main thread's clock to the painter's first, so the
/// reversed transition starts from the value the painter showed at that
/// instant, shortened by the factor css-transitions-1 §3 computes — the
/// progress the old one had reached — rather than from the last tick's. The
/// next frame anchors it, and the export resumes from that value.
#[test]
#[expect(
    clippy::float_cmp,
    reason = "a linear progress at a binary fraction is exact"
)]
fn a_retarget_mid_flight_reverses_from_the_painted_value() {
    let (mut fixture, card) = card(
        ".card { transform: translate(0px, 0px); transition: transform 1s linear; }",
        "none",
        &[],
    );
    fixture
        .restyles
        .push((card, "transform: translate(120px, 40px);".into()));
    let early = commit_early(&mut fixture);
    assert_eq!(early.animation_slots().len(), 1, "the transition exports");
    let tap = 0.625;
    let shown = painted(&early, LonghandId::Transform, tap);

    let dom = &mut fixture.doc.dom;
    dom.sync_animation_clock(tap);
    dom.set_inline_style(card, "transform: translate(0px, 0px);");
    let late = dom.commit();
    let handle = dom.animations().context_handle();
    let sets = handle.sets.read();
    let key = stylo::servo::animation::AnimationSetKey::new_for_non_pseudo(stylo::dom::OpaqueNode(
        card.arena_key(),
    ));
    let reversed = sets
        .get(&key)
        .and_then(|set| {
            set.transitions.iter().find(|transition| {
                transition.state != stylo::servo::animation::AnimationState::Canceled
            })
        })
        .expect("a reversed transition runs");
    assert_eq!(
        reversed.reversing_shortening_factor, tap,
        "shortened by the progress"
    );
    assert_eq!(reversed.start_time, tap);
    assert_eq!(reversed.property_animation.duration, tap);
    assert_eq!(
        reversed.calculate_value(tap),
        shown,
        "from the painted value"
    );
    drop(sets);
    // Pending until a frame anchors its start, as any new transition is.
    assert!(late.animation_slots().is_empty() && late.needs_main_ticks());
    let frame = tap + 0.0625;
    dom.advance_animations(frame);
    let anchored = dom.commit();
    assert_eq!(
        anchored.animation_slots().len(),
        1,
        "the reversed one exports"
    );
    assert_eq!(painted(&anchored, LonghandId::Transform, frame), shown);
}

/// `animation-play-state: paused` applied by a restyle while a curve covers
/// the element holds the progress the painter had reached, not the progress
/// at the last tick.
#[test]
fn a_pause_mid_flight_holds_the_painted_progress() {
    let (mut fixture, card) = card("", "translate 1s linear infinite", &[]);
    let early = commit_early(&mut fixture);
    assert_eq!(early.animation_slots().len(), 1, "the animation exports");
    let pause = 0.375;
    let AnimationValue::Transform(shown) = painted(&early, LonghandId::Transform, pause) else {
        unreachable!("a transform curve samples a transform list");
    };

    let dom = &mut fixture.doc.dom;
    dom.sync_animation_clock(pause);
    dom.set_inline_style(card, "animation: translate 1s linear infinite paused");
    dom.render();
    let committed = dom.paint_style(card).expect("the card is styled");
    assert_eq!(
        committed.get_box().transform,
        shown,
        "paused where it was painted"
    );
    let later = dom.commit();
    let AnimationValue::Transform(held) = painted(&later, LonghandId::Transform, 0.875) else {
        unreachable!("a transform curve samples a transform list");
    };
    assert_eq!(held, shown, "and it holds there");
}

/// Commits `fixture` at [`EARLY`] after its restyles, without checking it.
fn commit_early(fixture: &mut Fixture) -> std::sync::Arc<CommittedFrame> {
    let dom = &mut fixture.doc.dom;
    dom.render();
    dom.advance_animations(0.0);
    for (node, css) in std::mem::take(&mut fixture.restyles) {
        dom.set_inline_style(node, &css);
        dom.render();
        dom.advance_animations(0.0);
    }
    assert!(dom.advance_animations(EARLY).needs_next_frame);
    dom.commit()
}

/// What the painter could not reproduce stays on the main thread: an
/// `!important` transform the animation cannot move, a transition on a
/// property other than `opacity` and `transform`, and keyframes some
/// interpolation takes out of the plane: `matrix3d` with a perspective term,
/// and `rotateX` under a perspective parent.
#[test]
fn what_the_painter_cannot_reproduce_refuses_the_export() {
    let keyframes = "@keyframes persp {
                         from { transform: matrix3d(1,0,0,0, 0,1,0,0, 0,0,1,-0.002, 0,0,0,1); }
                         to { transform: matrix3d(1,0,0,0, 0,1,0,0, 0,0,1,-0.004, 0,0,0,1); } }
                     @keyframes tilt { from { transform: rotateX(0deg); }
                                       to { transform: rotateX(40deg); } }";
    for (css, animation, restyle) in [
        (
            ".card { transform: translateY(5px) !important; }",
            "translate 1s linear infinite",
            None,
        ),
        (
            ".card { transition: width 1s linear, transform 1s linear; }",
            "translate 1s linear infinite",
            Some("animation: translate 1s linear infinite; width: 200px;"),
        ),
        ("", "persp 1s linear infinite", None),
        (
            ".stage { perspective: 100px; }",
            "tilt 1s linear infinite",
            None,
        ),
    ] {
        let mut fixture = Fixture::new(&format!(
            ".stage {{ width: 400px; height: 300px; }}
             .card {{ width: 120px; height: 80px; margin: 60px 0 0 80px;
                      background-color: teal; }} {keyframes} {css}"
        ));
        let root = fixture.doc.root;
        let stage = fixture.el(root, "view.stage");
        let card = fixture.el(stage, "view.card");
        fixture
            .doc
            .set_inline(card, &format!("animation: {animation}"));
        fixture
            .restyles
            .extend(restyle.map(|css| (card, css.to_owned())));
        let frame = commit_early(&mut fixture);
        assert!(
            frame.animation_slots().is_empty(),
            "{css} {animation}: refused"
        );
        assert!(
            frame.needs_main_ticks(),
            "{css} {animation}: ticks on the main thread"
        );
    }
}

/// A row sheared by `skewX` keyframes has no reach, so the fading group
/// holding it cannot bound it: the row stays on the main thread while the
/// group's own fade exports.
#[test]
fn a_row_without_a_reach_inside_a_fading_group_refuses_the_export() {
    let mut fixture = Fixture::new(
        ".group { width: 300px; height: 200px; margin: 60px 0 0 80px; padding: 20px;
                  background-color: navy; }
         .row { width: 200px; height: 40px; background-color: teal; }
         @keyframes shear { from { transform: skewX(0deg); } to { transform: skewX(20deg); } }",
    );
    let root = fixture.doc.root;
    let group = fixture.el(root, "view.group");
    let row = fixture.el(group, "view.row");
    fixture.animate(group, "opacity");
    fixture.animate(row, "shear");
    let frame = commit_early(&mut fixture);
    let exported: Vec<NodeId> = frame
        .animation_slots()
        .iter()
        .map(|slot| slot.node)
        .collect();
    assert_eq!(exported, [group], "only the group's fade exports");
    assert!(frame.needs_main_ticks(), "the row ticks on the main thread");
}

/// A 300 px scroller holding a `.lead` when `lead`, then a card animating
/// `curve` with `fill` on `timeline`, then a `.filler`.
fn scroll_fixture(
    extra: &str,
    lead: bool,
    (curve, fill): (&str, &str),
    timeline: &str,
) -> (Fixture, NodeId, NodeId) {
    let mut fixture = Fixture::new(&format!(
        ".scroller {{ overflow: scroll; flex-direction: column; flex-shrink: 0;
                      width: 300px; height: 300px; margin: 40px 0 0 60px; }}
         .card {{ flex-shrink: 0; width: 120px; height: 80px; margin: 20px;
                  background-color: teal; }}
         .filler {{ flex-shrink: 0; width: 280px; background-color: navy; }}
         {extra}"
    ));
    let root = fixture.doc.root;
    let scroller = fixture.el(root, "view.scroller");
    if lead {
        fixture.el(scroller, "view.lead");
    }
    let card = fixture.el(scroller, "view.card");
    fixture.el(scroller, "view.filler");
    fixture.doc.set_inline(
        card,
        &format!("animation: {curve} 1s linear {fill}; {timeline}"),
    );
    fixture.probes = vec![card];
    (fixture, scroller, card)
}

/// `scroll()`: the card rides its scroller and animates across the whole
/// scroll range — at the range's start, inside it, and at its end, which is
/// the scroll limit.
#[test]
fn a_scroll_timeline_composes_as_committed() {
    for curve in CURVES {
        // 200 px of lead, 120 px of card and 260 px of filler: a 280 px
        // scroll range, over which some of the card stays in view.
        let (fixture, scroller, _) = scroll_fixture(
            ".lead { flex-shrink: 0; width: 280px; height: 200px; }
             .filler { height: 260px; }",
            true,
            (curve, "both"),
            "animation-timeline: scroll()",
        );
        fixture.check_scroll(
            &format!("scroll(), {curve}"),
            scroller,
            100.0,
            &[70.0, 0.0, 280.0, 123.25],
        );
    }
}

/// `view()` on the card over its `entry` range, 80 px to 160 px of a 300 px
/// scroll range: before it, exactly at its start and end, inside it, past
/// it, and at the scroll limit.
#[test]
fn a_view_timeline_composes_as_committed() {
    for curve in CURVES {
        // The card's border box spans 380 px to 460 px of 600 px content,
        // out of view before its range; the sweep reaches the scroller.
        let (mut fixture, scroller, _) = scroll_fixture(
            ".filler { height: 120px; }
             .lead { flex-shrink: 0; width: 280px; height: 360px; }",
            true,
            (curve, "both"),
            "animation-timeline: view(); animation-range: entry",
        );
        fixture.probes = vec![scroller];
        fixture.check_scroll(
            &format!("view(), {curve}"),
            scroller,
            120.0,
            &[40.0, 80.0, 160.0, 101.5, 230.0, 300.0],
        );
    }
}

/// The same `view()` range with no fill, committed before it, where the
/// cascade has no effect: the curve contributes exactly where the cascade
/// does — at the range's start and inside it, not before it, at its end, or
/// past it.
#[test]
fn a_view_timeline_without_a_fill_composes_its_presence_as_committed() {
    for curve in CURVES {
        let (mut fixture, scroller, _) = scroll_fixture(
            ".filler { height: 120px; }
             .lead { flex-shrink: 0; width: 280px; height: 360px; }",
            true,
            (curve, "none"),
            "animation-timeline: view(); animation-range: entry",
        );
        fixture.probes = vec![scroller];
        fixture.check_scroll(
            &format!("view() without a fill, {curve}"),
            scroller,
            40.0,
            &[0.0, 79.5, 80.0, 101.5, 160.0, 230.0, 300.0, 40.0],
        );
    }
}

/// A named scroll timeline on a list painted after the header it drives:
/// its slot is allocated after the header's animation slot, and the header,
/// outside the list, does not move with it.
#[test]
fn a_named_timeline_on_a_later_painted_list_composes_as_committed() {
    let fixture = |curve: &str| {
        let mut fixture = Fixture::new(
            ".header { flex-shrink: 0; width: 200px; height: 60px; margin: 20px 0 0 60px;
                       background-color: teal; }
             .list { overflow: scroll; flex-direction: column; flex-shrink: 0; z-index: 1;
                     width: 300px; height: 300px; margin: 20px 0 0 60px;
                     scroll-timeline: --list; }
             .row { flex-shrink: 0; width: 280px; height: 50px; margin-bottom: 10px;
                    background-color: navy; }",
        );
        let root = fixture.doc.root;
        let header = fixture.el(root, "view.header");
        let list = fixture.el(root, "view.list");
        for _ in 0..10 {
            fixture.el(list, "view.row");
        }
        fixture.doc.set_inline(
            header,
            &format!("animation: {curve} 1s linear both; animation-timeline: --list"),
        );
        fixture.probes = vec![header];
        (fixture, list)
    };
    for curve in CURVES {
        let (mut probe, list) = fixture(curve);
        let frame = probe.doc.dom.commit();
        let [slot] = frame.animation_slots() else {
            panic!("{curve}: the header exports");
        };
        let Timeline::Scroll(scroll) = &slot.curve.animations[0].1 else {
            panic!("{curve}: the header reads the list");
        };
        let bound = scroll.slot.expect("the list has a slot");
        assert_eq!(frame.scroll_slots()[bound as usize].node, list);
        let space = |kind: SpaceKind| {
            frame
                .order
                .spaces()
                .iter()
                .position(|space| space.kind == kind)
                .expect("the slot has a space")
        };
        assert!(
            space(SpaceKind::Animation(0)) < space(SpaceKind::Scroll(bound)),
            "{curve}: the list is allocated after the header"
        );
        let (fixture, list) = fixture(curve);
        fixture.check_scroll(
            &format!("named timeline, {curve}"),
            list,
            100.0,
            &[150.0, 0.0, 300.0, 37.5],
        );
    }
}
