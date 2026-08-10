//! The reference decoders against real encoded bytes.
//!
//! These run against whatever `image_decoders::platform_decoder()` ships on
//! the compiling OS — `ImageIO` on macOS, the pure-Rust reference on Linux.
//! The committed fixtures are frozen third-party ground truth (this workspace
//! encodes no JPEG or WebP); see `tests/fixtures/README.md`. Contract-level
//! behaviour (identification, gates, caps) is tested in `bobcat-core` against
//! a decoder double — what belongs here is how a *real* codec behaves.

use std::path::PathBuf;

use bobcat_cli::image_decoders::platform_decoder;
use bobcat_core::image::{
    DecodeRequest, Decoder, ImageFormat, PixelSize, decode_bytes, probe_bytes,
};

fn decoder() -> std::sync::Arc<dyn Decoder> {
    platform_decoder().expect("this platform ships a decoder")
}

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// A 4x4-quadrant RGBA8 PNG (red / green / blue / transparent), encoded
/// in-process so the common case needs no committed fixture.
fn checker_png(side: u32) -> Vec<u8> {
    let half = side / 2;
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let pixel = match (x < half, y < half) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [0, 0, 0, 0],
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, side, side);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&rgba).expect("png data");
    }
    bytes
}

#[test]
fn decodes_the_jpeg_fixture_with_recognisable_quadrants() {
    let decoder = decoder();
    let response = decode_bytes(
        decoder.as_ref(),
        &fixture("checker-16.jpg"),
        &DecodeRequest::default(),
    )
    .expect("decode JPEG fixture");
    assert_eq!(response.header.format, ImageFormat::Jpeg);
    assert_eq!((response.image.width(), response.image.height()), (16, 16));
    assert!(!response.header.has_alpha, "baseline JPEG carries no alpha");

    // JPEG is lossy; assert the dominant channel per quadrant rather than exact
    // bytes. Sampling the quadrant centre avoids the ringing at the edges.
    let pixel = |x: u32, y: u32| {
        let at = ((y * 16 + x) * 4) as usize;
        let p = &response.image.pixels()[at..at + 4];
        (p[0], p[1], p[2], p[3])
    };
    let (r, g, b, a) = pixel(4, 4);
    assert!(
        r > 180 && g < 80 && b < 80,
        "top-left should be red, got {r},{g},{b}"
    );
    assert_eq!(a, 255, "the synthesised alpha channel is opaque");
    let (r, g, b, _) = pixel(12, 4);
    assert!(
        g > 180 && r < 80,
        "top-right should be green, got {r},{g},{b}"
    );
    let (r, g, b, _) = pixel(4, 12);
    assert!(
        b > 180 && r < 80,
        "bottom-left should be blue, got {r},{g},{b}"
    );
}

#[test]
fn decodes_the_lossless_webp_fixture_including_its_transparent_quadrant() {
    let decoder = decoder();
    let response = decode_bytes(
        decoder.as_ref(),
        &fixture("checker-16.webp"),
        &DecodeRequest::default(),
    )
    .expect("decode WebP fixture");

    assert_eq!(response.header.format, ImageFormat::WebP);
    assert_eq!((response.image.width(), response.image.height()), (16, 16));
    assert!(response.header.has_alpha);
    assert!(!response.header.animated);

    let alpha_at = |x: u32, y: u32| response.image.pixels()[((y * 16 + x) * 4 + 3) as usize];
    assert_eq!(alpha_at(4, 4), 255, "opaque quadrant");
    assert_eq!(alpha_at(12, 12), 0, "transparent quadrant");
}

#[test]
fn exif_orientation_is_applied_to_both_the_header_and_the_pixels() {
    // Stored 16x8 tagged orientation 6 (rotate 90 CW) must present as 8x16.
    // css-images-3 makes `image-orientation: from-image` the initial value and
    // the fork has no such property, so un-oriented output is not authorable.
    let decoder = decoder();
    let bytes = fixture("exif-rot90.jpg");
    let header = probe_bytes(decoder.as_ref(), &bytes).expect("probe");
    assert_eq!(
        header.natural_size,
        PixelSize {
            width: 8,
            height: 16
        },
        "the probe must report the ORIENTED size, since layout consumes it"
    );

    let full = decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default()).expect("decode");
    assert_eq!(
        (full.image.width(), full.image.height()),
        (8, 16),
        "the pixels must agree with the probe"
    );
}

#[test]
fn a_png_with_a_fully_transparent_quadrant_round_trips() {
    // The fully transparent quadrant is [0,0,0,0] under both straight and
    // premultiplied encodings, so the assertion is exact on every platform.
    let decoder = decoder();
    let response = decode_bytes(decoder.as_ref(), &checker_png(4), &DecodeRequest::default())
        .expect("decode generated PNG");
    assert_eq!((response.image.width(), response.image.height()), (4, 4));
    assert_eq!(&response.image.pixels()[0..4], &[255, 0, 0, 255]);
    let last = &response.image.pixels()[4 * 4 * 4 - 4..];
    assert_eq!(last, &[0, 0, 0, 0], "transparent quadrant");
}

#[test]
fn grayscale_and_rgb_pngs_normalise_to_rgba8() {
    // Whatever layout the container stored, tightly packed RGBA8 must come out
    // or the atlas upload is garbage. Grayscale is asserted with a small
    // tolerance: a colour-managed decoder (ImageIO) may map device gray
    // through a gray→sRGB conversion the pure-Rust decoder does not perform.
    let decoder = decoder();
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer
            .write_image_data(&[0, 64, 128, 255])
            .expect("gray data");
    }
    let gray =
        decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default()).expect("gray decode");
    assert_eq!(gray.image.pixels().len(), 2 * 2 * 4);
    for (index, expected) in [0u8, 64, 128, 255].into_iter().enumerate() {
        let pixel = &gray.image.pixels()[index * 4..index * 4 + 4];
        assert!(
            pixel[0] == pixel[1] && pixel[1] == pixel[2],
            "gray must stay gray, got {pixel:?}"
        );
        assert!(
            pixel[0].abs_diff(expected) <= 3,
            "gray value drifted: expected ~{expected}, got {}",
            pixel[0]
        );
        assert_eq!(pixel[3], 255);
    }

    let mut rgb = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut rgb, 2, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer
            .write_image_data(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255])
            .expect("rgb data");
    }
    let full = decode_bytes(decoder.as_ref(), &rgb, &DecodeRequest::default()).expect("rgb decode");
    assert_eq!(full.image.pixels().len(), 2 * 2 * 4);
    assert_eq!(&full.image.pixels()[0..4], &[255, 0, 0, 255]);
}

#[test]
fn a_decode_target_downsamples_but_never_upsamples() {
    let decoder = decoder();
    let bytes = checker_png(16);

    let request = DecodeRequest::default().with_target(Some(PixelSize {
        width: 8,
        height: 8,
    }));
    let small = decode_bytes(decoder.as_ref(), &bytes, &request).expect("downsample");
    assert_eq!((small.image.width(), small.image.height()), (8, 8));
    assert_eq!(
        small.header.natural_size,
        PixelSize {
            width: 16,
            height: 16
        },
        "the header keeps reporting the source size"
    );

    let request = DecodeRequest::default().with_target(Some(PixelSize {
        width: 64,
        height: 64,
    }));
    let clamped = decode_bytes(decoder.as_ref(), &bytes, &request).expect("upsample request");
    assert_eq!(
        (clamped.image.width(), clamped.image.height()),
        (16, 16),
        "a larger target clamps to the source instead of upscaling"
    );
}

#[test]
fn the_platform_decoder_handles_every_committed_fixture() {
    // Probe and decode must agree with each other on every fixture — the
    // probe is what layout consumed, so a decode that disagrees with it shows
    // up as a mis-sized box.
    let decoder = decoder();
    for name in ["checker-16.jpg", "checker-16.webp", "exif-rot90.jpg"] {
        let bytes = fixture(name);
        let header = probe_bytes(decoder.as_ref(), &bytes)
            .unwrap_or_else(|error| panic!("probe of {name}: {error}"));
        let full = decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default())
            .unwrap_or_else(|error| panic!("decode of {name}: {error}"));
        assert_eq!(
            (full.image.width(), full.image.height()),
            (header.natural_size.width, header.natural_size.height),
            "{name}: decoded pixels must match the probed natural size"
        );
        assert_eq!(full.header, header, "{name}");
    }
}

#[test]
fn an_apng_reports_animated_and_decodes_a_full_canvas_frame() {
    // This file's default image carries no preceding `fcTL`, so per the APNG
    // spec it is a fallback for non-APNG decoders and is NOT part of the
    // animation; frame 0 of the animation is opaque red.
    let decoder = decoder();
    let response = decode_bytes(
        decoder.as_ref(),
        &fixture("apng-fallback.png"),
        &DecodeRequest::default(),
    )
    .expect("decode APNG");

    assert!(response.header.animated, "acTL present means animated");
    assert_eq!(
        (response.image.width(), response.image.height()),
        (4, 4),
        "the decode covers the full canvas"
    );
}

/// The strict frame-0-not-fallback pixel assertion is pinned to the pure-Rust
/// reference: it is the implementation whose `next_frame` would otherwise hand
/// the transparent fallback back, and the recorded frame-0 policy was written
/// against it. Platform decoders own their own frame-0 selection.
#[cfg(target_os = "linux")]
#[test]
fn the_reference_decoder_returns_animation_frame_zero_not_the_fallback() {
    let decoder = decoder();
    let response = decode_bytes(
        decoder.as_ref(),
        &fixture("apng-fallback.png"),
        &DecodeRequest::default(),
    )
    .expect("decode APNG");

    let pixels = response.image.pixels();
    assert_eq!(
        &pixels[0..4],
        &[255, 0, 0, 255],
        "expected animation frame 0 (opaque red), got the transparent fallback"
    );
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| *px == [255, 0, 0, 255]),
        "every pixel of frame 0 covers the canvas"
    );
}
