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
        .text { display: -lynx-text; position: absolute; left: 10px; top: 10px;
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
        .render(&scene, &[], 200, 100, Color::WHITE)
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
        .text { display: -lynx-text; position: absolute; left: 10px; top: 10px;
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
        .render(&scene, &[], 200, 100, Color::WHITE)
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
        .text { display: -lynx-text; position: absolute; left: 0px; top: 10px;
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
        .render(&scene, &[], 200, 100, Color::WHITE)
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

/// Renders a 100px square at the page origin with `background-image: <image>`
/// over a white page.
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
    let scene = doc.dom.scene(&dom::NoImages);
    gpu.render(&scene, &[], 200, 100, Color::WHITE)
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
    // The stop at -50% re-maps the ramp so the hard edge at 50% is its end.
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
    let scene = doc.dom.scene(&dom::NoImages);
    let pixels = gpu
        .render(&scene, &[], 200, 100, Color::WHITE)
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
        .render(&scene, &[], 128, 128, Color::WHITE)
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
            &[],
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

/// One 64x64 scene drawing `image` over the whole target, with nearest
/// sampling so a texel reads back exactly and a filtered edge cannot be
/// mistaken for a wrong atlas.
fn image_scene(image: &dom::vello::peniko::ImageData, at: Affine) -> Scene {
    use dom::vello::peniko::{ImageBrush, ImageQuality, ImageSampler};

    let mut scene = Scene::new();
    scene.draw_image(
        ImageBrush {
            image,
            sampler: ImageSampler::default().with_quality(ImageQuality::Low),
        },
        at,
    );
    scene
}

/// A scene of solid paths only, which is the shape that frees the atlas.
fn solid_scene() -> Scene {
    let mut scene = Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(0x11, 0x22, 0x33),
        None,
        &Rect::new(0.0, 0.0, 64.0, 64.0),
    );
    scene
}

/// An intervening image-free render costs the atlas its contents;
/// `Headless::render_frame`'s residency bookkeeping is what a target survives
/// it by, and the render entry point does it, so no caller can forget.
///
/// Mechanism, confirmed in vello 0.10.0's own source and unchanged in 0.11.0
/// (the cited files are byte-identical across the two): an encoding with no
/// patches at all — solid paths only, so no image, no gradient ramp and no
/// glyph run — takes `Resolver::resolve`'s early return and reports
/// `Images::default()` (`vello_encoding/src/resolve.rs:187-191`). That
/// zero-sized report clamps the atlas to 1x1
/// (`vello/src/render.rs:160-161`), which no longer matches the persistent
/// proxy, so the renderer frees the real atlas texture and installs a 1x1
/// one (`render.rs:166-171`). `ImageCache` is not told: its entries stay
/// resident and clean (`image_cache.rs:148-160`), so the next render would
/// re-use the freed slot and sample nothing. The control render below pins
/// that renderer reuse alone is fine — only the intervening patch-free
/// render breaks it.
#[test]
fn an_image_survives_an_intervening_image_free_render() {
    let mut gpu = headless("an_image_survives_an_intervening_image_free_render");

    // A 2x2 image whose four texels are distinct, so a blank-atlas read is
    // not mistakable for a correct one; 2x2 texels scaled to fill the target.
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
    let drawn = [Some(image.clone())];
    let draw_image = || image_scene(&image, Affine::scale(32.0));

    let first = gpu
        .render(&draw_image(), &drawn, 64, 64, Color::WHITE)
        .expect("first image render");
    assert_eq!(
        pixel(&first, 64, 16, 16),
        [255, 0, 0, 255],
        "the first render must show the image's own texels"
    );

    // Control: back-to-back image renders, nothing in between. This isolates
    // the intervening render as the cause rather than renderer reuse itself.
    let control = gpu
        .render(&draw_image(), &drawn, 64, 64, Color::WHITE)
        .expect("control image render");
    assert_eq!(
        control, first,
        "two image renders in a row must agree — renderer reuse alone is fine"
    );

    let _ = gpu
        .render(&solid_scene(), &[], 64, 64, Color::WHITE)
        .expect("intervening image-free render");
    let third = gpu
        .render(&draw_image(), &drawn, 64, 64, Color::WHITE)
        .expect("image render after the image-free one");

    assert_eq!(
        pixel(&third, 64, 16, 16),
        [255, 0, 0, 255],
        "an image-free render between two identical image renders must not \
         change what the second one draws"
    );
}

/// The repair is owed per image, not per frame: a loss is repaired at each
/// resident image's first later use, however many frames that takes.
///
/// A frame drawing both images, then a patch-free one, then a frame drawing
/// only the first — which repairs only the first — and then both again. The
/// second image's pixels in that last render are the whole test: a scheme
/// that only re-marked the images of the first frame after a loss would leave
/// it sampling the freed slot forever.
#[test]
fn a_frame_after_the_atlas_is_lost_repairs_only_the_images_it_draws() {
    let mut gpu = headless("a_frame_after_the_atlas_is_lost_repairs_only_the_images_it_draws");

    let red = flashbulb::rgba8(1, 1, vec![255, 0, 0, 255]);
    let blue = flashbulb::rgba8(1, 1, vec![0, 0, 255, 255]);
    // Side by side, each filling half of the 64x64 target.
    let both = || {
        let mut scene = image_scene(&red, Affine::scale_non_uniform(32.0, 64.0));
        scene.append(
            &image_scene(&blue, Affine::scale_non_uniform(32.0, 64.0)),
            Some(Affine::translate((32.0, 0.0))),
        );
        scene
    };
    let both_drawn = [Some(red.clone()), Some(blue.clone())];
    let red_drawn = [Some(red.clone())];

    let _ = gpu
        .render(&both(), &both_drawn, 64, 64, Color::WHITE)
        .expect("both images render");
    let _ = gpu
        .render(&solid_scene(), &[], 64, 64, Color::WHITE)
        .expect("the render that frees the atlas");
    let only_red = gpu
        .render(
            &image_scene(&red, Affine::scale_non_uniform(32.0, 64.0)),
            &red_drawn,
            64,
            64,
            Color::WHITE,
        )
        .expect("one image render");
    assert_eq!(pixel(&only_red, 64, 16, 32), RED, "the repaired image");

    let again = gpu
        .render(&both(), &both_drawn, 64, 64, Color::WHITE)
        .expect("both images render again");
    assert_eq!(pixel(&again, 64, 16, 32), RED);
    assert_eq!(
        pixel(&again, 64, 48, 32),
        BLUE,
        "the image no frame drew since the loss must still be repaired"
    );
}

/// A scroll is a translation applied while the committed frame is put
/// together, and the scrollport clip does not ride it: the content under the
/// port moves, and nothing leaks out beside or below it at any offset inside
/// the encode window.
#[test]
fn a_composition_at_a_scroll_offset_stays_inside_the_scrollport() {
    use dom::Vector2D;

    let mut gpu = headless("a_composition_at_a_scroll_offset_stays_inside_the_scrollport");
    // A 100x100 scroller over a red then a blue 100px row, on a 200x150 page,
    // so pixels beside and below the scroller prove the clip holds.
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

    // Screen y plus the offset is content y, so each probe names the row the
    // port shows there.
    for (offset, near, far) in [(0.0_f32, RED, RED), (30.0, RED, BLUE), (100.0, BLUE, BLUE)] {
        let mut scene = Scene::new();
        frame.compose_into(
            &mut scene,
            &[],
            &[],
            &[],
            &|_| Some(Vector2D::new(0.0, offset)),
            None,
        );
        let pixels = gpu
            .render(&scene, &[], 200, 150, Color::WHITE)
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

/// A nested scope paints in its own colour, not the paragraph root's.
///
/// This is the property per-run painting exists for, and the one thing the
/// golden suites cannot check: they would pass identically if every glyph in a
/// paragraph still wore the establishing element's style. Ahem gives every
/// glyph a full em square, so each run's colour is readable at an exact pixel.
#[test]
fn a_nested_scope_paints_in_its_own_colour() {
    let mut gpu = headless("a_nested_scope_paints_in_its_own_colour");
    let css = "page { display: flex; position: relative; width: 200px; height: 100px; }
        .text { display: -lynx-text; position: absolute; left: 0px; top: 10px;
                width: 200px; height: 50px;
                font-family: Ahem; font-size: 20px; color: #ff0000; }
        .scope { display: -lynx-text; color: #0000ff; }";
    let mut doc = Doc::with_css(css);
    doc.dom.register_fonts(dom::FontBlob::from_static(AHEM));
    let root = doc.root;
    let holder = doc.el(root, "text");
    // "AA" in the root's red, then a nested scope's "BB" in blue.
    doc.text(holder, "AA");
    let scope = doc.el(holder, "scope");
    doc.text(scope, "BB");

    doc.dom.render();
    let scene = doc.dom.scene(&dom::NoImages);
    let pixels = gpu
        .render(&scene, &[], 200, 100, Color::WHITE)
        .expect("headless render");

    // First em square is the root run, third is the nested scope's.
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

// ---------------------------------------------------------------------------
// `filter: blur()`
//
// The whole blur path is only observable in pixels: the compose program's
// bracket ops replay raw without a GPU, so every property below — that the
// bake happens at all, that it is premultiplied, that its margin survives the
// viewport edge, that a decimated sigma still centres — needs a real render.
// The assertions are analytic rather than golden: a monotone profile, a
// symmetry, an ink bound at 3 sigma.
// ---------------------------------------------------------------------------

/// A page with one absolutely-positioned square carrying `extra` declarations.
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

/// Renders a document through the same path an embedder's painter takes: the
/// committed frame, its filter bakes, then the composition.
fn render_filtered(gpu: &mut dom::render::gpu::Headless, doc: &mut Doc, size: u32) -> Vec<u8> {
    doc.dom.render();
    // One `Headless` here renders several independent documents, and commit
    // ids restart at one per document, so the bake cache's key cannot tell
    // them apart on its own.
    gpu.forget_filters();
    let frame = doc
        .dom
        .committed_frame()
        .expect("render leaves a committed frame retained");
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    gpu.render(&scene, &[], size, size, Color::WHITE)
        .expect("headless render")
}

/// The luminance of one pixel, 0 for black ink and 255 for the white page.
fn luma(pixels: &[u8], width: u32, x: u32, y: u32) -> i32 {
    i32::from(pixel(pixels, width, x, y)[0])
}

/// A black square under `blur(4px)`: the centre keeps the fill, the profile
/// across an edge is monotone and symmetric about the border, ink exists
/// outside the box within 3 sigma, and effectively none past 4 sigma.
#[test]
fn a_blurred_square_spreads_monotonically_and_symmetrically() {
    let mut gpu = headless("a_blurred_square_spreads_monotonically_and_symmetrically");
    let mut doc = blur_page(128.0, 48.0, "filter: blur(4px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 128);

    // The square is 48px at (40, 40); sigma is 4, so the right border is at
    // x = 88 and the ink cutoff at 3 sigma is x = 100.
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
    // The border is at x = 88.0, so the two pixels straddling it are 87
    // (centre 87.5) and 88 (centre 88.5) — the symmetric pair, and the pair
    // whose mean is the coverage at the border itself.
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

/// A WHITE square blurred over a WHITE page stays white everywhere.
///
/// This is the premultiply pass's test and nothing else's: vello writes its
/// render target unpremultiplied, so filtering it directly averages the
/// colour of the transparent margin — which is whatever the divide by a tiny
/// alpha left there — into the square's own edge, and a dark halo appears.
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

/// A sigma small enough to need no decimation blurs correctly.
///
/// That is its own code path, and the one place a texture is both read and
/// written inside one bake: with no pyramid the horizontal half-pass writes
/// back into the bake target, which the premultiply pass consumed. A missing
/// barrier or an aliasing mistake there shows up as a smeared or doubled
/// profile, so the same monotone-and-symmetric assertions apply.
#[test]
fn an_undecimated_blur_reuses_the_bake_target_correctly() {
    let mut gpu = headless("an_undecimated_blur_reuses_the_bake_target_correctly");
    // Sigma 2 is exactly the largest the kernel covers, so this bakes at
    // level 0 and allocates no pong at all.
    let mut doc = blur_page(128.0, 48.0, "filter: blur(2px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 128);

    assert!(
        luma(&pixels, 128, 64, 64) < 8,
        "the centre keeps the fill ({})",
        luma(&pixels, 128, 64, 64),
    );
    // The right border is x = 88.0, so 87 and 88 are the symmetric pair.
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

/// A sigma large enough to force decimation still centres on the box and
/// stays symmetric — the box downsample and the tent upsample have to agree
/// about where the pixel grid is, or the result slides by half a level.
#[test]
fn a_decimated_blur_stays_centred_and_symmetric() {
    let mut gpu = headless("a_decimated_blur_stays_centred_and_symmetric");
    let mut doc = blur_page(256.0, 64.0, "filter: blur(16px);");
    let pixels = render_filtered(&mut gpu, &mut doc, 256);

    // The square is 64 px at (96, 96), so its centre is x = y = 128.0 and the
    // symmetric pixel pairs are `127 − k` (centre 127.5 − k) against
    // `128 + k` (centre 128.5 + k).
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

/// A blurred square straddling the viewport edge shows exactly the pixels the
/// same square fully inside shows, translated.
///
/// The proof that the group's bake keeps its 3 sigma margin *outside* the
/// viewport: inflating the layer bounds after the viewport intersection would
/// cut the margin the visible pixels read from, and the edge would come out
/// darker than it should.
#[test]
fn a_blurred_square_straddling_the_viewport_edge_matches_one_inside() {
    let mut gpu = headless("a_blurred_square_straddling_the_viewport_edge_matches_one_inside");
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
         .box { display: flex; position: absolute; top: 40px;
                width: 48px; height: 48px; background-color: #000000;
                filter: blur(4px); }";

    let read = |gpu: &mut dom::render::gpu::Headless, left: f32| -> Vec<u8> {
        let mut doc = Doc::with_css_sized(css, 128.0, 128.0);
        let root = doc.root;
        let boxed = doc.el(root, "box");
        doc.dom.set_inline_style(boxed, &format!("left: {left}px"));
        render_filtered(gpu, &mut doc, 128)
    };
    // 40 px inside, then 24 px off the left edge: the same square, shifted by
    // 64 px, so the pixel at x is the pixel at x + 64 of the inside render.
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

/// A blurred box inside a scroller composes at the painter's offset and stays
/// inside the scrollport.
///
/// The blur is baked in the scroller's own chain, so the texture has to move
/// with the offset while the scrollport clip does not.
#[test]
fn a_blurred_box_in_a_scroller_moves_with_the_offset() {
    use dom::Vector2D;

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
        let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
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
        let mut scene = Scene::new();
        frame.compose_into(
            &mut scene,
            &[],
            &filtered,
            &[],
            &|_| Some(Vector2D::new(0.0, offset)),
            None,
        );
        let pixels = gpu
            .render(&scene, &[], 200, 200, Color::WHITE)
            .expect("headless render");
        // The box's content-space centre is y = 80, so it shows at 80 - offset.
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

/// A blurred card sliding by an exported curve inside an ancestor's
/// `overflow: clip`, composed later than its commit, is still cut at the
/// ancestor's edge rather than at where that edge sat relative to the card
/// when it was committed.
///
/// The ancestor's clip is the card's output clip, pushed in the ancestor's
/// still space outside the bake. The bake is the card's own content, which
/// rides the slide with it, so it does not sample the instant: the
/// composition moves the texture and cuts it at the still edge.
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

    // Committed at x = 30; at 0.6 s the card spans x = 80..140, and the
    // frame's clip ends at x = 100.
    let now = Some(0.6);
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, now, Color::WHITE)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, now);
    let pixels = gpu
        .render(&scene, &[], 200, 100, Color::WHITE)
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

/// Bakes `frame`'s filter entries and composes it at `now`, the way an
/// embedder's painter draws one frame.
fn compose_at(
    gpu: &mut dom::render::gpu::Headless,
    frame: &dom::CommittedFrame,
    now: Option<f64>,
    (width, height): (u32, u32),
) -> Vec<u8> {
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(frame, &[], &[], &|_| None, 0, now, Color::WHITE)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, now);
    gpu.render(&scene, &[], width, height, Color::WHITE)
        .expect("headless render")
}

/// Every channel of `a` within `slack` of `b`'s.
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

/// A child sliding out of a still `opacity: 0.5` parent, composed later than
/// its commit, draws where a fresh commit at that instant draws it: the
/// parent's group rect holds the whole slide, not the child's committed box.
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

    // Committed at x = 30..70 inside the group's 20..80; at 0.6 s the card
    // spans x = 80..120, wholly past the group's box.
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

/// A blurred group with a sliding child beside a still blurred box, composed
/// at two instants of one commit: the second re-bakes the sliding group
/// alone, and both frames draw what fresh commits at those instants draw.
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

    // Both instants of `early` first: a later commit's bakes would replace
    // its textures.
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

/// One element with both `filter: blur()` and `backdrop-filter` beside a
/// sliding card, composed at two instants of one commit: the second re-bakes
/// the backdrop, and the blur group that draws it with it, so both frames
/// draw what fresh commits at those instants draw.
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
        groups.iter().all(dom::FilterGroup::samples_animations),
        "both sample the timeline: the backdrop reads the slide, the group draws the backdrop",
    );

    // Both instants of `early` first: a later commit's bakes would replace
    // its textures. The card spans x = 80..110 at 0.4 s and 120..150 at
    // 0.6 s, both under the element.
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

/// A card fading by an exported curve, committed while its opacity reads 1,
/// is the Backdrop Root of the `backdrop-filter` box inside it: composed
/// later, the box filters the card alone, as a fresh commit at that instant
/// does, and not the red page behind the card.
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

/// The same element over a scroller, composed at a second offset of one
/// commit: the cache re-bakes the backdrop and the group drawing it, so the
/// frame is the one a cold bake at that offset draws.
#[test]
fn a_blurred_backdrop_over_a_scroller_re_bakes_with_it() {
    use dom::Vector2D;

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

    let draw = |gpu: &mut dom::render::gpu::Headless, generation: u64, offset: f32| {
        let offset_of = |_: &dom::ScrollSlot| Some(Vector2D::new(0.0, offset));
        let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
            .prepare_filters(&frame, &[], &[], &offset_of, generation, None, Color::WHITE)
            .expect("the entries bake")
            .to_vec();
        assert!(filtered.iter().all(Option::is_some), "both entries baked");
        let mut scene = Scene::new();
        frame.compose_into(&mut scene, &[], &filtered, &[], &offset_of, None);
        gpu.render(&scene, &[], 128, 128, Color::WHITE)
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

/// A page of 4 px black stripes on 8 px centres over white, which any blur
/// visibly greys: a pixel reading pure black or pure white is one no filter
/// reached. The page paints its own white ground, so a test over it reads
/// nothing of the canvas; the canvas tests further down cover a page that
/// paints none.
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

/// A `backdrop-filter` element hanging half out of an `overflow: hidden`
/// card is cut at the card's edge: past it the page shows unfiltered, the
/// same pixels a page without the element shows, and hit testing there
/// answers the page rather than the element.
///
/// css-overflow-3 clips everything a box's descendants paint, and
/// filter-effects-2 composites the filtered backdrop into the element's
/// parent, inside that clip.
#[test]
fn a_backdrop_half_outside_an_overflow_hidden_card_stops_at_its_edge() {
    use euclid::default::Point2D;

    let mut gpu = headless("a_backdrop_half_outside_an_overflow_hidden_card_stops_at_its_edge");
    // The card spans x 20..100; the frost spans x 60..140, so its right half
    // lies outside the card.
    let css = ".card { display: flex; position: absolute; left: 20px; top: 20px;
                       width: 80px; height: 60px; overflow: hidden; }
               .frost { display: flex; position: absolute; left: 40px; top: 10px;
                        width: 80px; height: 40px; }
               .frosted { backdrop-filter: blur(8px); }";
    let read = |gpu: &mut dom::render::gpu::Headless, class: &str| {
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
            .any(dom::FilterGroup::is_backdrop),
        "the frost records a backdrop",
    );

    // Inside the card the backdrop is filtered: x = 81 is a black stripe's
    // centre, which the blur greys.
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
    // Outside it the page shows exactly as it does with no frost at all.
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

/// A blurred box whose right edge meets its `overflow: hidden` parent's shows
/// its 3σ halo inside the parent and none past the parent's edge — and the
/// same holds for that box inside a second, outer blurred group, whose own
/// blur is the only ink allowed past the edge.
#[test]
fn a_blur_halo_stops_at_its_ancestors_overflow_clip() {
    let mut gpu = headless("a_blur_halo_stops_at_its_ancestors_overflow_clip");
    // The card spans x 16..80 and the box x 40..80, so the box's right edge
    // is the card's: a 4 px blur spreads 12 px of halo past it, to x = 92.
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
         .outer { display: flex; position: absolute; left: 0px; top: 0px;
                  width: 128px; height: 128px; }
         .card { display: flex; position: absolute; left: 16px; top: 16px;
                 width: 64px; height: 96px; overflow: hidden; }
         .box { display: flex; position: absolute; left: 24px; top: 24px;
                width: 40px; height: 48px; background-color: #000000;
                filter: blur(4px); }";
    let read = |gpu: &mut dom::render::gpu::Headless, outer: &str| -> Vec<u8> {
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

    // The outer group blurs the card's clipped content by 1 px more, so ink
    // may reach 3 px past the edge and no further.
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

/// A blurred box straddling its `overflow: hidden` parent's edge reads, just
/// inside the edge, what it reads with the parent's overflow visible — the
/// blur spreads the box's ink from past the edge back inside it — and past
/// the edge it shows nothing. A box wholly past the edge, but within 3σ of
/// it, still spreads ink back inside.
///
/// CSS clips the blur's *output* by the ancestor's clip, not its input: the
/// group renders whole, is blurred, and only then is cut.
#[test]
fn a_blur_straddling_an_overflow_clip_reads_its_content_past_the_edge() {
    let mut gpu = headless("a_blur_straddling_an_overflow_clip_reads_its_content_past_the_edge");
    // The card spans x 16..80. The straddling box spans x 40..120, so 40 px of
    // it lie past the edge; the outside one spans x 84..124, 4 px past it.
    let css = "page { display: flex; position: relative; width: 128px; height: 128px; }
         .card { display: flex; position: absolute; left: 16px; top: 16px;
                 width: 64px; height: 96px; }
         .box { display: flex; position: absolute; top: 24px; height: 48px;
                background-color: #000000; filter: blur(4px); }";
    let read = |gpu: &mut dom::render::gpu::Headless, overflow: &str, (left, width): (f32, f32)| {
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

/// A backdrop nested in a `backdrop-filter` root that straddles an
/// `overflow: hidden` card reads the root's content past the card's edge:
/// just inside the edge it draws what it draws with the card's overflow
/// visible, and past the edge the page shows unfiltered.
///
/// The root's Backdrop Root Image is its group's content, which the card
/// clips only when the root's output is composited — after the nested
/// element has read it.
#[test]
fn a_nested_backdrop_reads_its_roots_content_past_an_ancestors_edge() {
    let mut gpu = headless("a_nested_backdrop_reads_its_roots_content_past_an_ancestors_edge");
    // The card spans x 20..100; the root and the nested box span x 60..140.
    let css = ".card { display: flex; position: absolute; left: 20px; top: 20px;
                       width: 80px; height: 60px; }
               .root { display: flex; position: absolute; left: 40px; top: 10px;
                       width: 80px; height: 40px; backdrop-filter: brightness(1); }
               .frost { display: flex; position: absolute; left: 0px; top: 0px;
                        width: 80px; height: 40px; backdrop-filter: blur(4px); }";
    let read = |gpu: &mut dom::render::gpu::Headless, overflow: Option<&str>| {
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

/// A blurred child inside a blurred parent renders, and blurs more than
/// either blur alone.
///
/// Nested bakes are ordered by op range end, so the child's texture exists
/// before the parent's bake replays the op that draws it.
#[test]
fn a_nested_blur_blurs_more_than_either_alone() {
    let mut gpu = headless("a_nested_blur_blurs_more_than_either_alone");
    let css = "page { display: flex; position: relative; width: 192px; height: 192px; }
         .outer { display: flex; position: absolute; left: 64px; top: 64px;
                  width: 64px; height: 64px; }
         .inner { display: flex; width: 64px; height: 64px;
                  background-color: #000000; }";

    let read = |gpu: &mut dom::render::gpu::Headless, outer: &str, inner: &str| -> Vec<u8> {
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

    // 6 px outside the box's right border (x = 128), where more blur means
    // more ink and therefore a lower luminance.
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

/// A group whose bake would exceed the area budget renders unblurred rather
/// than not at all, and nothing panics.
///
/// The group's rect is device pixels, so a large enough device pixel ratio
/// puts a modest CSS box past `MAX_FILTER_DIMENSION` without a huge viewport.
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
        u64::from(width) * u64::from(height) <= dom::render::blur::MAX_FILTER_AREA,
        "this group is inside the budget",
    );

    // The bake is refused by making the *page* ask for more than the budget
    // allows: a 3000x3000 blurred box at DPR 2 is 36 Mpx of device area
    // against a 16.7 Mpx cap.
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
        u64::from(width) * u64::from(height) > dom::render::blur::MAX_FILTER_AREA,
        "and it is past the budget ({width}x{height})",
    );
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("a refused group is not an error")
        .to_vec();
    assert_eq!(filtered.len(), 1);
    assert!(
        filtered[0].is_none(),
        "a group over the budget gets no texture"
    );
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    // Rendered at a viewport-sized target, which is all the retained scene is
    // valid for; the point is that the unblurred fallback draws.
    let pixels = gpu
        .render(&scene, &[], 256, 256, Color::WHITE)
        .expect("the unblurred fallback renders");
    assert!(
        luma(&pixels, 256, 128, 128) < 8,
        "the fallback paints the box, hard-edged ({})",
        luma(&pixels, 256, 128, 128),
    );
}

// ---------------------------------------------------------------------------
// `backdrop-filter`
//
// Every property here is a relationship between what is inside the element's
// border box and what is beside it, so the assertions compare the two rather
// than naming absolute colours: the crop is the border box, the source is
// what was painted before the element inside its Backdrop Root, and the
// element's own painting composites over the result.
// ---------------------------------------------------------------------------

/// A page whose left half is black and right half is white, with one
/// absolutely positioned box carrying `extra` over the seam.
///
/// The seam is the subject: a blur turns the step into a gradient, and every
/// pixel of that gradient is inside the box while every pixel outside it
/// stays a hard edge. The page paints its own opaque white ground;
/// [`seam_page`] is the same page with the ground left to the canvas.
fn backdrop_page(size: f32, box_rect: (f32, f32, f32, f32), extra: &str) -> Doc {
    seam_page(Ground::Painted, size, box_rect, "", extra)
}

/// What lies under a [`seam_page`]'s black half and its box.
#[derive(Clone, Copy)]
enum Ground {
    /// An opaque white background the page paints itself.
    Painted,
    /// Nothing: the canvas, which is the render's base colour.
    Canvas,
}

/// [`backdrop_page`] over `ground`, with the box inside a viewport-sized
/// wrapper carrying `wrapper`.
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

/// [`render_filtered`] with the two facts a backdrop test rests on asserted
/// first: the frame recorded `entries` of them, and every one of them baked.
///
/// Without that, a backdrop test asserting "what is beside the element is
/// untouched" would pass just as happily with no backdrop drawn at all.
fn render_baked(
    gpu: &mut dom::render::gpu::Headless,
    doc: &mut Doc,
    size: u32,
    entries: usize,
) -> Vec<u8> {
    render_baked_over(gpu, doc, size, entries, Color::WHITE)
}

/// [`render_baked`] over a canvas of `canvas`.
fn render_baked_over(
    gpu: &mut dom::render::gpu::Headless,
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
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, canvas)
        .expect("the bakes render")
        .to_vec();
    assert!(
        filtered.iter().all(Option::is_some),
        "every recorded entry baked a texture",
    );
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    gpu.render(&scene, &[], size, size, canvas)
        .expect("headless render")
}

/// Inside the box the black/white seam is a monotone gradient; outside it the
/// same seam is still one hard step; and the box's own translucent background
/// composites over the filtered backdrop rather than under it.
#[test]
fn a_backdrop_blur_softens_the_seam_only_inside_the_box() {
    let mut gpu = headless("a_backdrop_blur_softens_the_seam_only_inside_the_box");
    let mut doc = backdrop_page(
        128.0,
        (32.0, 32.0, 64.0, 64.0),
        "backdrop-filter: blur(6px); background-color: rgb(255 0 0 / 20%);",
    );
    let pixels = render_baked(&mut gpu, &mut doc, 128, 1);

    // Inside the box (y = 64), across the seam at x = 64: monotone and never
    // a step, because sigma 6 spreads it over some 36 px.
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

    // Outside the box (y = 16), the same seam is one step.
    assert!(
        luma(&pixels, 128, 60, 16) < 8,
        "unfiltered black left of it"
    );
    assert!(
        luma(&pixels, 128, 68, 16) >= 250,
        "unfiltered white right of it",
    );

    // The box's own 20% red over the blurred backdrop: on the white side the
    // red channel stays high while green falls, which an unfiltered backdrop
    // with no box on top could not produce.
    let over_white = pixel(&pixels, 128, 88, 64);
    assert!(
        over_white[0] > 240 && over_white[1] < 215,
        "the box's own background composites on top ({over_white:?})",
    );
}

/// A later sibling overlapping the box is not part of its backdrop.
///
/// The Backdrop Root Image is everything painted *before* the element; a box
/// drawn after it is in front of both the element and its backdrop, and shows
/// with a hard edge.
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

    // `.under` is painted before the box, so inside the box its edges (x = 72
    // and x = 88) are spread and its centre is no longer solid.
    assert!(
        (40..=190).contains(&luma(&pixels, 128, 80, 64)),
        "the earlier bar is blurred inside the box ({})",
        luma(&pixels, 128, 80, 64),
    );
    // `.over` is painted after it, so its edges (x = 32 and x = 48) are not.
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

/// An `opacity` ancestor is a Backdrop Root: the element behind it is not in
/// its descendant's backdrop, and the descendant's own sibling is.
///
/// filter-effects-2 §2.2. The whole point of the rule is that an ancestor
/// which flattens its subtree hides everything behind it, so an element
/// inside it cannot read through.
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

    // `.outside`'s edge is at x = 64, and it is painted before the Backdrop
    // Root: inside the box it must still be one step, exactly as it is above
    // the box.
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

    // `.inside`'s edge is at x = 24, inside the Backdrop Root and before the
    // box: under the box its white spreads onto the black, above the box it
    // does not.
    let spread = luma(&pixels, 128, 28, 64) - luma(&pixels, 128, 28, 20);
    assert!(
        spread > 10,
        "the edge inside the Backdrop Root is blurred under the box ({spread})",
    );
}

/// A `position: fixed` box over a scroller re-bakes as the scroller moves:
/// its backdrop rides a chain the box itself does not.
///
/// This is `inner_chains` on a backdrop entry. The box never moves, so a
/// bake that ignored the offset would show the same pixels forever.
#[test]
fn a_fixed_backdrop_over_a_scroller_rebakes_per_offset() {
    use dom::Vector2D;

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
        let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
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
        let mut scene = Scene::new();
        frame.compose_into(
            &mut scene,
            &[],
            &filtered,
            &[],
            &|_| Some(Vector2D::new(0.0, offset)),
            None,
        );
        reads.push(
            gpu.render(&scene, &[], 128, 128, Color::WHITE)
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

/// The crop is the element's *rounded* border box: a pixel inside the bounding
/// box but outside the radius shows the page untouched.
///
/// The backdrop is eight-pixel stripes, which a σ = 6 blur washes to mid
/// gray, so every pixel the crop admits changes and every pixel it refuses
/// keeps its stripe exactly.
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

    // y = 34 is inside the bounding box and outside the circle (the corner
    // is 42 px from the centre, the radius is 32).
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
    // y = 64 is the centre line; these two are deep inside the circle and far
    // enough from the crop's own edges that the mirror is not what they read.
    // One sits in a black stripe and one in a white gap, and both wash to the
    // same middle.
    for x in [60_u32, 70] {
        let washed = luma(&pixels, 128, x, 64);
        assert!(
            (80..=175).contains(&washed),
            "inside the radius the stripes wash out at x {x} ({washed})",
        );
    }
}

/// A backdrop that is uniform white inside the crop and black just outside it
/// stays white to the element's own edge.
///
/// The mirror edge mode is what makes that true. A bake that read transparent
/// black past the crop — which is the edge mode a `filter: blur()` group's
/// margin deliberately supplies — would leave a dark rim one σ wide all the
/// way around.
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

/// A backdrop whose bake would exceed the area budget draws nothing, leaving
/// the unfiltered backdrop showing.
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
        u64::from(width) * u64::from(height) > dom::render::blur::MAX_FILTER_AREA,
        "and it is past the budget ({width}x{height})",
    );
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("a refused entry is not an error")
        .to_vec();
    assert_eq!(filtered.len(), 1);
    assert!(
        filtered[0].is_none(),
        "an entry over the budget gets no texture",
    );
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    let pixels = gpu
        .render(&scene, &[], 256, 256, Color::WHITE)
        .expect("the unfiltered fallback renders");
    // The seam is at device x = 4000, far outside this target; everything
    // drawn here is the black half, hard-edged and unfiltered.
    assert!(
        luma(&pixels, 256, 128, 128) < 8,
        "the fallback leaves the backdrop showing ({})",
        luma(&pixels, 256, 128, 128),
    );
}

/// A colour-only `backdrop-filter` bakes at σ = 0 and still isolates its
/// copy: the backdrop is darkened inside the element's box and nowhere else.
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
    let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
        .prepare_filters(&frame, &[], &[], &|_| None, 0, None, Color::WHITE)
        .expect("the backdrop bakes")
        .to_vec();
    assert!(
        filtered[0].is_some(),
        "a zero-sigma backdrop still produces its texture",
    );
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
    let pixels = gpu
        .render(&scene, &[], 128, 128, Color::WHITE)
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

// The canvas. filter-effects-2's Backdrop Root Image of the document's root
// element holds the canvas under whatever the page painted, and here the
// canvas is the render's base colour, which no op of the frame draws. Most of
// these render one page twice — over the bare canvas, and over an opaque
// ground of the canvas's colour that the page paints itself — and hold the
// two to the same pixels, so they need no opinion on what a filter looks
// like.

/// A box over nothing but the canvas filters the canvas: `brightness(0.5)`
/// halves the canvas colour inside the border box and leaves it beside it,
/// and a `blur()` of that one colour is that colour.
///
/// Nothing is painted before the box, so its range is empty and the canvas is
/// its whole backdrop.
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

/// One committed frame drawn over two canvas colours through one renderer:
/// the second draw filters the second canvas, not the bake the first left in
/// the cache.
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
        let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
            .prepare_filters(&frame, &[], &[], &|_| None, 0, None, canvas)
            .expect("the backdrop bakes")
            .to_vec();
        let mut scene = Scene::new();
        frame.compose_into(&mut scene, &[], &filtered, &[], &|_| None, None);
        let pixels = gpu
            .render(&scene, &[], 64, 64, canvas)
            .expect("headless render");
        let inside = pixel(&pixels, 64, 32, 32);
        for channel in 0..3 {
            assert!(
                inside[channel].abs_diff(colour[channel] / 2) <= 3,
                "the canvas {colour:?} is halved inside the box ({inside:?})",
            );
        }
    }
}

/// A box straddling the edge of a black half over the bare canvas blurs that
/// edge on both sides: the profile is symmetric about it, where a bake
/// without the canvas leaves the black side of the edge unfiltered — the
/// blurred copy fades to transparency there and the sharp half shows through.
#[test]
fn a_backdrop_blur_over_the_canvas_softens_an_edge_on_both_sides() {
    let mut gpu = headless("a_backdrop_blur_over_the_canvas_softens_an_edge_on_both_sides");
    let subject = "backdrop-filter: blur(6px);";
    let rect = (32.0, 32.0, 64.0, 64.0);
    let mut bare = seam_page(Ground::Canvas, 128.0, rect, "", subject);
    let bare = render_baked(&mut gpu, &mut bare, 128, 1);

    // The seam is at x = 64.0, so 63 and 64 are the symmetric pair.
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

/// The same holds under a transform on the box, on an ancestor of it, and on
/// both, and for a colour pass on either side of the blur: over the bare
/// canvas each draws what it draws over a ground the page painted.
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

        // And the comparison is of a filtered edge: just on the black side
        // of the seam, at the box's centre row, the blur has let light in.
        assert!(
            luma(&bare, 128, 61, 64) > 20,
            "{label}: the black side of the edge is filtered ({})",
            luma(&bare, 128, 61, 64),
        );
    }
}

/// A box sliding by an exported `transform` curve, composed later than its
/// commit: over the bare canvas each instant draws what it draws over a
/// ground the page painted. The bake of a moving box is taken per instant, in
/// the box's own space, and the canvas is under all of it.
#[test]
fn a_sliding_backdrop_over_the_canvas_composes_as_over_a_painted_ground() {
    let mut gpu = headless("a_sliding_backdrop_over_the_canvas_composes_as_over_a_painted_ground");
    let draw = |gpu: &mut dom::render::gpu::Headless, ground: Ground, wrapper: &str| {
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
            dom::StylesheetOrigin::Author,
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

/// A box inside an `opacity` group does not read the canvas: the group is its
/// Backdrop Root, and the group's own pixels start empty.
///
/// Where the group painted nothing a colour pass has nothing to change, so
/// the canvas shows as it does beside the box; and the bar the group did
/// paint keeps its full ink under the box, because the blurred copy fades to
/// transparency over it rather than to the canvas colour.
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
    // The bar's edge is at x = 24. Above the box (y = 20) the half-faded bar
    // is a step; under it the blur spreads the bar outward, and inside the
    // bar the ink is what it is above the box.
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

/// css-anchor-position-1 at the GPU: an anchored box outside a scroller
/// follows its anchor through a composed scroll (§3.3's default scroll
/// shift), and once the anchor is scrolled wholly out of the scroller's clip
/// `position-visibility: anchor-visible` hides it — its rounded, shadowed,
/// blurred box and all, through the zero map its anchored node composes,
/// without a bake through a map with no inverse.
#[test]
fn an_anchored_box_follows_its_scrolled_anchor_and_hides_with_it() {
    use dom::Vector2D;

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

    // (offset, where the box's middle is, or `None` when it is hidden)
    for (generation, (offset, middle)) in [(0.0_f32, Some(50)), (30.0, Some(20)), (70.0, None)]
        .into_iter()
        .enumerate()
    {
        let offset_of = move |_: &dom::ScrollSlot| Some(Vector2D::new(0.0, offset));
        let filtered: Vec<Option<dom::vello::peniko::ImageData>> = gpu
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
        let mut scene = Scene::new();
        frame.compose_into(&mut scene, &[], &filtered, &[], &offset_of, None);
        let pixels = gpu
            .render(&scene, &[], 200, 150, Color::WHITE)
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
