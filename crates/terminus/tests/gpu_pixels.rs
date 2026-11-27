//! End-to-end pixel tests: `Document` → `PaintOrder` → `DisplayList` → GPU →
//! readback. Ahem glyphs are solid em squares, so glyph coverage is
//! pixel-assertable. A usable GPU adapter is mandatory.

mod paint_common;

use paint_common::Doc;
use prime_radiant::headless;
use terminus::link::DisplayList;
use terminus::link::kurbo::{Affine, Rect};
use terminus::link::peniko::{BlendMode, Color, Compose, Fill, ImageBrush, ImageSampler, Mix};

const AHEM: &[u8] = include_bytes!("../../seldon/tests/fixtures/Ahem.ttf");
const ISOLATION_ATLAS_WIDTH: u32 = 384;
const ISOLATION_ATLAS_HEIGHT: u32 = 192;
const ISOLATION_CELL_X: u32 = 128;
const ISOLATION_CELL_Y: u32 = 32;
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

#[test]
fn background_clip_text_clips_to_glyph_ink() {
    let mut gpu = headless("background_clip_text_clips_to_glyph_ink");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { position: absolute; left: 10px; top: 10px;
                width: 180px; height: 50px;
                font-family: Ahem; font-size: 20px; color: transparent;
                background-color: rebeccapurple; background-clip: text; }";
    let mut doc = Doc::with_css(css);
    doc.define_lynx_text("text");
    doc.dom
        .register_fonts(terminus::link::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el_tag(root, "text", "text");
    doc.text(holder, "HH HH");

    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
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
        .text { position: absolute; left: 10px; top: 10px;
                width: 180px; height: 50px;
                font-family: Ahem; font-size: 20px; color: black;
                background-color: rebeccapurple; }";
    let mut doc = Doc::with_css(css);
    doc.define_lynx_text("text");
    doc.dom
        .register_fonts(terminus::link::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el_tag(root, "text", "text");
    doc.text(holder, "HH HH");

    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let gap = pixel(&pixels, 200, 60, 40);
    assert!(
        gap[0] > 80 && gap[0] < 130 && gap[2] > 120,
        "without clip-text the box paints everywhere ({gap:?})"
    );
}

#[cfg(feature = "devtools")]
#[test]
fn a_node_highlight_fills_its_four_areas_over_later_content() {
    let mut gpu = headless("a_node_highlight_fills_its_four_areas_over_later_content");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .box { position: absolute; left: 20px; top: 10px; width: 80px; height: 60px;
               box-sizing: border-box; border: 4px solid black; padding: 8px; margin: 6px; }
        .over { position: absolute; z-index: 5; left: 0px; top: 0px;
                width: 200px; height: 100px; background-color: white; }";
    let mut doc = Doc::with_css_sized(css, 200.0, 100.0);
    let root = doc.root;
    let target = doc.el(root, "box");
    doc.el(root, "over");
    doc.dom
        .set_node_highlight(Some(terminus::link::NodeHighlight {
            node: target,
            content: Color::from_rgba8(0, 0, 255, 255),
            padding: Color::from_rgba8(0, 255, 0, 255),
            border: Color::from_rgba8(255, 255, 0, 255),
            margin: Color::from_rgba8(255, 128, 0, 255),
        }));

    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::BLACK)
        .expect("headless render");

    assert_eq!(pixel(&pixels, 200, 60, 40), BLUE, "content box");
    assert_eq!(pixel(&pixels, 200, 33, 40), [0, 255, 0, 255], "padding");
    assert_eq!(pixel(&pixels, 200, 28, 40), [255, 255, 0, 255], "border");
    assert_eq!(pixel(&pixels, 200, 23, 40), [255, 128, 0, 255], "margin");
    assert_eq!(
        pixel(&pixels, 200, 150, 90),
        WHITE,
        "the later page content"
    );
}

#[test]
fn gradient_color_fills_glyph_ink_from_the_padding_box() {
    let mut gpu = headless("gradient_color_fills_glyph_ink_from_the_padding_box");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { position: absolute; left: 0px; top: 10px;
                width: 120px; height: 50px; box-sizing: border-box;
                border-left: 20px solid black;
                font-family: Ahem; font-size: 20px;
                color: linear-gradient(90deg, #ff0000, #0000ff); }";
    let mut doc = Doc::with_css(css);
    doc.define_lynx_text("text");
    doc.dom
        .register_fonts(terminus::link::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el_tag(root, "text", "text");
    doc.text(holder, "HH");

    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
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

fn gradient_square(name: &str, image: &str) -> Vec<u8> {
    let mut gpu = headless(name);
    let css = format!(
        "page {{ display: flex; position: relative; width: 200px; height: 100px; }}
        .square {{ display: flex; position: absolute; left: 0px; top: 0px;
                   width: 100px; height: 100px; background-image: {image}; }}"
    );
    let mut doc = Doc::with_css(&css);
    let root = doc.root;
    doc.el(root, "square");

    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
    gpu.render(&scene, 200, 100, Color::WHITE)
        .expect("headless render")
}

#[test]
fn a_linear_gradients_first_stop_colours_the_line_before_it() {
    let pixels = gradient_square(
        "a_linear_gradients_first_stop_colours_the_line_before_it",
        "linear-gradient(red 50%, blue 50%)",
    );
    for y in [5, 25, 45] {
        let above = pixel(&pixels, 200, 50, y);
        assert!(
            above[0] > 240 && above[2] < 15,
            "the line before the first stop is its colour at y={y} ({above:?})"
        );
    }
    for y in [55, 75, 95] {
        let below = pixel(&pixels, 200, 50, y);
        assert!(
            below[0] < 15 && below[2] > 240,
            "the line past the hard edge is blue at y={y} ({below:?})"
        );
    }
}

#[test]
fn a_linear_gradients_last_stop_colours_the_line_past_it() {
    let pixels = gradient_square(
        "a_linear_gradients_last_stop_colours_the_line_past_it",
        "linear-gradient(red -50%, red 50%, blue 50%)",
    );
    let above = pixel(&pixels, 200, 50, 45);
    assert!(
        above[0] > 240 && above[2] < 15,
        "the line before the hard edge is red ({above:?})"
    );
    for y in [55, 75, 95] {
        let below = pixel(&pixels, 200, 50, y);
        assert!(
            below[0] < 15 && below[2] > 240,
            "the line past the last stop is its colour at y={y} ({below:?})"
        );
    }
}

#[test]
fn a_radial_gradients_first_stop_fills_the_disc_up_to_it() {
    let pixels = gradient_square(
        "a_radial_gradients_first_stop_fills_the_disc_up_to_it",
        "radial-gradient(circle 40px, red 100%, transparent 100%)",
    );
    for x in [50, 70, 87] {
        let inside = pixel(&pixels, 200, x, 50);
        assert!(
            inside[0] > 240 && inside[1] < 15 && inside[2] < 15,
            "the disc is solid red up to its edge at x={x} ({inside:?})"
        );
    }
    for (x, y) in [(93, 50), (50, 7), (5, 5)] {
        assert_eq!(
            pixel(&pixels, 200, x, y),
            WHITE,
            "past the last stop the disc is transparent at ({x}, {y})"
        );
    }
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
    let scene = doc.dom.scene(&terminus::link::NoImages);
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
    let scene = doc.dom.scene(&terminus::link::NoImages);
    let standalone = gpu
        .render(&scene, 128, 128, Color::WHITE)
        .expect("standalone headless render");

    let cell_x = f64::from(ISOLATION_CELL_X);
    let cell_y = f64::from(ISOLATION_CELL_Y);
    let left_neighbor = Rect::new(0.0, cell_y, cell_x, cell_y + 128.0);
    let cell = Rect::new(cell_x, cell_y, cell_x + 128.0, cell_y + 128.0);
    let right_neighbor = Rect::new(cell_x + 128.0, cell_y, 384.0, cell_y + 128.0);
    let mut atlas = DisplayList::new();
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

#[test]
fn a_composition_at_a_scroll_offset_stays_inside_the_scrollport() {
    use terminus::link::Vector2D;

    let mut gpu = headless("a_composition_at_a_scroll_offset_stays_inside_the_scrollport");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; width: 200px; height: 150px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .red, .blue { display: flex; flex-shrink: 0; width: 100px; height: 100px; }
         .red { background-color: #ff0000; }
         .blue { background-color: #0000ff; }",
        200.0,
        150.0,
    );
    let root = doc.root;
    let scroller = doc.el(root, "scroller");
    doc.el(scroller, "red");
    doc.el(scroller, "blue");
    doc.dom.render();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");

    for (offset, near, far) in [(0.0_f32, RED, RED), (30.0, RED, BLUE), (100.0, BLUE, BLUE)] {
        let mut scene = DisplayList::new();
        frame.compose_into(
            &mut scene,
            &[],
            &[],
            &[],
            &|_| Some(Vector2D::new(0.0, offset)),
            None,
        );
        let pixels = gpu
            .render(&scene, 200, 150, Color::WHITE)
            .expect("headless render");
        assert_eq!(pixel(&pixels, 200, 50, 20), near, "offset {offset}: y 20");
        assert_eq!(pixel(&pixels, 200, 50, 95), far, "offset {offset}: y 95");
        assert_eq!(
            pixel(&pixels, 200, 150, 50),
            WHITE,
            "offset {offset}: beside the scrollport"
        );
        assert_eq!(
            pixel(&pixels, 200, 50, 120),
            WHITE,
            "offset {offset}: below the scrollport"
        );
    }
}

#[test]
fn a_nested_scope_paints_in_its_own_colour() {
    let mut gpu = headless("a_nested_scope_paints_in_its_own_colour");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { position: absolute; left: 0px; top: 10px;
                width: 200px; height: 50px;
                font-family: Ahem; font-size: 20px; color: #ff0000; }
        .scope { color: #0000ff; }";
    let mut doc = Doc::with_css(css);
    doc.define_lynx_text("text");
    doc.dom
        .register_fonts(terminus::link::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el_tag(root, "text", "text");
    doc.text(holder, "AA");
    let scope = doc.el_tag(holder, "text", "scope");
    doc.text(scope, "BB");

    doc.dom.render();
    let scene = doc.dom.scene(&terminus::link::NoImages);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");

    let root_run = pixel(&pixels, 200, 10, 20);
    let nested = pixel(&pixels, 200, 50, 20);
    assert!(
        root_run[0] > 200 && root_run[2] < 60,
        "the root's own run stays red ({root_run:?})"
    );
    assert!(
        nested[2] > 200 && nested[0] < 60,
        "the nested scope paints in its own blue, not the root's red ({nested:?})"
    );
}

fn blur_page(size: f32, square: f32, extra: &str) -> Doc {
    let css = format!(
        "page {{ display: flex; position: relative; width: {size}px; height: {size}px; }}
         .box {{ display: flex; position: absolute; left: {left}px; top: {left}px;
                 width: {square}px; height: {square}px; background-color: #000000; {extra} }}",
        left = (size - square) / 2.0,
    );
    let mut doc = Doc::with_css_sized(&css, size, size);
    let root = doc.root;
    doc.el(root, "box");
    doc
}

fn render_filtered(
    gpu: &mut terminus::link::render::gpu::Headless,
    doc: &mut Doc,
    size: u32,
) -> Vec<u8> {
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    gpu.render(&scene, size, size, Color::WHITE)
        .expect("headless render")
}

fn luma(pixels: &[u8], width: u32, x: u32, y: u32) -> i32 {
    i32::from(pixel(pixels, width, x, y)[0])
}

#[test]
fn a_blurred_square_spreads_monotonically_and_symmetrically() {
    let mut gpu = headless("a_blurred_square_spreads_monotonically_and_symmetrically");
    let mut doc = blur_page(128.0, 48.0, "filter: blur(4px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 128);

    assert!(
        luma(&pixels, 128, 64, 64) < 8,
        "the centre keeps the fill ({:?})",
        pixel(&pixels, 128, 64, 64),
    );
    let profile: Vec<i32> = (76..=104).map(|x| luma(&pixels, 128, x, 64)).collect();
    for pair in profile.windows(2) {
        assert!(
            pair[1] >= pair[0] - 1,
            "the profile across the edge must not go back down: {profile:?}",
        );
    }
    let at_border = i32::midpoint(luma(&pixels, 128, 87, 64), luma(&pixels, 128, 88, 64));
    assert!(
        (100..=155).contains(&at_border),
        "the border sits at half coverage ({at_border})",
    );
    for distance in 0..=10_u32 {
        let inside = luma(&pixels, 128, 87 - distance, 64);
        let outside = luma(&pixels, 128, 88 + distance, 64);
        assert!(
            (inside + outside - 255).abs() <= 6,
            "the profile is symmetric about the border at {distance} px \
             ({inside} inside, {outside} outside)",
        );
    }
    assert!(
        luma(&pixels, 128, 94, 64) < 250,
        "ink reaches past the border box, within 3 sigma",
    );
    assert!(
        luma(&pixels, 128, 105, 64) >= 254,
        "and effectively none past 4 sigma ({})",
        luma(&pixels, 128, 105, 64),
    );
}

#[test]
fn a_white_blurred_square_over_white_grows_no_halo() {
    let mut gpu = headless("a_white_blurred_square_over_white_grows_no_halo");
    let mut doc = blur_page(128.0, 48.0, "background-color: #ffffff; filter: blur(4px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 128);
    for y in (40..=100).step_by(4) {
        for x in (28..=100).step_by(4) {
            assert_eq!(
                pixel(&pixels, 128, x, y),
                WHITE,
                "white on white must stay white at ({x}, {y})",
            );
        }
    }
}

#[test]
fn a_translucent_blurred_square_keeps_its_colour() {
    let mut gpu = headless("a_translucent_blurred_square_keeps_its_colour");
    let mut doc = blur_page(
        128.0,
        48.0,
        "background-color: rgb(255 0 0 / 50%); filter: blur(4px);",
    );
    let pixels = render_filtered(&mut gpu, &mut doc, 128);
    let [red, green, blue, _] = pixel(&pixels, 128, 64, 64);
    assert!(
        red >= 253 && (124..=132).contains(&green) && green.abs_diff(blue) <= 1,
        "the centre is half red over white ({:?})",
        [red, green, blue],
    );
}

#[test]
fn an_undecimated_blur_reuses_the_bake_target_correctly() {
    let mut gpu = headless("an_undecimated_blur_reuses_the_bake_target_correctly");
    let mut doc = blur_page(128.0, 48.0, "filter: blur(2px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 128);

    assert!(
        luma(&pixels, 128, 64, 64) < 8,
        "the centre keeps the fill ({})",
        luma(&pixels, 128, 64, 64),
    );
    for distance in 0..=5_u32 {
        let inside = luma(&pixels, 128, 87 - distance, 64);
        let outside = luma(&pixels, 128, 88 + distance, 64);
        assert!(
            (inside + outside - 255).abs() <= 6,
            "the profile is symmetric about the border at {distance} px \
             ({inside} inside, {outside} outside)",
        );
    }
    assert!(
        luma(&pixels, 128, 90, 64) < 250,
        "ink reaches past the border box ({})",
        luma(&pixels, 128, 90, 64),
    );
    assert!(
        luma(&pixels, 128, 97, 64) >= 254,
        "and effectively none past 4 sigma ({})",
        luma(&pixels, 128, 97, 64),
    );
}

#[test]
fn a_decimated_blur_stays_centred_and_symmetric() {
    let mut gpu = headless("a_decimated_blur_stays_centred_and_symmetric");
    let mut doc = blur_page(256.0, 64.0, "filter: blur(16px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 256);

    for distance in [8_u32, 16, 32, 48] {
        let left = luma(&pixels, 256, 127 - distance, 128);
        let right = luma(&pixels, 256, 128 + distance, 128);
        let up = luma(&pixels, 256, 128, 127 - distance);
        let down = luma(&pixels, 256, 128, 128 + distance);
        assert!(
            (left - right).abs() <= 2 && (up - down).abs() <= 2 && (left - up).abs() <= 3,
            "a decimated blur must stay centred at {distance} px \
             (l {left}, r {right}, u {up}, d {down})",
        );
    }
    assert!(
        luma(&pixels, 256, 128, 128) < 90,
        "the centre of a 64 px square under sigma 16 keeps most of its ink ({})",
        luma(&pixels, 256, 128, 128),
    );
    assert!(
        luma(&pixels, 256, 128, 30) >= 253,
        "and nothing reaches past 3 sigma of the box ({})",
        luma(&pixels, 256, 128, 30),
    );
}

#[test]
fn a_blurred_square_straddling_the_viewport_edge_matches_one_inside() {
    let mut gpu = headless("a_blurred_square_straddling_the_viewport_edge_matches_one_inside");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
         .box { display: flex; position: absolute; top: 40px;
                width: 48px; height: 48px; background-color: #000000;
                filter: blur(4px); }";

    let read = |gpu: &mut terminus::link::render::gpu::Headless, left: f32| -> Vec<u8> {
        let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
        let root = doc.root;
        let boxed = doc.el(root, "box");
        doc.dom.set_inline_style(boxed, &format!("left: {left}px"));
        render_filtered(gpu, &mut doc, 128)
    };
    let inside = read(&mut gpu, 40.0);
    let straddling = read(&mut gpu, -24.0);
    for x in 0..40_u32 {
        for y in (44..=84).step_by(8) {
            let expected = luma(&inside, 128, x + 64, y);
            let actual = luma(&straddling, 128, x, y);
            assert!(
                (expected - actual).abs() <= 2,
                "({x}, {y}) reads {actual} where the same square inside reads {expected}",
            );
        }
    }
}

#[test]
fn a_blurred_box_in_a_scroller_moves_with_the_offset() {
    use terminus::link::Vector2D;

    let mut gpu = headless("a_blurred_box_in_a_scroller_moves_with_the_offset");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; width: 200px; height: 200px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .spacer { display: flex; flex-shrink: 0; width: 100px; height: 60px; }
         .box { display: flex; flex-shrink: 0; width: 60px; height: 40px;
                background-color: #000000; filter: blur(3px); }
         .tail { display: flex; flex-shrink: 0; width: 100px; height: 200px; }",
        200.0,
        200.0,
    );
    let root = doc.root;
    let scroller = doc.el(root, "scroller");
    doc.el(scroller, "spacer");
    doc.el(scroller, "box");
    doc.el(scroller, "tail");
    doc.dom.render();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert!(
        !frame.filter_groups().is_empty(),
        "the blurred box records a filter group"
    );

    gpu.forget_filters();
    for offset in [0.0_f32, 40.0] {
        let filtered: Vec<Option<terminus::link::Texture>> = gpu
            .prepare_filters(
                &frame,
                &[],
                &[],
                &|_| Some(Vector2D::new(0.0, offset)),
                0,
                None,
                Color::WHITE,
            )
            .expect("the filter bakes render")
            .to_vec();
        let mut scene = DisplayList::new();
        frame.compose_into(
            &mut scene,
            &[],
            &filtered,
            &[],
            &|_| Some(Vector2D::new(0.0, offset)),
            None,
        );
        let pixels = gpu
            .render(&scene, 200, 200, Color::WHITE)
            .expect("headless render");
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "both offsets put the probe at a whole pixel inside the port"
        )]
        let centre = (80.0 - offset) as u32;
        assert!(
            luma(&pixels, 200, 30, centre) < 40,
            "offset {offset}: the blurred box shows at y {centre} ({})",
            luma(&pixels, 200, 30, centre),
        );
        assert_eq!(
            pixel(&pixels, 200, 150, centre),
            WHITE,
            "offset {offset}: nothing leaks beside the scrollport",
        );
        assert_eq!(
            pixel(&pixels, 200, 30, 150),
            WHITE,
            "offset {offset}: nothing leaks below the scrollport",
        );
    }
}

#[test]
fn a_sliding_blurred_card_stays_inside_its_ancestors_clip() {
    let mut gpu = headless("a_sliding_blurred_card_stays_inside_its_ancestors_clip");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; width: 200px; height: 100px; }
         .frame { display: flex; width: 100px; height: 100px; overflow: clip; }
         .card { display: flex; flex-shrink: 0; margin: 20px; width: 60px; height: 60px;
                 background-color: #000000; filter: blur(2px);
                 animation: slide 1s linear infinite; }
         @keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(100px); } }",
        200.0,
        100.0,
    );
    let root = doc.root;
    let clip = doc.el(root, "frame");
    doc.el(clip, "card");
    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(0.1);
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert!(frame.has_live_curves(), "the slide exports");
    assert!(
        !frame.filter_groups()[0].samples_animations(),
        "the slide moves the texture across its ancestor's clip, not what is baked",
    );

    let now = Some(0.6);
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, now, Color::WHITE)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, now);
    let pixels = gpu
        .render(&scene, 200, 100, Color::WHITE)
        .expect("headless render");
    assert!(
        luma(&pixels, 200, 90, 50) < 40,
        "the card shows inside the clip ({})",
        luma(&pixels, 200, 90, 50),
    );
    assert_eq!(
        pixel(&pixels, 200, 120, 50),
        WHITE,
        "and nothing of it past the clip's edge",
    );
}

fn compose_at(
    gpu: &mut terminus::link::render::gpu::Headless,
    frame: &terminus::link::CommittedFrame,
    now: Option<f64>,
    (width, height): (u32, u32),
) -> Vec<u8> {
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(frame, &[], &[], &|_| None, 0, now, Color::WHITE)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, now);
    gpu.render(&scene, width, height, Color::WHITE)
        .expect("headless render")
}

fn assert_pixels_match(a: &[u8], b: &[u8], slack: u8, label: &str) {
    assert_eq!(a.len(), b.len(), "{label}: sizes");
    let worst = a
        .iter()
        .zip(b)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(worst <= slack, "{label}: channels differ by up to {worst}");
}

#[test]
fn a_child_sliding_out_of_a_still_opacity_group_composes_as_committed() {
    let mut gpu = headless("a_child_sliding_out_of_a_still_opacity_group_composes_as_committed");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; width: 200px; height: 100px; }
         .group { display: flex; margin: 20px; width: 60px; height: 60px; opacity: 0.5; }
         .card { display: flex; flex-shrink: 0; width: 40px; height: 40px;
                 background-color: #000000; animation: slide 1s linear infinite; }
         @keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(100px); } }",
        200.0,
        100.0,
    );
    let root = doc.root;
    let group = doc.el(root, "group");
    doc.el(group, "card");
    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(0.1);
    doc.dom.render();
    let early = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert!(
        early.has_live_curves(),
        "the slide exports inside the group"
    );

    let composed = compose_at(&mut gpu, &early, Some(0.6), (200, 100));
    doc.dom.advance_animations(0.6);
    doc.dom.render();
    let late = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert_ne!(early.commit_id(), late.commit_id(), "a fresh commit");
    let committed = compose_at(&mut gpu, &late, None, (200, 100));
    assert!(
        luma(&committed, 200, 100, 40) < 160,
        "the fresh commit draws the half-faded card past its parent's box ({})",
        luma(&committed, 200, 100, 40),
    );
    assert_pixels_match(&composed, &committed, 2, "composed at 0.6 s");
}

#[test]
fn a_blurred_group_with_a_sliding_child_composes_as_committed_beside_a_still_blur() {
    let mut gpu =
        headless("a_blurred_group_with_a_sliding_child_composes_as_committed_beside_a_still_blur");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; width: 200px; height: 100px; }
         .still { display: flex; flex-shrink: 0; margin: 20px 0px 0px 10px; width: 30px;
                  height: 30px; background-color: #000000; filter: blur(2px); }
         .group { display: flex; flex-shrink: 0; margin: 20px; width: 50px; height: 50px;
                  filter: blur(2px); }
         .card { display: flex; flex-shrink: 0; width: 30px; height: 30px;
                 background-color: #000000; animation: slide 1s linear infinite; }
         @keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(100px); } }",
        200.0,
        100.0,
    );
    let root = doc.root;
    doc.el(root, "still");
    let group = doc.el(root, "group");
    doc.el(group, "card");
    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(0.1);
    doc.dom.render();
    gpu.forget_filters();
    let early = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let groups = early.filter_groups();
    assert_eq!(groups.len(), 2, "two blurred groups");
    assert!(
        !groups[0].samples_animations() && groups[1].samples_animations(),
        "only the group holding the slide samples the timeline",
    );

    let instants = [0.4, 0.6];
    let composed = instants.map(|now| compose_at(&mut gpu, &early, Some(now), (200, 100)));
    for (now, composed) in instants.into_iter().zip(composed) {
        doc.dom.advance_animations(now);
        doc.dom.render();
        let late = doc
            .dom
            .committed_frame()
            .expect("render leaves a committed frame retained");
        let committed = compose_at(&mut gpu, &late, None, (200, 100));
        assert!(
            luma(&committed, 200, 25, 35) < 128,
            "the still box is drawn at {now} s ({})",
            luma(&committed, 200, 25, 35),
        );
        assert_pixels_match(&composed, &committed, 3, &format!("composed at {now} s"));
    }
}

#[test]
fn a_blurred_backdrop_beside_a_sliding_card_composes_as_committed() {
    let mut gpu = headless("a_blurred_backdrop_beside_a_sliding_card_composes_as_committed");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .card { display: flex; position: absolute; left: 0px; top: 20px; width: 30px;
                 height: 60px; background-color: #000000; animation: slide 1s linear infinite; }
         .frost { display: flex; position: absolute; left: 60px; top: 10px; width: 130px;
                  height: 80px; background-color: rgba(255, 255, 255, 0.2);
                  backdrop-filter: blur(3px); filter: blur(1px); }
         @keyframes slide { from { transform: translateX(0px); }
                            to { transform: translateX(200px); } }",
        200.0,
        100.0,
    );
    let root = doc.root;
    doc.el(root, "card");
    doc.el(root, "frost");
    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(0.1);
    doc.dom.render();
    gpu.forget_filters();
    let early = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let groups = early.filter_groups();
    assert_eq!(groups.len(), 2, "a blur group and a backdrop");
    assert!(
        groups
            .iter()
            .all(terminus::link::FilterGroup::samples_animations),
        "both sample the timeline: the backdrop reads the slide, the group draws the backdrop",
    );

    let instants = [0.4, 0.6];
    let composed = instants.map(|now| compose_at(&mut gpu, &early, Some(now), (200, 100)));
    for (now, composed) in instants.into_iter().zip(composed) {
        doc.dom.advance_animations(now);
        doc.dom.render();
        let late = doc
            .dom
            .committed_frame()
            .expect("render leaves a committed frame retained");
        let committed = compose_at(&mut gpu, &late, None, (200, 100));
        assert_pixels_match(&composed, &committed, 3, &format!("composed at {now} s"));
    }
}

#[test]
fn a_backdrop_inside_a_fading_card_composes_as_committed() {
    let mut gpu = headless("a_backdrop_inside_a_fading_card_composes_as_committed");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 200px; height: 100px; }
         .under { display: flex; position: absolute; left: 0px; top: 0px; width: 200px;
                  height: 100px; background-color: #ff0000; }
         .card { display: flex; position: absolute; left: 20px; top: 20px; width: 120px;
                 height: 60px; background-color: #0000ff; animation: fade 1s linear infinite; }
         .frost { display: flex; margin: 10px; width: 60px; height: 40px;
                  backdrop-filter: blur(3px); }
         @keyframes fade { 0%, 20% { opacity: 1; } 100% { opacity: 0.3; } }",
        200.0,
        100.0,
    );
    let root = doc.root;
    doc.el(root, "under");
    let card = doc.el(root, "card");
    doc.el(card, "frost");
    doc.dom.render();
    doc.dom.advance_animations(0.0);
    doc.dom.advance_animations(0.1);
    doc.dom.render();
    gpu.forget_filters();
    let early = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert!(early.has_live_curves(), "the fade exports");
    let groups = early.filter_groups();
    assert_eq!(groups.len(), 1, "one backdrop");
    assert!(
        !groups[0].samples_animations(),
        "the backdrop reads the card alone, which fades with it",
    );

    let composed = compose_at(&mut gpu, &early, Some(0.6), (200, 100));
    doc.dom.advance_animations(0.6);
    doc.dom.render();
    let late = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert_ne!(early.commit_id(), late.commit_id(), "a fresh commit");
    let committed = compose_at(&mut gpu, &late, None, (200, 100));
    assert_pixels_match(&composed, &committed, 3, "composed at 0.6 s");
}

#[test]
fn a_blurred_backdrop_over_a_scroller_re_bakes_with_it() {
    use terminus::link::Vector2D;

    let mut gpu = headless("a_blurred_backdrop_over_a_scroller_re_bakes_with_it");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 128px; height: 128px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 128px; height: 128px; }
         .stripe { display: flex; flex-shrink: 0; width: 128px; height: 24px;
                   background-color: #000000; }
         .gap { display: flex; flex-shrink: 0; width: 128px; height: 24px; }
         .box { display: flex; position: fixed; left: 16px; top: 40px;
                width: 96px; height: 48px; backdrop-filter: blur(4px); filter: blur(1px); }",
        128.0,
        128.0,
    );
    let root = doc.root;
    let scroller = doc.el(root, "scroller");
    for class in ["stripe", "gap", "stripe", "gap", "stripe", "gap"] {
        doc.el(scroller, class);
    }
    doc.el(root, "box");
    doc.dom.render();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert_eq!(
        frame.filter_groups().len(),
        2,
        "a blur group and a backdrop"
    );

    let draw = |gpu: &mut terminus::link::render::gpu::Headless, generation: u64, offset: f32| {
        let offset_of = |_: &terminus::link::ScrollSlot| Some(Vector2D::new(0.0, offset));
        let filtered: Vec<Option<terminus::link::Texture>> = gpu
            .prepare_filters(&frame, &[], &[], &offset_of, generation, None, Color::WHITE)
            .expect("the entries bake")
            .to_vec();
        assert!(filtered.iter().all(Option::is_some), "both entries baked");
        let mut scene = DisplayList::new();
        frame.compose_into(&mut scene, &[], &filtered, &[], &offset_of, None);
        gpu.render(&scene, 128, 128, Color::WHITE)
            .expect("headless render")
    };
    gpu.forget_filters();
    let first = draw(&mut gpu, 0, 0.0);
    let warm = draw(&mut gpu, 1, 12.0);
    gpu.forget_filters();
    let cold = draw(&mut gpu, 1, 12.0);
    assert_pixels_match(&warm, &cold, 2, "the second offset");
    let moved = (44..=84_u32)
        .filter(|&y| (luma(&first, 128, 64, y) - luma(&cold, 128, 64, y)).abs() > 12)
        .count();
    assert!(
        moved >= 8,
        "and the offsets draw different backdrops ({moved} rows differ)"
    );
}

fn striped_page(css: &str, width: f32, height: f32) -> Doc {
    let mut doc = Doc::with_css_sized(
        &format!(
            "page {{ display: flex; position: relative; width: {width}px; height: {height}px;
                     background-color: #ffffff; }}
             .stripe {{ display: flex; position: absolute; top: 0px; width: 4px;
                        height: {height}px; background-color: #000000; }}
             {css}"
        ),
        width,
        height,
    );
    let root = doc.root;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a test page a few hundred px wide"
    )]
    for stripe in 0..(width as u32).div_ceil(8) {
        let id = doc.el(root, "stripe");
        doc.dom
            .set_inline_style(id, &format!("left: {}px", stripe * 8));
    }
    doc
}

#[test]
fn a_backdrop_half_outside_an_overflow_hidden_card_stops_at_its_edge() {
    use euclid::default::Point2D;

    let mut gpu = headless("a_backdrop_half_outside_an_overflow_hidden_card_stops_at_its_edge");
    let css = ".card { display: flex; position: absolute; left: 20px; top: 20px;
                       width: 80px; height: 60px; overflow: hidden; }
               .frost { display: flex; position: absolute; left: 40px; top: 10px;
                        width: 80px; height: 40px; }
               .frosted { backdrop-filter: blur(8px); }";
    let read = |gpu: &mut terminus::link::render::gpu::Headless, class: &str| {
        let mut doc = striped_page(css, 200.0, 100.0);
        let root = doc.root;
        let card = doc.el(root, "card");
        let frost = doc.el(card, class);
        let pixels = render_filtered(gpu, &mut doc, 200);
        (pixels, doc, frost)
    };
    let (plain, _, _) = read(&mut gpu, "frost");
    let (frosted, doc, frost) = read(&mut gpu, "frost frosted");
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert!(
        frame
            .filter_groups()
            .iter()
            .any(terminus::link::FilterGroup::is_backdrop),
        "the frost records a backdrop",
    );

    assert_eq!(
        luma(&plain, 200, 81, 50),
        0,
        "the stripe is black unfiltered"
    );
    assert!(
        luma(&frosted, 200, 81, 50) > 60,
        "the frost greys the stripe inside the card ({})",
        luma(&frosted, 200, 81, 50),
    );
    for y in (32..=68).step_by(4) {
        for x in 101..140 {
            assert_eq!(
                pixel(&frosted, 200, x, y),
                pixel(&plain, 200, x, y),
                "({x}, {y}) is past the card's clip and shows the page unfiltered",
            );
        }
    }
    assert!(
        doc.dom
            .elements_from_point(Point2D::new(80.0, 50.0))
            .contains(&frost),
        "the frost is hit inside the card",
    );
    assert!(
        !doc.dom
            .elements_from_point(Point2D::new(120.0, 50.0))
            .contains(&frost),
        "and not past its edge, where it paints nothing",
    );
}

#[test]
fn a_blur_halo_stops_at_its_ancestors_overflow_clip() {
    let mut gpu = headless("a_blur_halo_stops_at_its_ancestors_overflow_clip");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
         .outer { display: flex; position: absolute; left: 0px; top: 0px;
                  width: 128px; height: 128px; }
         .card { display: flex; position: absolute; left: 16px; top: 16px;
                 width: 64px; height: 96px; overflow: hidden; }
         .box { display: flex; position: absolute; left: 24px; top: 24px;
                width: 40px; height: 48px; background-color: #000000;
                filter: blur(4px); }";
    let read = |gpu: &mut terminus::link::render::gpu::Headless, outer: &str| -> Vec<u8> {
        let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
        let root = doc.root;
        let outer_box = doc.el(root, "outer");
        doc.dom.set_inline_style(outer_box, outer);
        let card = doc.el(outer_box, "card");
        doc.el(card, "box");
        render_filtered(gpu, &mut doc, 128)
    };

    let single = read(&mut gpu, "");
    assert!(
        luma(&single, 128, 36, 64) < 250,
        "the halo shows inside the card, left of the box ({})",
        luma(&single, 128, 36, 64),
    );
    for x in 80..96 {
        assert_eq!(
            pixel(&single, 128, x, 64),
            WHITE,
            "no halo past the card's edge at x = {x}",
        );
    }

    let nested = read(&mut gpu, "filter: blur(1px)");
    assert!(
        luma(&nested, 128, 36, 64) < 250,
        "the nested halo shows inside the card ({})",
        luma(&nested, 128, 36, 64),
    );
    assert!(
        luma(&nested, 128, 80, 64) < 250,
        "the outer blur spreads the clipped halo past the card's edge ({})",
        luma(&nested, 128, 80, 64),
    );
    for x in 84..96 {
        assert_eq!(
            pixel(&nested, 128, x, 64),
            WHITE,
            "only the outer blur's own 3σ passes the card's edge, not the box's halo (x = {x})",
        );
    }
}

#[test]
fn a_blur_straddling_an_overflow_clip_reads_its_content_past_the_edge() {
    let mut gpu = headless("a_blur_straddling_an_overflow_clip_reads_its_content_past_the_edge");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
         .card { display: flex; position: absolute; left: 16px; top: 16px;
                 width: 64px; height: 96px; }
         .box { display: flex; position: absolute; top: 24px; height: 48px;
                background-color: #000000; filter: blur(4px); }";
    let read = |gpu: &mut terminus::link::render::gpu::Headless,
                overflow: &str,
                (left, width): (f32, f32)| {
        let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
        let root = doc.root;
        let card = doc.el(root, "card");
        doc.dom
            .set_inline_style(card, &format!("overflow: {overflow}"));
        let boxed = doc.el(card, "box");
        doc.dom
            .set_inline_style(boxed, &format!("left: {left}px; width: {width}px"));
        render_filtered(gpu, &mut doc, 128)
    };

    for (label, geometry) in [("straddling", (24.0_f32, 80.0)), ("outside", (68.0, 40.0))] {
        let clipped = read(&mut gpu, "hidden", geometry);
        let open = read(&mut gpu, "visible", geometry);
        for y in (44..=84).step_by(4) {
            for x in 56..80 {
                let (inside, reference) = (luma(&clipped, 128, x, y), luma(&open, 128, x, y));
                assert!(
                    (inside - reference).abs() <= 2,
                    "{label}: ({x}, {y}) inside the edge reads {inside}, \
                     {reference} with the overflow visible",
                );
            }
            for x in 80..128 {
                assert_eq!(
                    pixel(&clipped, 128, x, y),
                    WHITE,
                    "{label}: nothing past the edge at ({x}, {y})",
                );
            }
        }
        if label == "straddling" {
            assert!(
                luma(&clipped, 128, 79, 64) < 40,
                "the box's own ink reaches the edge nearly whole ({})",
                luma(&clipped, 128, 79, 64),
            );
        } else {
            assert!(
                luma(&clipped, 128, 79, 64) < 250,
                "ink from past the edge reaches back inside it ({})",
                luma(&clipped, 128, 79, 64),
            );
        }
    }
}

#[test]
fn a_nested_backdrop_reads_its_roots_content_past_an_ancestors_edge() {
    let mut gpu = headless("a_nested_backdrop_reads_its_roots_content_past_an_ancestors_edge");
    let css = ".card { display: flex; position: absolute; left: 20px; top: 20px;
                       width: 80px; height: 60px; }
               .root { display: flex; position: absolute; left: 40px; top: 10px;
                       width: 80px; height: 40px; backdrop-filter: brightness(1); }
               .frost { display: flex; position: absolute; left: 0px; top: 0px;
                        width: 80px; height: 40px; backdrop-filter: blur(4px); }";
    let read = |gpu: &mut terminus::link::render::gpu::Headless, overflow: Option<&str>| {
        let mut doc = striped_page(css, 200.0, 100.0);
        let root = doc.root;
        let card = doc.el(root, "card");
        if let Some(overflow) = overflow {
            doc.dom
                .set_inline_style(card, &format!("overflow: {overflow}"));
            let backdrop_root = doc.el(card, "root");
            doc.el(backdrop_root, "frost");
        }
        render_filtered(gpu, &mut doc, 200)
    };
    let plain = read(&mut gpu, None);
    let clipped = read(&mut gpu, Some("hidden"));
    let open = read(&mut gpu, Some("visible"));

    assert!(
        (luma(&clipped, 200, 81, 50) - luma(&plain, 200, 81, 50)).abs() > 60,
        "the nested backdrop blurs the stripes inside the card",
    );
    for y in (32..=68).step_by(4) {
        for x in 84..100 {
            let (inside, reference) = (luma(&clipped, 200, x, y), luma(&open, 200, x, y));
            assert!(
                (inside - reference).abs() <= 2,
                "({x}, {y}) inside the edge reads {inside}, \
                 {reference} with the overflow visible",
            );
        }
        for x in 101..140 {
            assert_eq!(
                pixel(&clipped, 200, x, y),
                pixel(&plain, 200, x, y),
                "({x}, {y}) is past the card's clip and shows the page unfiltered",
            );
        }
    }
}

#[test]
fn a_nested_blur_blurs_more_than_either_alone() {
    let mut gpu = headless("a_nested_blur_blurs_more_than_either_alone");
    let css = "page { display: flex; position: relative; width: 192px; height: 192px; }
         .outer { display: flex; position: absolute; left: 64px; top: 64px;
                  width: 64px; height: 64px; }
         .inner { display: flex; width: 64px; height: 64px;
                  background-color: #000000; }";

    let read =
        |gpu: &mut terminus::link::render::gpu::Headless, outer: &str, inner: &str| -> Vec<u8> {
            let mut doc = Doc::with_css_sized(css, 192.0, 192.0);
            let root = doc.root;
            let outer_box = doc.el(root, "outer");
            let inner_box = doc.el(outer_box, "inner");
            doc.dom.set_inline_style(outer_box, outer);
            doc.dom.set_inline_style(inner_box, inner);
            render_filtered(gpu, &mut doc, 192)
        };

    let outer_only = read(&mut gpu, "filter: blur(4px)", "");
    let inner_only = read(&mut gpu, "", "filter: blur(4px)");
    let both = read(&mut gpu, "filter: blur(4px)", "filter: blur(4px)");

    let probe = |pixels: &[u8]| luma(pixels, 192, 134, 96);
    assert!(
        probe(&both) < probe(&outer_only) - 4 && probe(&both) < probe(&inner_only) - 4,
        "nesting two blurs must spread further than either \
         (both {}, outer {}, inner {})",
        probe(&both),
        probe(&outer_only),
        probe(&inner_only),
    );
    assert!(
        luma(&both, 192, 96, 96) < 60,
        "and the centre still keeps its ink ({})",
        luma(&both, 192, 96, 96),
    );
}

#[test]
fn a_group_over_the_budget_renders_unblurred() {
    let mut gpu = headless("a_group_over_the_budget_renders_unblurred");
    let mut doc = blur_page(128.0, 48.0, "filter: blur(4px);");
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let groups = frame.filter_groups();
    assert_eq!(groups.len(), 1, "one blurred group");
    let (width, height) = groups[0].size();
    assert!(
        u64::from(width) * u64::from(height) <= terminus::link::render::blur::MAX_FILTER_AREA,
        "this group is inside the budget",
    );

    let mut wide = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 4000px; height: 4000px; }
         .box { display: flex; position: absolute; left: 0px; top: 0px;
                width: 3000px; height: 3000px; background-color: #000000;
                filter: blur(4px); }",
        4000.0,
        4000.0,
    );
    let root = wide.root;
    wide.el(root, "box");
    wide.dom.set_device_pixel_ratio(2.0);
    wide.dom.render();
    gpu.forget_filters();
    let frame = wide
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let groups = frame.filter_groups();
    assert_eq!(groups.len(), 1, "the group is still recorded");
    let (width, height) = groups[0].size();
    assert!(
        u64::from(width) * u64::from(height) > terminus::link::render::blur::MAX_FILTER_AREA,
        "and it is past the budget ({width}x{height})",
    );
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("a refused group is not an error")
        .to_vec();
    assert_eq!(filtered.len(), 1);
    assert!(
        filtered[0].is_none(),
        "a group over the budget gets no texture"
    );
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    let pixels = gpu
        .render(&scene, 256, 256, Color::WHITE)
        .expect("the unblurred fallback renders");
    assert!(
        luma(&pixels, 256, 128, 128) < 8,
        "the fallback paints the box, hard-edged ({})",
        luma(&pixels, 256, 128, 128),
    );
}

fn backdrop_page(size: f32, box_rect: (f32, f32, f32, f32), extra: &str) -> Doc {
    seam_page(Ground::Painted, size, box_rect, "", extra)
}

#[derive(Clone, Copy)]
enum Ground {
    Painted,
    Canvas,
}

fn seam_page(
    ground: Ground,
    size: f32,
    box_rect: (f32, f32, f32, f32),
    wrapper: &str,
    extra: &str,
) -> Doc {
    let (left, top, width, height) = box_rect;
    let ground = match ground {
        Ground::Painted => "background-color: #ffffff;",
        Ground::Canvas => "",
    };
    let css = format!(
        "page {{ display: flex; position: relative; width: {size}px; height: {size}px;
                 {ground} }}
         .half {{ display: flex; position: absolute; left: 0px; top: 0px;
                  width: {half}px; height: {size}px; background-color: #000000; }}
         .wrap {{ display: flex; position: absolute; left: 0px; top: 0px;
                  width: {size}px; height: {size}px; {wrapper} }}
         .box {{ display: flex; position: absolute; left: {left}px; top: {top}px;
                 width: {width}px; height: {height}px; {extra} }}",
        half = size / 2.0,
    );
    let mut doc = Doc::with_css_sized(&css, size, size);
    let root = doc.root;
    doc.el(root, "half");
    let wrap = doc.el(root, "wrap");
    doc.el(wrap, "box");
    doc
}

fn render_baked(
    gpu: &mut terminus::link::render::gpu::Headless,
    doc: &mut Doc,
    size: u32,
    entries: usize,
) -> Vec<u8> {
    render_baked_over(gpu, doc, size, entries, Color::WHITE)
}

fn render_baked_over(
    gpu: &mut terminus::link::render::gpu::Headless,
    doc: &mut Doc,
    size: u32,
    entries: usize,
    canvas: Color,
) -> Vec<u8> {
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert_eq!(
        frame.filter_groups().len(),
        entries,
        "the frame's filter entries",
    );
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, canvas)
        .expect("the bakes render")
        .to_vec();
    assert!(
        filtered.iter().all(Option::is_some),
        "every recorded entry baked a texture",
    );
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    gpu.render(&scene, size, size, canvas)
        .expect("headless render")
}

#[test]
fn a_backdrop_blur_softens_the_seam_only_inside_the_box() {
    let mut gpu = headless("a_backdrop_blur_softens_the_seam_only_inside_the_box");
    let mut doc = backdrop_page(
        128.0,
        (32.0, 32.0, 64.0, 64.0),
        "backdrop-filter: blur(6px); background-color: rgb(255 0 0 / 20%);",
    );
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);

    let inside: Vec<i32> = (46..=82).map(|x| luma(&pixels, 128, x, 64)).collect();
    for pair in inside.windows(2) {
        assert!(
            pair[1] >= pair[0] - 1,
            "the blurred seam must rise monotonically: {inside:?}",
        );
    }
    assert!(
        inside[0] < 60 && inside[inside.len() - 1] > 180,
        "the gradient must still span the seam ({} to {})",
        inside[0],
        inside[inside.len() - 1],
    );
    assert!(
        (inside[14]..=inside[22]).contains(&luma(&pixels, 128, 64, 64)),
        "and the seam's own pixel sits inside it",
    );

    assert!(
        luma(&pixels, 128, 60, 16) < 8,
        "unfiltered black left of it"
    );
    assert!(
        luma(&pixels, 128, 68, 16) >= 250,
        "unfiltered white right of it",
    );

    let over_white = pixel(&pixels, 128, 88, 64);
    assert!(
        over_white[0] > 240 && over_white[1] < 215,
        "the box's own background composites on top ({over_white:?})",
    );
}

#[test]
fn a_later_sibling_is_not_in_the_backdrop() {
    let mut gpu = headless("a_later_sibling_is_not_in_the_backdrop");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px;
                background-color: #ffffff; }
         .under { display: flex; position: absolute; left: 72px; top: 0px;
                  width: 16px; height: 128px; background-color: #000000; }
         .box { display: flex; position: absolute; left: 16px; top: 16px;
                width: 96px; height: 96px; backdrop-filter: blur(6px); }
         .over { display: flex; position: absolute; left: 32px; top: 0px;
                 width: 16px; height: 128px; background-color: #000000; }";
    let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
    let root = doc.root;
    doc.el(root, "under");
    doc.el(root, "box");
    doc.el(root, "over");
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);

    assert!(
        (40..=190).contains(&luma(&pixels, 128, 80, 64)),
        "the earlier bar is blurred inside the box ({})",
        luma(&pixels, 128, 80, 64),
    );
    assert!(
        luma(&pixels, 128, 40, 64) < 8,
        "the later bar stays opaque ({})",
        luma(&pixels, 128, 40, 64),
    );
    assert!(
        luma(&pixels, 128, 30, 64) >= 250 && luma(&pixels, 128, 50, 64) >= 250,
        "and its edges stay hard ({}, {})",
        luma(&pixels, 128, 30, 64),
        luma(&pixels, 128, 50, 64),
    );
}

#[test]
fn an_opacity_ancestor_bounds_the_backdrop() {
    let mut gpu = headless("an_opacity_ancestor_bounds_the_backdrop");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px;
                background-color: #ffffff; }
         .outside { display: flex; position: absolute; left: 0px; top: 0px;
                    width: 64px; height: 128px; background-color: #000000; }
         .root { display: flex; position: absolute; left: 0px; top: 0px;
                 width: 128px; height: 128px; opacity: 0.5; }
         .inside { display: flex; position: absolute; left: 0px; top: 0px;
                   width: 24px; height: 128px; background-color: #ffffff; }
         .box { display: flex; position: absolute; left: 4px; top: 40px;
                width: 120px; height: 48px; backdrop-filter: blur(5px); }";
    let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
    let root = doc.root;
    doc.el(root, "outside");
    let wrap = doc.el(root, "root");
    doc.el(wrap, "inside");
    doc.el(wrap, "box");
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);

    for y in [20_u32, 64] {
        let step = luma(&pixels, 128, 65, y) - luma(&pixels, 128, 63, y);
        assert!(
            step > 100,
            "the edge outside the Backdrop Root is a step at y {y} ({step})",
        );
    }
    assert_eq!(
        luma(&pixels, 128, 58, 64),
        luma(&pixels, 128, 58, 20),
        "and nothing of it bleeds inward under the box",
    );

    let spread = luma(&pixels, 128, 28, 64) - luma(&pixels, 128, 28, 20);
    assert!(
        spread > 10,
        "the edge inside the Backdrop Root is blurred under the box ({spread})",
    );
}

#[test]
fn a_fixed_backdrop_over_a_scroller_rebakes_per_offset() {
    use terminus::link::Vector2D;

    let mut gpu = headless("a_fixed_backdrop_over_a_scroller_rebakes_per_offset");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 128px; height: 128px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 128px; height: 128px; }
         .stripe { display: flex; flex-shrink: 0; width: 128px; height: 24px;
                   background-color: #000000; }
         .gap { display: flex; flex-shrink: 0; width: 128px; height: 24px; }
         .box { display: flex; position: fixed; left: 16px; top: 40px;
                width: 96px; height: 48px; backdrop-filter: blur(4px); }",
        128.0,
        128.0,
    );
    let root = doc.root;
    let scroller = doc.el(root, "scroller");
    for class in ["stripe", "gap", "stripe", "gap", "stripe", "gap"] {
        doc.el(scroller, class);
    }
    doc.el(root, "box");
    doc.dom.render();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let entries = frame.filter_groups();
    assert_eq!(entries.len(), 1, "one backdrop entry");
    assert!(entries[0].is_backdrop());

    gpu.forget_filters();
    let mut reads = Vec::new();
    for (generation, offset) in [(0_u64, 0.0_f32), (1, 12.0)] {
        let filtered: Vec<Option<terminus::link::Texture>> = gpu
            .prepare_filters(
                &frame,
                &[],
                &[],
                &|_| Some(Vector2D::new(0.0, offset)),
                generation,
                None,
                Color::WHITE,
            )
            .expect("the backdrop bakes")
            .to_vec();
        assert!(filtered[0].is_some(), "offset {offset} baked a texture");
        let mut scene = DisplayList::new();
        frame.compose_into(
            &mut scene,
            &[],
            &filtered,
            &[],
            &|_| Some(Vector2D::new(0.0, offset)),
            None,
        );
        reads.push(
            gpu.render(&scene, 128, 128, Color::WHITE)
                .expect("headless render"),
        );
    }
    let moved = (44..=84_u32)
        .filter(|&y| (luma(&reads[0], 128, 64, y) - luma(&reads[1], 128, 64, y)).abs() > 12)
        .count();
    assert!(
        moved >= 8,
        "the fixed box's backdrop must follow the scroller ({moved} rows differ)",
    );
}

#[test]
fn a_rounded_backdrop_crops_to_its_radius() {
    let mut gpu = headless("a_rounded_backdrop_crops_to_its_radius");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px;
                background-color: #ffffff; }
         .stripe { display: flex; position: absolute; top: 0px;
                   width: 8px; height: 128px; background-color: #000000; }
         .box { display: flex; position: absolute; left: 32px; top: 32px;
                width: 64px; height: 64px; border-radius: 50%;
                backdrop-filter: blur(6px); }";
    let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
    let root = doc.root;
    for index in 0..8_u32 {
        let stripe = doc.el(root, "stripe");
        doc.dom
            .set_inline_style(stripe, &format!("left: {}px", index * 16));
    }
    doc.el(root, "box");
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);

    assert!(
        luma(&pixels, 128, 34, 34) < 12,
        "a black stripe outside the radius is untouched ({})",
        luma(&pixels, 128, 34, 34),
    );
    assert!(
        luma(&pixels, 128, 44, 34) > 243,
        "and so is the white one beside it ({})",
        luma(&pixels, 128, 44, 34),
    );
    for x in [60_u32, 70] {
        let washed = luma(&pixels, 128, x, 64);
        assert!(
            (80..=175).contains(&washed),
            "inside the radius the stripes wash out at x {x} ({washed})",
        );
    }
}

#[test]
fn a_backdrop_mirrors_at_its_crop_rather_than_darkening() {
    let mut gpu = headless("a_backdrop_mirrors_at_its_crop_rather_than_darkening");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px;
                background-color: #ffffff; }
         .frame { display: flex; position: absolute; left: 0px; top: 0px;
                  width: 128px; height: 128px; background-color: #000000; }
         .hole { display: flex; position: absolute; left: 32px; top: 32px;
                 width: 64px; height: 64px; background-color: #ffffff; }
         .box { display: flex; position: absolute; left: 32px; top: 32px;
                width: 64px; height: 64px; backdrop-filter: blur(6px); }";
    let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
    let root = doc.root;
    doc.el(root, "frame");
    doc.el(root, "hole");
    doc.el(root, "box");
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);

    for offset in 0..6_u32 {
        for along in (34..=94_u32).step_by(6) {
            for (x, y) in [
                (32 + offset, along),
                (95 - offset, along),
                (along, 32 + offset),
                (along, 95 - offset),
            ] {
                assert!(
                    luma(&pixels, 128, x, y) >= 250,
                    "({x}, {y}) is {} — the crop's edge darkened",
                    luma(&pixels, 128, x, y),
                );
            }
        }
    }
    assert!(
        luma(&pixels, 128, 30, 64) < 8,
        "and the black outside the element is untouched",
    );
}

#[test]
fn a_backdrop_over_the_budget_stays_unfiltered() {
    let mut gpu = headless("a_backdrop_over_the_budget_stays_unfiltered");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 4000px; height: 4000px; }
         .half { display: flex; position: absolute; left: 0px; top: 0px;
                 width: 2000px; height: 4000px; background-color: #000000; }
         .box { display: flex; position: absolute; left: 500px; top: 500px;
                width: 3000px; height: 3000px; backdrop-filter: blur(8px); }",
        4000.0,
        4000.0,
    );
    let root = doc.root;
    doc.el(root, "half");
    doc.el(root, "box");
    doc.dom.set_device_pixel_ratio(2.0);
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let entries = frame.filter_groups();
    assert_eq!(entries.len(), 1, "the entry is still recorded");
    let (width, height) = entries[0].size();
    assert!(
        u64::from(width) * u64::from(height) > terminus::link::render::blur::MAX_FILTER_AREA,
        "and it is past the budget ({width}x{height})",
    );
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("a refused entry is not an error")
        .to_vec();
    assert_eq!(filtered.len(), 1);
    assert!(
        filtered[0].is_none(),
        "an entry over the budget gets no texture",
    );
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    let pixels = gpu
        .render(&scene, 256, 256, Color::WHITE)
        .expect("the unfiltered fallback renders");
    assert!(
        luma(&pixels, 256, 128, 128) < 8,
        "the fallback leaves the backdrop showing ({})",
        luma(&pixels, 256, 128, 128),
    );
}

#[test]
fn a_colour_only_backdrop_darkens_only_inside_the_box() {
    let mut gpu = headless("a_colour_only_backdrop_darkens_only_inside_the_box");
    let mut doc = backdrop_page(
        128.0,
        (32.0, 32.0, 64.0, 64.0),
        "backdrop-filter: brightness(0.5);",
    );
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    assert_eq!(frame.filter_groups().len(), 1, "one backdrop entry");
    assert!(
        (frame.filter_groups()[0].sigma - 0.0).abs() < f32::EPSILON,
        "a colour-only list has no blur",
    );
    let filtered: Vec<Option<terminus::link::Texture>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("the backdrop bakes")
        .to_vec();
    assert!(
        filtered[0].is_some(),
        "a zero-sigma backdrop still produces its texture",
    );
    let mut scene = DisplayList::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    let pixels = gpu
        .render(&scene, 128, 128, Color::WHITE)
        .expect("headless render");

    let inside = luma(&pixels, 128, 80, 64);
    assert!(
        (110..=145).contains(&inside),
        "the white half is halved inside the box ({inside})",
    );
    assert!(
        luma(&pixels, 128, 80, 16) >= 250,
        "and untouched above it ({})",
        luma(&pixels, 128, 80, 16),
    );
    assert!(
        luma(&pixels, 128, 48, 64) < 8,
        "the black half stays black ({})",
        luma(&pixels, 128, 48, 64),
    );
}

#[test]
fn a_backdrop_over_the_bare_canvas_filters_the_canvas_colour() {
    let mut gpu = headless("a_backdrop_over_the_bare_canvas_filters_the_canvas_colour");
    let page = |extra: &str| {
        let css = format!(
            "page {{ display: flex; position: relative; width: 128px; height: 128px; }}
             .box {{ display: flex; position: absolute; left: 32px; top: 32px;
                     width: 64px; height: 64px; {extra} }}"
        );
        let mut doc = Doc::with_css_sized(&css, 128.0, 128.0);
        let root = doc.root;
        doc.el(root, "box");
        doc
    };
    for (canvas, colour) in [
        (Color::WHITE, [255_u8, 255, 255, 255]),
        (Color::from_rgb8(40, 120, 200), [40, 120, 200, 255]),
    ] {
        let mut doc = page("backdrop-filter: brightness(0.5);");
        let pixels = render_baked_over(&mut gpu, &mut doc, 128, 1, canvas);
        let frame = doc.dom.committed_frame().expect("a committed frame");
        assert!(
            frame.filter_groups()[0].reads_canvas(),
            "the box's Backdrop Root is the document's",
        );
        let inside = pixel(&pixels, 128, 64, 64);
        for channel in 0..3 {
            assert!(
                inside[channel].abs_diff(colour[channel] / 2) <= 3,
                "the canvas {colour:?} is halved inside the box ({inside:?})",
            );
        }
        assert_eq!(
            pixel(&pixels, 128, 16, 64),
            colour,
            "and untouched beside it"
        );

        let mut doc = page("backdrop-filter: blur(6px);");
        let pixels = render_baked_over(&mut gpu, &mut doc, 128, 1, canvas);
        for (x, y) in [(64, 64), (33, 33), (94, 94), (16, 64)] {
            let read = pixel(&pixels, 128, x, y);
            for channel in 0..4 {
                assert!(
                    read[channel].abs_diff(colour[channel]) <= 1,
                    "a blur of the canvas {colour:?} alone is the canvas at ({x}, {y}): {read:?}",
                );
            }
        }
    }
}

#[test]
fn a_canvas_change_re_bakes_a_backdrop_over_it() {
    let mut gpu = headless("a_canvas_change_re_bakes_a_backdrop_over_it");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 64px; height: 64px; }
         .box { display: flex; position: absolute; left: 16px; top: 16px;
                width: 32px; height: 32px; backdrop-filter: brightness(0.5); }",
        64.0,
        64.0,
    );
    let root = doc.root;
    doc.el(root, "box");
    doc.dom.render();
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    for (canvas, colour) in [
        (Color::WHITE, [255_u8, 255, 255]),
        (Color::from_rgb8(40, 120, 200), [40, 120, 200]),
    ] {
        let filtered: Vec<Option<terminus::link::Texture>> = gpu
            .prepare_filters(&frame, &[], &[], &|_| None, 0, None, canvas)
            .expect("the backdrop bakes")
            .to_vec();
        let mut scene = DisplayList::new();
        frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
        let pixels = gpu.render(&scene, 64, 64, canvas).expect("headless render");
        let inside = pixel(&pixels, 64, 32, 32);
        for channel in 0..3 {
            assert!(
                inside[channel].abs_diff(colour[channel] / 2) <= 3,
                "the canvas {colour:?} is halved inside the box ({inside:?})",
            );
        }
    }
}

#[test]
fn a_backdrop_blur_over_the_canvas_softens_an_edge_on_both_sides() {
    let mut gpu = headless("a_backdrop_blur_over_the_canvas_softens_an_edge_on_both_sides");
    let subject = "backdrop-filter: blur(6px);";
    let rect = (32.0, 32.0, 64.0, 64.0);
    let mut bare = seam_page(Ground::Canvas, 128.0, rect, "", subject);
    let bare = render_baked(&mut gpu, &mut bare, 128, 1);

    for distance in 0..=12_u32 {
        let dark = luma(&bare, 128, 63 - distance, 64);
        let light = luma(&bare, 128, 64 + distance, 64);
        assert!(
            (dark + light - 255).abs() <= 6,
            "the blurred edge is symmetric at {distance} px ({dark} dark, {light} light)",
        );
    }
    assert!(
        luma(&bare, 128, 60, 64) > 40,
        "the black side of the edge is lightened, not left sharp ({})",
        luma(&bare, 128, 60, 64),
    );

    let mut painted = seam_page(Ground::Painted, 128.0, rect, "", subject);
    let painted = render_baked(&mut gpu, &mut painted, 128, 1);
    assert_pixels_match(&bare, &painted, 1, "the canvas against a painted ground");
}

#[test]
fn a_transformed_backdrop_over_the_canvas_draws_as_over_a_painted_ground() {
    let mut gpu = headless("a_transformed_backdrop_over_the_canvas_draws_as_over_a_painted_ground");
    let rect = (32.0, 32.0, 64.0, 64.0);
    for (wrapper, subject) in [
        ("", "backdrop-filter: blur(6px); transform: rotate(20deg);"),
        (
            "",
            "backdrop-filter: blur(6px); transform: translate(9.5px, -7px) scale(1.25, 0.8);",
        ),
        (
            "transform: translate(6px, -4px) rotate(-12deg) scale(1.1);",
            "backdrop-filter: blur(6px);",
        ),
        (
            "transform: rotate(8deg);",
            "backdrop-filter: blur(6px); transform: skewX(15deg) scale(0.9);",
        ),
        (
            "transform: rotate(8deg);",
            "backdrop-filter: brightness(0.6) blur(4px); border-radius: 16px;",
        ),
        ("", "backdrop-filter: blur(4px) brightness(0.6);"),
    ] {
        let label = format!("wrapper `{wrapper}`, box `{subject}`");
        let mut bare = seam_page(Ground::Canvas, 128.0, rect, wrapper, subject);
        let bare = render_baked(&mut gpu, &mut bare, 128, 1);
        let mut painted = seam_page(Ground::Painted, 128.0, rect, wrapper, subject);
        let painted = render_baked(&mut gpu, &mut painted, 128, 1);
        assert_pixels_match(&bare, &painted, 1, &label);

        assert!(
            luma(&bare, 128, 61, 64) > 20,
            "{label}: the black side of the edge is filtered ({})",
            luma(&bare, 128, 61, 64),
        );
    }
}

#[test]
fn a_sliding_backdrop_over_the_canvas_composes_as_over_a_painted_ground() {
    let mut gpu = headless("a_sliding_backdrop_over_the_canvas_composes_as_over_a_painted_ground");
    let draw = |gpu: &mut terminus::link::render::gpu::Headless, ground: Ground, wrapper: &str| {
        let mut doc = seam_page(
            ground,
            128.0,
            (16.0, 32.0, 48.0, 64.0),
            wrapper,
            "backdrop-filter: blur(5px); animation: slide 1s linear infinite;",
        );
        doc.dom.add_stylesheet(
            "@keyframes slide { from { transform: translateX(0px) rotate(0deg); }
                                to { transform: translateX(60px) rotate(40deg); } }",
            terminus::link::StylesheetOrigin::Author,
        );
        doc.dom.render();
        doc.dom.advance_animations(0.0);
        doc.dom.advance_animations(0.1);
        doc.dom.render();
        gpu.forget_filters();
        let frame = doc
            .dom
            .committed_frame()
            .expect("render leaves a committed frame retained");
        assert!(frame.has_live_curves(), "the slide exports");
        let [entry] = frame.filter_groups() else {
            panic!("one backdrop, got {:?}", frame.filter_groups());
        };
        assert!(
            entry.reads_canvas() && entry.samples_animations(),
            "the box moves over the document root's image",
        );
        [0.3, 0.7].map(|now| compose_at(gpu, &frame, Some(now), (128, 128)))
    };
    for wrapper in ["", "transform: rotate(-10deg);"] {
        let bare = draw(&mut gpu, Ground::Canvas, wrapper);
        let painted = draw(&mut gpu, Ground::Painted, wrapper);
        for ((bare, painted), now) in bare.iter().zip(&painted).zip([0.3, 0.7]) {
            assert_pixels_match(bare, painted, 1, &format!("wrapper `{wrapper}` at {now} s"));
        }
        assert!(
            (0..128 * 128).any(|index| bare[0][index * 4].abs_diff(bare[1][index * 4]) > 40),
            "wrapper `{wrapper}`: the two instants draw the box in two places",
        );
    }
}

#[test]
fn a_backdrop_inside_an_opacity_group_does_not_read_the_canvas() {
    let mut gpu = headless("a_backdrop_inside_an_opacity_group_does_not_read_the_canvas");
    let page = |extra: &str| {
        let css = format!(
            "page {{ display: flex; position: relative; width: 128px; height: 128px; }}
             .root {{ display: flex; position: absolute; left: 0px; top: 0px;
                      width: 128px; height: 128px; opacity: 0.5; }}
             .bar {{ display: flex; position: absolute; left: 0px; top: 0px;
                     width: 24px; height: 128px; background-color: #000000; }}
             .box {{ display: flex; position: absolute; left: 4px; top: 40px;
                     width: 120px; height: 48px; {extra} }}"
        );
        let mut doc = Doc::with_css_sized(&css, 128.0, 128.0);
        let root = doc.root;
        let wrap = doc.el(root, "root");
        doc.el(wrap, "bar");
        doc.el(wrap, "box");
        doc
    };

    let mut doc = page("backdrop-filter: brightness(0.5);");
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);
    let frame = doc.dom.committed_frame().expect("a committed frame");
    assert!(
        !frame.filter_groups()[0].reads_canvas(),
        "the box's Backdrop Root is the group",
    );
    assert_eq!(
        pixel(&pixels, 128, 80, 64),
        WHITE,
        "the group painted nothing here, so nothing is dimmed",
    );

    let mut doc = page("backdrop-filter: blur(5px);");
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);
    assert_eq!(
        pixel(&pixels, 128, 80, 64),
        WHITE,
        "nothing far from the bar"
    );
    assert!(
        (luma(&pixels, 128, 20, 64) - luma(&pixels, 128, 20, 20)).abs() <= 2,
        "the bar keeps its ink under the box ({} against {})",
        luma(&pixels, 128, 20, 64),
        luma(&pixels, 128, 20, 20),
    );
    assert!(
        luma(&pixels, 128, 27, 64) < luma(&pixels, 128, 27, 20) - 10,
        "and its edge is spread past it ({} against {})",
        luma(&pixels, 128, 27, 64),
        luma(&pixels, 128, 27, 20),
    );
}

#[test]
fn an_anchored_box_follows_its_scrolled_anchor_and_hides_with_it() {
    use terminus::link::Vector2D;

    let mut gpu = headless("an_anchored_box_follows_its_scrolled_anchor_and_hides_with_it");
    let mut doc = Doc::with_css_sized(
        "page { display: flex; position: relative; width: 200px; height: 150px; }
         .scroller { display: flex; flex-direction: column; overflow: scroll;
                     width: 100px; height: 100px; }
         .gap { display: flex; flex-shrink: 0; width: 100px; height: 40px; }
         .anchor { display: flex; flex-shrink: 0; width: 20px; height: 20px;
                   background-color: #0000ff; anchor-name: --a; }
         .tail { display: flex; flex-shrink: 0; width: 100px; height: 300px; }
         .anchored { display: flex; position: absolute; position-anchor: --a;
                     top: anchor(top); left: 120px; width: 40px; height: 20px;
                     background-color: #00ff00; border-radius: 5px;
                     box-shadow: 0 0 4px #000000; filter: blur(1px); }",
        200.0,
        150.0,
    );
    let root = doc.root;
    let scroller = doc.el(root, "scroller");
    doc.el(scroller, "gap");
    doc.el(scroller, "anchor");
    doc.el(scroller, "tail");
    doc.el(root, "anchored");
    doc.dom.render();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let green = |pixel: [u8; 4]| pixel[1] > 200 && pixel[0] < 60 && pixel[2] < 60;

    for (generation, (offset, middle)) in [(0.0_f32, Some(50)), (30.0, Some(20)), (70.0, None)]
        .into_iter()
        .enumerate()
    {
        let offset_of = move |_: &terminus::link::ScrollSlot| Some(Vector2D::new(0.0, offset));
        let filtered: Vec<Option<terminus::link::Texture>> = gpu
            .prepare_filters(
                &frame,
                &[],
                &[],
                &offset_of,
                generation as u64,
                None,
                Color::WHITE,
            )
            .expect("the filter bakes render")
            .to_vec();
        let mut scene = DisplayList::new();
        frame.compose_into(&mut scene, &[], &filtered, &[], &offset_of, None);
        let pixels = gpu
            .render(&scene, 200, 150, Color::WHITE)
            .expect("headless render");
        let column: Vec<u32> = (0..150)
            .filter(|&y| green(pixel(&pixels, 200, 140, y)))
            .collect();
        if let Some(middle) = middle {
            assert!(
                column.contains(&middle) && column.iter().all(|y| y.abs_diff(middle) <= 12),
                "offset {offset}: the box is centred on y {middle}, got {column:?}"
            );
        } else {
            assert!(column.is_empty(), "offset {offset}: hidden, got {column:?}");
            assert_eq!(
                pixel(&pixels, 200, 140, 5),
                WHITE,
                "offset {offset}: no shadow either"
            );
        }
    }
}

fn swatch(scene: &mut DisplayList, texel: [u8; 4], x: f64) {
    let image = prime_radiant::rgba8(16, 16, texel.repeat(256));
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        ImageBrush {
            image: &image,
            sampler: ImageSampler::default(),
        },
        Some(Affine::translate((x, 0.0))),
        &Rect::new(x, 0.0, x + 16.0, 16.0),
    );
}

#[test]
fn targets_on_two_threads_draw_their_own_bitmaps() {
    const NAME: &str = "targets_on_two_threads_draw_their_own_bitmaps";
    let shared = [0, 160, 0, 255];
    let mut scene = DisplayList::new();
    swatch(&mut scene, shared, 16.0);
    let mut first = headless(NAME);
    let pixels = first.render(&scene, 32, 16, Color::WHITE).expect("render");
    assert_eq!(pixel(&pixels, 32, 20, 8), shared);
    let scene = &scene;
    std::thread::scope(|scope| {
        for side in 0..2_u8 {
            scope.spawn(move || {
                let mut gpu = headless(NAME);
                for frame in 0..40_u8 {
                    if side == 1 && frame == 20 {
                        drop(gpu);
                        gpu = headless(NAME);
                    }
                    let own = [255 - side * 200, frame * 6, side * 200, 255];
                    let mut list = scene.clone();
                    swatch(&mut list, own, 0.0);
                    let pixels = gpu.render(&list, 32, 16, Color::WHITE).expect("render");
                    assert_eq!(
                        pixel(&pixels, 32, 8, 8),
                        own,
                        "target {side}, frame {frame}"
                    );
                    assert_eq!(pixel(&pixels, 32, 24, 8), shared, "target {side}");
                }
            });
        }
    });
    drop(first);
}
