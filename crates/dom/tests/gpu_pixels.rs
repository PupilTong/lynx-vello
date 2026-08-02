//! End-to-end pixel tests: `Document` → `PaintOrder` → `Scene` → GPU →
//! readback. Ahem glyphs are solid em squares, so glyph coverage is
//! pixel-assertable. A usable GPU adapter is mandatory.

mod paint_common;

use flashbulb::headless;
use paint_common::Doc;
use pulsar::vello::Scene;
use pulsar::vello::kurbo::{Affine, Rect};
use pulsar::vello::peniko::{BlendMode, Color, Compose, Fill, Mix};

const AHEM: &[u8] = include_bytes!("../../hughie/tests/fixtures/Ahem.ttf");
const ISOLATION_ATLAS_WIDTH: u32 = 384;
const ISOLATION_ATLAS_HEIGHT: u32 = 192;
const ISOLATION_CELL_X: u32 = 128;
const ISOLATION_CELL_Y: u32 = 32;

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

/// `background-clip: text` over Ahem text: the purple background shows
/// through glyph ink (a solid em square) and not through the space between
/// words or the un-inked box area.
#[test]
fn background_clip_text_clips_to_glyph_ink() {
    let mut gpu = headless("background_clip_text_clips_to_glyph_ink");
    // Box at (10, 10); Ahem at 20px: "HH HH" → glyph squares x 10..50 and
    // 70..110, space x 50..70; ink y 10..30 (0.8em ascent + 0.2em descent
    // spans the whole line box at the top of the 50px-tall element).
    // `color: transparent` is the authoring pattern: the glyph fill must
    // not cover the background showing through the clip.
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { display: flex; position: absolute; left: 10px; top: 10px;
                width: 180px; height: 50px;
                font-family: Ahem; font-size: 20px; color: transparent;
                background-color: rebeccapurple; background-clip: text; }";
    let mut doc = Doc::with_css(css);
    doc.dom.register_fonts(AHEM);
    let root = doc.root;
    let holder = doc.el(root, "text");
    doc.text(holder, "HH HH");

    doc.dom.render();
    let scene = doc.dom.scene();
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    // Inside the first glyph square: rebeccapurple (#663399).
    let ink = pixel(&pixels, 200, 30, 20);
    assert!(
        ink[0] > 80 && ink[0] < 130 && ink[2] > 120,
        "glyph ink must show the background ({ink:?})"
    );
    // In the inter-word space: the page base color (white).
    let gap = pixel(&pixels, 200, 60, 20);
    assert_eq!(gap, [255, 255, 255, 255], "space must stay unpainted");
    // Below the line box, still inside the element: unpainted.
    let below = pixel(&pixels, 200, 30, 50);
    assert_eq!(
        below,
        [255, 255, 255, 255],
        "un-inked box must stay unpainted"
    );
}

/// The same element without `background-clip: text` paints the whole box —
/// the sanity contrast for the assertions above.
#[test]
fn plain_background_covers_the_box() {
    let mut gpu = headless("plain_background_covers_the_box");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { display: flex; position: absolute; left: 10px; top: 10px;
                width: 180px; height: 50px;
                font-family: Ahem; font-size: 20px; color: black;
                background-color: rebeccapurple; }";
    let mut doc = Doc::with_css(css);
    doc.dom.register_fonts(AHEM);
    let root = doc.root;
    let holder = doc.el(root, "text");
    doc.text(holder, "HH HH");

    doc.dom.render();
    let scene = doc.dom.scene();
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let gap = pixel(&pixels, 200, 60, 40);
    assert!(
        gap[0] > 80 && gap[0] < 130 && gap[2] > 120,
        "without clip-text the box paints everywhere ({gap:?})"
    );
}

/// Lynx's gradient-valued `color` fills glyph ink with a real ramp, anchored
/// to the element's **padding** box.
///
/// This is the assertion the screenshot goldens cannot make: the anchor is a
/// choice about a denominator and an origin, and getting it wrong shifts every
/// color by a few percent — well inside golden tolerance, but wrong. Ahem's
/// glyphs are solid em squares, so the ramp can be read straight off a known
/// pixel.
///
/// Geometry: a 120px-wide box with a 20px left border, so the padding box runs
/// x 20..120 — 100px of ramp. Ahem at 20px puts "HH" squares at x 20..40 and
/// 40..60, so the sampled centers sit 10% and 30% along a `#ff0000 → #0000ff`
/// ramp. Both stops are opaque, so premultiplied-sRGB interpolation is just
/// `red = 255 * (1 - t)`: 230 and 179. Anchoring to the *border* box instead
/// would stretch the ramp over 120px from x=0 and read 191 and 149, which the
/// asserted windows exclude.
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
    doc.dom.register_fonts(AHEM);
    let root = doc.root;
    let holder = doc.el(root, "text");
    doc.text(holder, "HH");

    doc.dom.render();
    let scene = doc.dom.scene();
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    // Ink y: the 20px line box starts at the element's top (y = 10), so the
    // em square spans y 10..30; sample its middle.
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
    // The ramp has to actually advance between the two squares.
    assert!(
        first[0] > second[0] + 30,
        "red must fall across the ramp ({first:?} then {second:?})"
    );
}

/// `outline: solid` paints a flush ring outside the border box.
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
    let scene = doc.dom.scene();
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    // Ring band left of the border box: x 15..20 at mid-height.
    let ring = pixel(&pixels, 200, 17, 45);
    assert!(
        ring[0] > 200 && ring[1] < 60 && ring[2] < 60,
        "outline ring must be red ({ring:?})"
    );
    // Inside the box: teal background, not outline.
    let inside = pixel(&pixels, 200, 60, 45);
    assert!(
        inside[0] < 60 && inside[1] > 90 && inside[1] < 160,
        "box interior keeps its teal background ({inside:?})"
    );
    // Outside the ring: base white.
    let outside = pixel(&pixels, 200, 10, 45);
    assert_eq!(outside, [255, 255, 255, 255]);
}

/// Appending a separately painted document into an isolated atlas cell must
/// preserve nested blend layers exactly. The CSS screenshot matrix relies on
/// this for opacity, filters, masks, clips, and shadows.
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
    let scene = doc.dom.scene();
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
