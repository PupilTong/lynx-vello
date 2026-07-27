//! GPU smoke tests for [`pulsar::gpu::Headless`]: build a tiny scene by
//! hand, render it headless, and check pixels from the readback. Machines
//! without a usable GPU adapter (headless CI) skip rather than fail.

use pulsar::gpu::{GpuError, Headless};
use pulsar::vello;
use pulsar::vello::kurbo::{Affine, Rect};
use pulsar::vello::peniko::{Color, Fill};

/// Creates the headless renderer, or skips the test (returns `None`) when
/// the machine has no usable GPU adapter.
fn headless_or_skip(test: &str) -> Option<Headless> {
    match Headless::new() {
        Ok(headless) => Some(headless),
        Err(GpuError::NoAdapter) => {
            eprintln!("skipping {test}: no usable GPU adapter on this machine");
            None
        }
        Err(error) => panic!("creating the headless renderer failed: {error}"),
    }
}

/// Reads the RGBA pixel at (`x`, `y`) from a tightly-packed row-major
/// readback of the given width.
fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let offset = (y * width + x) * 4;
    pixels[offset..offset + 4].try_into().unwrap()
}

#[test]
fn red_square_over_white_base() {
    let Some(mut headless) = headless_or_skip("red_square_over_white_base") else {
        return;
    };

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
    let Some(mut headless) =
        headless_or_skip("empty_scene_paints_exact_base_color_with_row_padding")
    else {
        return;
    };

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
