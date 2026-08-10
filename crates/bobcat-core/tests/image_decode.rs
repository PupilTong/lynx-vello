//! Decoding through the platform's injected decoder, and refusing everything
//! else.
//!
//! These are contract tests: they run against whatever
//! `image_decoders::platform_decoder()` ships on the compiling OS — `ImageIO`
//! on macOS, the pure-Rust reference on Linux — because that seam is exactly
//! what an embedder injects through, and a contract test that only ever met one
//! implementation would prove nothing about the contract.

mod support;

use std::sync::Arc;

use bobcat_core::image::{
    DecodeRequest, Decoder, ImageError, ImageFormat, PixelSize, decode_bytes, probe_bytes, sniff,
};
use support::{checker_png, checker_rgba, fixture};

fn decoder() -> Arc<dyn Decoder> {
    image_decoders::platform_decoder().expect("this platform ships a decoder")
}

#[test]
fn decodes_a_png_round_trip_byte_for_byte() {
    // PNG is lossless and the fixture's translucent quadrant is *fully*
    // transparent, so straight and premultiplied encodings share the same
    // bytes and this stays an exact assertion on every platform.
    let side = 4;
    let source = checker_rgba(side);
    let decoder = decoder();
    let response = decode_bytes(
        decoder.as_ref(),
        &checker_png(side),
        &DecodeRequest::default(),
    )
    .expect("decode generated PNG");

    assert_eq!(response.image.width(), side);
    assert_eq!(response.image.height(), side);
    assert_eq!(response.image.pixels(), source.as_slice());
    assert_eq!(response.header.format, ImageFormat::Png);
    assert!(response.header.has_alpha);
    assert!(!response.header.animated);
    // Provenance must be reported truthfully for whichever decoder ran.
    assert_eq!(response.backend, decoder.name());
    assert_eq!(
        Some(response.acceleration),
        decoder.capabilities().tier(ImageFormat::Png)
    );
}

#[test]
fn probing_costs_no_pixels_but_reports_the_same_size() {
    let bytes = checker_png(8);
    let decoder = decoder();
    let header = probe_bytes(decoder.as_ref(), &bytes).expect("probe");
    let full = decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default()).expect("decode");

    assert_eq!(
        header.natural_size,
        PixelSize {
            width: 8,
            height: 8
        }
    );
    assert_eq!(header.natural_size.width, full.image.width());
    assert_eq!(header.natural_size.height, full.image.height());
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
    // Every decoder implementation owes this, which is why the fixture runs
    // against whichever one the platform ships.
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
fn a_truncated_container_is_rejected_before_any_decoder_runs() {
    // ImageIO decodes this to a full-size transparent image and calls the
    // source complete; `is_complete` is what stops that divergence, and it
    // must fire identically no matter which decoder is injected.
    let decoder = decoder();
    let bytes = fixture("truncated.png");
    assert_eq!(sniff(&bytes), Some(ImageFormat::Png), "still identifiable");

    let error = decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default())
        .expect_err("a truncated PNG must not decode");
    assert!(
        matches!(
            error,
            ImageError::Truncated {
                format: ImageFormat::Png,
                ..
            }
        ),
        "expected Truncated, got {error:?}"
    );
    // The header probe takes the same gate.
    assert!(matches!(
        probe_bytes(decoder.as_ref(), &bytes),
        Err(ImageError::Truncated { .. })
    ));
}

#[test]
fn unidentified_containers_are_refused_as_unknown() {
    let decoder = decoder();
    for bytes in [
        b"BM.parade.of.bytes".to_vec(),
        b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
        Vec::new(),
    ] {
        assert!(matches!(
            decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default()),
            Err(ImageError::UnknownFormat)
        ));
    }
}

/// The reference decoder claims PNG/JPEG/WebP only, so an identified GIF is
/// the `Unsupported` case — distinct from `UnknownFormat` — on Linux. On the
/// platforms whose decoder claims GIF the same bytes take the decode path
/// instead, which is the capability gate working, not a divergence.
#[cfg(target_os = "linux")]
#[test]
fn an_identified_but_unclaimed_format_is_refused_as_unsupported() {
    let decoder = decoder();
    let gif = b"GIF89a\x10\x00\x10\x00".to_vec();
    assert_eq!(sniff(&gif), Some(ImageFormat::Gif));
    assert!(matches!(
        decode_bytes(decoder.as_ref(), &gif, &DecodeRequest::default()),
        Err(ImageError::Unsupported {
            format: ImageFormat::Gif
        })
    ));
    assert!(matches!(
        probe_bytes(decoder.as_ref(), &gif),
        Err(ImageError::Unsupported { .. })
    ));
}

#[test]
fn the_pixel_caps_reject_before_allocating() {
    let decoder = decoder();
    let bytes = checker_png(64);
    let request = DecodeRequest::default().with_max_dimension(32);
    let error = decode_bytes(decoder.as_ref(), &bytes, &request).expect_err("past max_dimension");
    assert!(matches!(error, ImageError::TooLarge { .. }));

    let request = DecodeRequest::default().with_max_pixels(100);
    assert!(matches!(
        decode_bytes(decoder.as_ref(), &bytes, &request),
        Err(ImageError::TooLarge { .. })
    ));
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
    // The header keeps reporting the SOURCE size: layout resolves `object-fit`
    // against the natural size, not the decode size.
    assert_eq!(
        small.header.natural_size,
        PixelSize {
            width: 16,
            height: 16
        }
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

    let rgb = encode_rgb(2, 2, &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    let full = decode_bytes(decoder.as_ref(), &rgb, &DecodeRequest::default()).expect("rgb decode");
    assert_eq!(full.image.pixels().len(), 2 * 2 * 4);
    assert_eq!(&full.image.pixels()[0..4], &[255, 0, 0, 255]);
}

fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("header");
    writer.write_image_data(rgb).expect("rgb data");
    drop(writer);
    bytes
}

#[test]
fn the_platform_decoder_handles_every_committed_fixture() {
    // Probe and decode must agree with each other on every fixture, whichever
    // decoder the platform ships — the probe is what layout consumed, so a
    // decode that disagrees with it shows up as a mis-sized box.
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
            "{name}: full pixels must match the probed natural size"
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

/// The strict frame-0-not-fallback pixel assertion is pinned to the reference
/// decoder: it is the implementation whose `next_frame` would otherwise hand
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
