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
/// Mechanism, confirmed in vello 0.10.0's own source: an encoding with no
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
        .prepare_filters(&frame, &[], &|_| None, 0)
        .expect("the filter bakes render")
        .to_vec();
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &|_| None, None);
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
            .prepare_filters(&frame, &[], &|_| Some(Vector2D::new(0.0, offset)), 0)
            .expect("the filter bakes render")
            .to_vec();
        let mut scene = Scene::new();
        frame.compose_into(
            &mut scene,
            &[],
            &filtered,
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
        .prepare_filters(&frame, &[], &|_| None, 0)
        .expect("a refused group is not an error")
        .to_vec();
    assert_eq!(filtered.len(), 1);
    assert!(
        filtered[0].is_none(),
        "a group over the budget gets no texture"
    );
    let mut scene = Scene::new();
    frame.compose_into(&mut scene, &[], &filtered, &|_| None, None);
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
