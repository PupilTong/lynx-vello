//! GPU smoke tests for [`dom::render::gpu::Headless`]: build a tiny scene by
//! hand, render it headless, and check pixels from the readback. A usable GPU
//! adapter is mandatory, including in CI.

use dom::render::gpu::Headless;
use dom::vello;
use dom::vello::kurbo::{Affine, Rect};
use dom::vello::peniko::{Color, Fill};

/// Reads the RGBA pixel at (`x`, `y`) from a tightly-packed row-major
/// readback of the given width.
fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let offset = (y * width + x) * 4;
    pixels[offset..offset + 4].try_into().unwrap()
}

#[track_caller]
fn headless(test: &str) -> Headless {
    Headless::new().unwrap_or_else(|error| panic!("{test}: GPU initialization failed: {error}"))
}

#[test]
fn red_square_over_white_base() {
    let mut headless = headless("red_square_over_white_base");

    // A 40×40 red square at (12, 12) on a 64×64 white canvas.
    let mut scene = vello::Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(255, 0, 0),
        None,
        &Rect::new(12.0, 12.0, 52.0, 52.0),
    );
    let pixels = headless
        .render(&scene, 64, 64, Color::WHITE)
        .expect("headless render should succeed");
    assert_eq!(pixels.len(), 64 * 64 * 4);

    let center = pixel(&pixels, 64, 32, 32);
    assert!(
        center[0] > 200 && center[1] < 60 && center[2] < 60,
        "center pixel should be red-ish, got {center:?}"
    );
    assert_eq!(center[3], 255, "center pixel should be opaque");

    let corner = pixel(&pixels, 64, 2, 2);
    assert!(
        corner[0] > 200 && corner[1] > 200 && corner[2] > 200,
        "corner pixel should be white-ish, got {corner:?}"
    );
    assert_eq!(corner[3], 255, "corner pixel should be opaque");
}

#[test]
fn empty_scene_paints_exact_base_color_with_row_padding() {
    let mut headless = headless("empty_scene_paints_exact_base_color_with_row_padding");

    // 33×17 makes each tight row 132 bytes, forcing 256-byte copy padding:
    // exact pixels prove the readback strips row padding correctly.
    let scene = vello::Scene::new();
    let blue = Color::from_rgb8(0, 0, 255);
    let pixels = headless
        .render(&scene, 33, 17, blue)
        .expect("headless render should succeed");
    assert_eq!(pixels.len(), 33 * 17 * 4);

    assert_eq!(pixel(&pixels, 33, 0, 0), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 33, 32, 16), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 33, 16, 8), [0, 0, 255, 255]);
}
