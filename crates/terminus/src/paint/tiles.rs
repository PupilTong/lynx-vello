//! Vello's tile demand for a committed scene, estimated on the CPU the way
//! its `binning` and `tile_alloc` stages count tiles: every draw object and
//! every clip layer costs the 16×16 px tiles of its path's bounding box, cut
//! to the enclosing clips and the viewport; an end-clip costs none.
//!
//! The renderer's bump buffers are fixed-size — `vello_encoding::BufferSizes`
//! hand-picks `1 << 21` tiles — and a frame over the budget sets
//! `bump.failed`, after which every later stage exits and the frame is
//! transparent. No error reaches the CPU. These tests hold the walker's
//! layer discipline to budgets a page can actually meet.

#![allow(
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "tile counts and grid positions are small, exact integers"
)]

use vello::kurbo::Rect;
use vello::low_level::vello_encoding::{DrawTag, Encoding, PathTag};

use crate::render::image::NoImages;
use crate::test_common::{Doc, device_with};

/// `vello_encoding::BufferSizes::new`'s `tiles`.
const VELLO_TILE_BUDGET: u64 = 1 << 21;
const TILE: f64 = 16.0;

fn tiles(bbox: Rect, width: f64, height: f64) -> u64 {
    if !(bbox.x0 < bbox.x1 && bbox.y0 < bbox.y1) {
        return 0;
    }
    let wt = (width / TILE).ceil();
    let ht = (height / TILE).ceil();
    let x0 = (bbox.x0 / TILE).floor().clamp(0.0, wt);
    let y0 = (bbox.y0 / TILE).floor().clamp(0.0, ht);
    let x1 = (bbox.x1 / TILE).ceil().clamp(0.0, wt);
    let y1 = (bbox.y1 / TILE).ceil().clamp(0.0, ht);
    ((x1 - x0) * (y1 - y0)) as u64
}

/// Per path, its device bounding box, in path order: the stream decoded as
/// `PathMonoid::new` counts it — a segment tag's low two bits are its point
/// count, plus one on the subpath-end segment, each point two words of f32.
fn path_bboxes(enc: &Encoding) -> Vec<Option<Rect>> {
    let mut out = Vec::new();
    let mut data_ix = 0_usize;
    let mut trans_ix = 0_usize;
    let mut bbox: Option<Rect> = None;
    for tag in &enc.path_tags {
        let t = tag.0;
        if t & PathTag::TRANSFORM.0 != 0 {
            trans_ix += 1;
        }
        let seg = t & 0x3;
        if seg != 0 {
            let n_points = (seg + ((t >> 2) & 1)) as usize;
            let is_f32 = t & 0x8 != 0;
            let tf = &enc.transforms[trans_ix - 1];
            for p in 0..n_points {
                let (x, y) = if is_f32 {
                    (
                        f32::from_bits(enc.path_data[data_ix + 2 * p]),
                        f32::from_bits(enc.path_data[data_ix + 2 * p + 1]),
                    )
                } else {
                    let w = enc.path_data[data_ix + p];
                    ((w & 0xffff) as i16 as f32, (w >> 16) as i16 as f32)
                };
                let dx = tf.matrix[0] * x + tf.matrix[2] * y + tf.translation[0];
                let dy = tf.matrix[1] * x + tf.matrix[3] * y + tf.translation[1];
                let (dx, dy) = (f64::from(dx), f64::from(dy));
                bbox = Some(match bbox {
                    None => Rect::new(dx, dy, dx, dy),
                    Some(r) => Rect::new(r.x0.min(dx), r.y0.min(dy), r.x1.max(dx), r.y1.max(dy)),
                });
            }
            data_ix += n_points * if is_f32 { 2 } else { 1 };
        }
        if t & PathTag::PATH.0 != 0 {
            out.push(bbox.take());
        }
    }
    out
}

#[derive(Debug, Default)]
struct Demand {
    total: u64,
    /// Per begin-clip, its tile count.
    clips: Vec<u64>,
}

fn tile_demand(enc: &Encoding, width: f64, height: f64) -> Demand {
    let bboxes = path_bboxes(enc);
    assert_eq!(
        bboxes.len(),
        enc.draw_tags.len(),
        "one path per draw object"
    );
    let everything = Rect::new(-1e9, -1e9, 1e9, 1e9);
    let mut stack: Vec<Rect> = vec![];
    let mut demand = Demand::default();
    for (tag, bbox) in enc.draw_tags.iter().zip(bboxes) {
        let enclosing = stack.last().copied().unwrap_or(everything);
        let own = bbox.unwrap_or(Rect::ZERO).intersect(enclosing);
        if *tag == DrawTag::BEGIN_CLIP {
            let n = tiles(own, width, height);
            stack.push(own);
            demand.clips.push(n);
            demand.total += n;
        } else if *tag == DrawTag::END_CLIP {
            stack.pop();
        } else if *tag != DrawTag::NOP {
            demand.total += tiles(own, width, height);
        }
    }
    demand
}

const PAGE: (f32, f32) = (400.0, 800.0);
const RATIO: f32 = 2.0;
const COLUMNS: usize = 25;

/// The Explorer's "Image animations" benchmark, as boxes: `count` boxes of
/// a 370 px row of 25, absolutely positioned in one page-sized Lynx `view`
/// — which clips and contains them, as a Lynx `view` does — each carrying
/// one of `effects` in turn.
fn grid(count: usize, effects: &[&str], css: &str) -> Doc {
    let mut doc = Doc::with_device(device_with(
        PAGE.0,
        PAGE.1,
        RATIO,
        stylo::queries::values::PrefersColorScheme::Light,
    ));
    doc.add_css(&format!(
        "page {{ display: flex; position: relative; width: {}px; height: {}px; }}
         .app {{ display: flex; position: relative; width: 100%; height: 100%;
                overflow: hidden; }}
         .grid {{ display: relative; position: relative; width: 370px; height: 100%; }}
         .box {{ display: flex; position: absolute; width: 14.8px; height: 14.8px;
                background-color: teal; }}
         {css}",
        PAGE.0, PAGE.1
    ));
    let root = doc.root;
    let app = doc.el(root, "view.app");
    let grid = doc.el(app, "view.grid");
    for index in 0..count {
        let left = (index % COLUMNS) as f32 * 14.8;
        let top = (index / COLUMNS) as f32 * 14.8;
        let item = doc.el(grid, "view.box");
        doc.set_inline(
            item,
            &format!(
                "left: {left}px; top: {top}px; {}",
                effects[index % effects.len()]
            ),
        );
    }
    doc
}

fn demand_of(doc: &mut Doc) -> (Demand, u64) {
    doc.dom.render();
    let scene = doc.dom.scene(&NoImages);
    let (w, h) = (f64::from(PAGE.0 * RATIO), f64::from(PAGE.1 * RATIO));
    (
        tile_demand(scene.encoding(), w, h),
        tiles(Rect::new(0.0, 0.0, w, h), w, h),
    )
}

/// Every third box fades: a group scope that restarts the clip chain around
/// it, so the page's clip is pushed once per run of boxes. Each push is cut
/// to the run it holds, so the whole frame costs a few tiles per box, not a
/// viewport of tiles per group.
#[test]
fn interleaved_groups_in_one_clip_cost_their_content_not_the_viewport() {
    let mut doc = grid(300, &["", "", "opacity: 0.5;"], "");
    let (demand, viewport) = demand_of(&mut doc);
    assert_eq!(
        demand.clips.len(),
        300,
        "per fading box: the run before it, its effect layer, and its own chain inside"
    );
    // The one link holding the page-sized grid box costs a viewport; every
    // other push holds a run of boxes and costs that run.
    let costly = demand.clips.iter().filter(|&&n| n * 8 >= viewport).count();
    assert_eq!(costly, 1, "{:?}", demand.clips);
    assert!(
        demand.total < viewport + 300 * 24,
        "{} tiles for 300 boxes in a {viewport}-tile viewport",
        demand.total
    );
}

/// The benchmark itself: 1,000 boxes rotating, scaling and fading by
/// exported curves. Before the pushed links were cut to what they hold, this
/// page cost a viewport of tiles for every fading box — 1.5 million at this
/// size, past vello's budget on a larger window — and rendered as a
/// transparent frame.
#[test]
fn the_image_animation_benchmark_fits_vellos_tile_budget_with_room() {
    let mut doc = grid(
        1000,
        &[
            "animation: spin 1s infinite;",
            "animation: grow 1s infinite;",
            "animation: fade 1s infinite;",
        ],
        "@keyframes fade { 0% { opacity: 0.1; } 100% { opacity: 1; } }
         @keyframes grow { 0% { transform: scale(0.1, 0.1); } 100% { transform: scale(1, 1); } }
         @keyframes spin { 0% { transform: rotateZ(0); } 100% { transform: rotateZ(360deg); } }",
    );
    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(0.25);
    let frame = doc.dom.build_paint_order();
    assert_eq!(frame.animations().len(), 1000, "every curve exports");
    let (demand, viewport) = demand_of(&mut doc);
    let costly = demand.clips.iter().filter(|&&n| n * 8 >= viewport).count();
    assert_eq!(costly, 1, "only the grid box's link costs a viewport");
    assert!(
        demand.total * 16 < VELLO_TILE_BUDGET,
        "{} tiles of {VELLO_TILE_BUDGET}",
        demand.total
    );
}
