//! Pixel equivalence of the layered path: a frame drawn from its retained
//! plane textures shows the same interiors as the same frame composed flat,
//! at the committed offset and at in-window scroll overrides. Interiors
//! only — the two paths rasterize edges independently, and the layered one
//! quantizes through an 8-bit texture, so edge pixels may differ by a bit
//! while solid interiors may not. A usable GPU adapter is mandatory.

mod paint_common;

use std::sync::Arc;

use dom::render::gpu::Headless;
use dom::vello::Scene;
use dom::vello::peniko::Color;
use dom::{CommittedFrame, Vector2D};
use flashbulb::headless;
use paint_common::Doc;

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A 100x100 scroller over a red then a blue 100px row, on a 200x150 page,
/// so pixels beside the scroller prove the scrollport clip holds.
fn scrolling_doc() -> (Doc, dom::NodeId) {
    let mut doc = Doc::with_css(
        "page { display: flex; width: 200px; height: 150px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .red, .blue { display: flex; flex-shrink: 0; width: 100px; height: 100px; }
         .red { background-color: #ff0000; }
         .blue { background-color: #0000ff; }",
    );
    let root = doc.root;
    let scroller = doc.el(root, "scroller");
    let red = doc.el(scroller, "red");
    doc.el(scroller, "blue");
    (doc, red)
}

/// One composite render, prepared the way a target renders every frame:
/// planes brought up to the commit, then the plan composed and rendered.
fn render_layered(gpu: &mut Headless, frame: &Arc<CommittedFrame>, offset: f32) -> Vec<u8> {
    gpu.prepare_planes(frame, &dom::NoImages, 0)
        .expect("plane bake");
    let offsets = move |_: &dom::ScrollSlot| Some(Vector2D::new(0.0, offset));
    let mut layered = Scene::new();
    frame.composite_into(
        &mut layered,
        gpu.plane_images(),
        &dom::NoImages,
        &offsets,
        None,
    );
    gpu.render(&layered, 200, 150, Color::WHITE)
        .expect("layered render")
}

#[test]
fn a_layered_frame_matches_the_flat_composition() {
    let mut gpu = headless("a_layered_frame_matches_the_flat_composition");
    let (mut doc, _) = scrolling_doc();
    let frame = doc.dom.commit();
    assert!(
        frame.composite_plan().is_some(),
        "scroller content must layer"
    );

    for offset in [0.0_f32, 30.0, 100.0] {
        let offsets = move |_: &dom::ScrollSlot| Some(Vector2D::new(0.0, offset));
        let mut flat = Scene::new();
        frame.compose_into(&mut flat, &dom::NoImages, &offsets, None);
        let expected = gpu.render(&flat, 200, 150, Color::WHITE).expect("flat");

        let composed = render_layered(&mut gpu, &frame, offset);

        // Row interiors inside the scrollport, a point past the scrollport
        // on both axes, and a point inside the port near its bottom edge.
        for (x, y) in [(50, 20), (50, 80), (150, 50), (50, 120), (10, 95)] {
            assert_eq!(
                pixel(&composed, 200, x, y),
                pixel(&expected, 200, x, y),
                "offset {offset}: ({x}, {y}) diverges between the paths"
            );
        }
    }

    // Scrolled fully, blue shows through the port and nothing leaks past it.
    let composed = render_layered(&mut gpu, &frame, 100.0);
    assert_eq!(pixel(&composed, 200, 50, 50), BLUE);
    assert_eq!(pixel(&composed, 200, 150, 50), WHITE, "beside the port");
    assert_eq!(pixel(&composed, 200, 50, 120), WHITE, "below the port");
}

/// A commit re-bakes: after the content changes color, the planes show the
/// new pixels without reconstructing the bank.
#[test]
fn a_new_commit_rebakes_the_planes() {
    let mut gpu = headless("a_new_commit_rebakes_the_planes");
    let (mut doc, red) = scrolling_doc();
    let frame = doc.dom.commit();
    let _ = render_layered(&mut gpu, &frame, 0.0);

    doc.dom.set_inline_style(red, "background-color: #0000ff");
    let frame = doc.dom.commit();
    let composed = render_layered(&mut gpu, &frame, 0.0);
    assert_eq!(pixel(&composed, 200, 50, 20), BLUE, "the re-baked row");
}
