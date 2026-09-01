//! End-to-end pixel tests: `Document` → `PaintOrder` → `Scene` → GPU →
//! readback. Ahem glyphs are solid em squares, so glyph coverage is
//! pixel-assertable. A usable GPU adapter is mandatory.

mod paint_common;

use dom::vello::Scene;
use dom::vello::kurbo::{Affine, Rect};
use dom::vello::peniko::{BlendMode, Color, Compose, Fill, Mix};
use flashbulb::headless;
use paint_common::Doc;

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");
const ISOLATION_ATLAS_WIDTH: u32 = 384;
const ISOLATION_ATLAS_HEIGHT: u32 = 192;
const ISOLATION_CELL_X: u32 = 128;
const ISOLATION_CELL_Y: u32 = 32;

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

#[test]
fn background_clip_text_clips_to_glyph_ink() {
    let mut gpu = headless("background_clip_text_clips_to_glyph_ink");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { display: flex; position: absolute; left: 10px; top: 10px;
                width: 180px; height: 50px;
                font-family: Ahem; font-size: 20px; color: transparent;
                background-color: rebeccapurple; background-clip: text; }";
    let mut doc = Doc::with_css(css);
    doc.dom.register_fonts(dom::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el(root, "text");
    doc.text(holder, "HH HH");

    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let ink = pixel(&pixels, 200, 30, 20);
    assert!(
        ink[0] > 80 && ink[0] < 130 && ink[2] > 120,
        "glyph ink must show the background ({ink:?})"
    );
    let gap = pixel(&pixels, 200, 60, 20);
    assert_eq!(gap, [255, 255, 255, 255], "space must stay unpainted");
    let below = pixel(&pixels, 200, 30, 50);
    assert_eq!(
        below,
        [255, 255, 255, 255],
        "un-inked box must stay unpainted"
    );
}

#[test]
fn plain_background_covers_the_box() {
    let mut gpu = headless("plain_background_covers_the_box");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { display: flex; position: absolute; left: 10px; top: 10px;
                width: 180px; height: 50px;
                font-family: Ahem; font-size: 20px; color: black;
                background-color: rebeccapurple; }";
    let mut doc = Doc::with_css(css);
    doc.dom.register_fonts(dom::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el(root, "text");
    doc.text(holder, "HH HH");

    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let gap = pixel(&pixels, 200, 60, 40);
    assert!(
        gap[0] > 80 && gap[0] < 130 && gap[2] > 120,
        "without clip-text the box paints everywhere ({gap:?})"
    );
}

#[test]
fn gradient_color_fills_glyph_ink_from_the_padding_box() {
    let mut gpu = headless("gradient_color_fills_glyph_ink_from_the_padding_box");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { display: flex; position: absolute; left: 0px; top: 10px;
                width: 120px; height: 50px; box-sizing: border-box;
                border-left: 20px solid black;
                font-family: Ahem; font-size: 20px;
                color: linear-gradient(90deg, #ff0000, #0000ff); }";
    let mut doc = Doc::with_css(css);
    doc.dom.register_fonts(dom::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el(root, "text");
    doc.text(holder, "HH");

    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let first = pixel(&pixels, 200, 30, 20);
    assert!(
        (215..=245).contains(&first[0]) && first[2] < 45,
        "first glyph must sit ~10% along the ramp ({first:?})"
    );
    let second = pixel(&pixels, 200, 50, 20);
    assert!(
        (164..=194).contains(&second[0]) && second[2] > 60,
        "second glyph must sit ~30% along the ramp ({second:?})"
    );
    assert!(
        first[0] > second[0] + 30,
        "red must fall across the ramp ({first:?} then {second:?})"
    );
}

#[test]
fn outline_rings_the_border_box() {
    let mut gpu = headless("outline_rings_the_border_box");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .out { display: flex; position: absolute; left: 20px; top: 20px;
               width: 100px; height: 50px;
               background-color: teal; outline: 5px solid red; }";
    let mut doc = Doc::with_css(css);
    let root = doc.root;
    doc.el(root, "out");

    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let ring = pixel(&pixels, 200, 17, 45);
    assert!(
        ring[0] > 200 && ring[1] < 60 && ring[2] < 60,
        "outline ring must be red ({ring:?})"
    );
    let inside = pixel(&pixels, 200, 60, 45);
    assert!(
        inside[0] < 60 && inside[1] > 90 && inside[1] < 160,
        "box interior keeps its teal background ({inside:?})"
    );
    let outside = pixel(&pixels, 200, 10, 45);
    assert_eq!(outside, [255, 255, 255, 255]);
}

#[test]
fn isolated_atlas_cell_matches_standalone_group_effects() {
    let mut gpu = headless("isolated_atlas_cell_matches_standalone_group_effects");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
        .effect { display: flex; position: absolute; left: 14px; top: 14px;
                  width: 100px; height: 100px;
                  background: linear-gradient(135deg, red, lime, blue);
                  box-shadow: 10px 8px 6px rgb(124 58 237 / 80%);
                  opacity: .72; filter: brightness(.7);
                  clip-path: inset(3px round 8px);
                  mask-image: linear-gradient(0deg, black 0%, transparent 100%);
                  mask-repeat: no-repeat; }";
    let mut doc = Doc::with_css(css);
    let root = doc.root;
    doc.el(root, "effect");

    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    let standalone = gpu
        .render(&scene, 128, 128, Color::WHITE)
        .expect("standalone headless render");

    let cell_x = f64::from(ISOLATION_CELL_X);
    let cell_y = f64::from(ISOLATION_CELL_Y);
    let left_neighbor = Rect::new(0.0, cell_y, cell_x, cell_y + 128.0);
    let cell = Rect::new(cell_x, cell_y, cell_x + 128.0, cell_y + 128.0);
    let right_neighbor = Rect::new(cell_x + 128.0, cell_y, 384.0, cell_y + 128.0);
    let mut atlas = Scene::new();
    atlas.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(8, 145, 178),
        None,
        &left_neighbor,
    );
    atlas.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(219, 39, 119),
        None,
        &right_neighbor,
    );
    atlas.push_layer(
        Fill::NonZero,
        BlendMode::new(Mix::Normal, Compose::SrcOver),
        1.0,
        Affine::IDENTITY,
        &cell,
    );
    atlas.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &cell);
    atlas.append(&scene, Some(Affine::translate((cell_x, cell_y))));
    atlas.pop_layer();
    let appended = gpu
        .render(
            &atlas,
            ISOLATION_ATLAS_WIDTH,
            ISOLATION_ATLAS_HEIGHT,
            Color::WHITE,
        )
        .expect("atlas headless render");

    let mut cropped = Vec::with_capacity(standalone.len());
    for row in ISOLATION_CELL_Y..ISOLATION_CELL_Y + 128 {
        let start = ((row * ISOLATION_ATLAS_WIDTH + ISOLATION_CELL_X) * 4) as usize;
        cropped.extend_from_slice(&appended[start..start + 128 * 4]);
    }
    assert_eq!(
        standalone, cropped,
        "translated atlas cell changed its scene"
    );
    assert_eq!(
        pixel(&appended, ISOLATION_ATLAS_WIDTH, 120, 96),
        [8, 145, 178, 255],
        "outset effects leaked into the left neighbor"
    );
    assert_eq!(
        pixel(&appended, ISOLATION_ATLAS_WIDTH, 264, 96),
        [219, 39, 119, 255],
        "outset effects leaked into the right neighbor"
    );
}

/// An intervening image-free render costs the atlas its contents. This
/// test currently FAILS, and it documents a live defect rather than a
/// migration hazard.
///
/// Mechanism, confirmed in vello 0.9.0's own source: an encoding with no
/// patches at all — solid paths only, so no image, no gradient ramp and no
/// glyph run — takes `Resolver::resolve`'s early return and reports
/// `Images::default()` (`vello_encoding/src/resolve.rs:188-192`). That
/// zero-sized report clamps the atlas to 1x1
/// (`vello/src/render.rs:160-161`), which no longer matches the persistent
/// proxy, so the renderer frees the real atlas texture and installs a 1x1
/// one (`render.rs:166-171`). `ImageCache` is not told: its entries stay
/// resident and clean (`image_cache.rs:148-160`), so the next render
/// re-uses the freed slot and samples nothing. The control render below
/// pins that renderer reuse alone is fine — only the intervening
/// patch-free render breaks it.
///
/// Fixing it needs the set of images the next frame will draw, so those
/// entries can be marked dirty (`Renderer::mark_override_image_dirty` →
/// `Resolver::mark_image_dirty`, which applies to any resident image, not
/// only overrides). Nothing in the tree holds that set today — the images
/// are buried inside scene encodings. The painter-side image migration
/// creates it, and un-ignores this test.
#[test]
#[ignore = "known defect: a patch-free render frees the atlas; fixed with the painter-side image set"]
fn an_image_survives_an_intervening_image_free_render() {
    use dom::vello::peniko::{ImageBrush, ImageQuality, ImageSampler};

    let mut gpu = headless("an_image_survives_an_intervening_image_free_render");

    // A 2x2 image whose four texels are distinct, so a blank-atlas read is
    // not mistakable for a correct one.
    let image = flashbulb::rgba8(
        2,
        2,
        vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ],
    );

    let draw_image = || {
        let mut scene = Scene::new();
        scene.draw_image(
            ImageBrush {
                image: &image,
                // Nearest, so a texel reads back exactly and a filtered
                // edge cannot be mistaken for a wrong atlas.
                sampler: ImageSampler::default().with_quality(ImageQuality::Low),
            },
            // 2x2 texels scaled to fill the 64x64 target.
            Affine::scale(32.0),
        );
        scene
    };

    let mut solids = Scene::new();
    solids.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(0x11, 0x22, 0x33),
        None,
        &Rect::new(0.0, 0.0, 64.0, 64.0),
    );

    let first = gpu
        .render(&draw_image(), 64, 64, Color::WHITE)
        .expect("first image render");
    assert_eq!(
        pixel(&first, 64, 16, 16),
        [255, 0, 0, 255],
        "the first render must show the image's own texels"
    );

    // Control: back-to-back image renders, nothing in between. This isolates
    // the intervening render as the cause rather than renderer reuse itself.
    let control = gpu
        .render(&draw_image(), 64, 64, Color::WHITE)
        .expect("control image render");
    assert_eq!(
        control, first,
        "two image renders in a row must agree — renderer reuse alone is fine"
    );

    let _ = gpu
        .render(&solids, 64, 64, Color::WHITE)
        .expect("intervening image-free render");
    let third = gpu
        .render(&draw_image(), 64, 64, Color::WHITE)
        .expect("image render after the image-free one");

    assert_eq!(
        pixel(&third, 64, 16, 16),
        [255, 0, 0, 255],
        "an image-free render between two identical image renders must not \
         change what the second one draws"
    );
}
