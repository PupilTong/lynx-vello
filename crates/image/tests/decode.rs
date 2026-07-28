//! Decoding the three supported containers, and refusing everything else.

mod support;

use image::{
    Acceleration, BackendRegistry, DecodeRequest, ImageError, ImageFormat, PixelSize, decode_bytes,
    probe_bytes, sniff,
};
use support::{checker_png, checker_rgba, fixture};

fn software() -> BackendRegistry {
    BackendRegistry::software_only()
}

#[test]
fn decodes_a_png_round_trip_byte_for_byte() {
    // PNG is lossless, so this is an exact assertion — the one format where a
    // tolerance would be hiding something.
    let side = 4;
    let source = checker_rgba(side);
    let response = decode_bytes(&software(), &checker_png(side), &DecodeRequest::default())
        .expect("decode generated PNG");

    assert_eq!(response.image.width(), side);
    assert_eq!(response.image.height(), side);
    assert_eq!(response.image.pixels(), source.as_slice());
    assert_eq!(response.header.format, ImageFormat::Png);
    assert!(response.header.has_alpha);
    assert!(!response.header.animated);
    assert_eq!(response.acceleration, Acceleration::Software);
    assert_eq!(response.backend, "software");
}

#[test]
fn probing_costs_no_pixels_but_reports_the_same_size() {
    let bytes = checker_png(8);
    let header = probe_bytes(&software(), &bytes).expect("probe");
    let decoded = decode_bytes(&software(), &bytes, &DecodeRequest::default()).expect("decode");

    assert_eq!(
        header.natural_size,
        PixelSize {
            width: 8,
            height: 8
        }
    );
    assert_eq!(header.natural_size.width, decoded.image.width());
    assert_eq!(header.natural_size.height, decoded.image.height());
}

#[test]
fn decodes_the_jpeg_fixture_with_recognisable_quadrants() {
    let response = decode_bytes(
        &software(),
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
    let response = decode_bytes(
        &software(),
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
    let bytes = fixture("exif-rot90.jpg");
    let header = probe_bytes(&software(), &bytes).expect("probe");
    assert_eq!(
        header.natural_size,
        PixelSize {
            width: 8,
            height: 16
        },
        "the probe must report the ORIENTED size, since layout consumes it"
    );

    let decoded = decode_bytes(&software(), &bytes, &DecodeRequest::default()).expect("decode");
    assert_eq!(
        (decoded.image.width(), decoded.image.height()),
        (8, 16),
        "the pixels must agree with the probe"
    );
}

#[test]
fn a_truncated_container_is_rejected_before_any_backend_runs() {
    // ImageIO decodes this to a full-size transparent image and calls the
    // source complete; `is_complete` is what stops that divergence.
    let bytes = fixture("truncated.png");
    assert_eq!(sniff(&bytes), Some(ImageFormat::Png), "still identifiable");

    let error = decode_bytes(&software(), &bytes, &DecodeRequest::default())
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
        probe_bytes(&software(), &bytes),
        Err(ImageError::Truncated { .. })
    ));
}

#[test]
fn unsupported_containers_are_refused_by_name() {
    for bytes in [
        b"GIF89a\x10\x00\x10\x00".to_vec(),
        b"\0\0\0\x20ftypavif".to_vec(),
        b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
        Vec::new(),
    ] {
        assert!(matches!(
            decode_bytes(&software(), &bytes, &DecodeRequest::default()),
            Err(ImageError::UnknownFormat)
        ));
    }
}

#[test]
fn the_pixel_caps_reject_before_allocating() {
    let bytes = checker_png(64);
    let request = DecodeRequest::default().with_max_dimension(32);
    let error = decode_bytes(&software(), &bytes, &request).expect_err("past max_dimension");
    assert!(matches!(error, ImageError::TooLarge { .. }));

    let request = DecodeRequest::default().with_max_pixels(100);
    assert!(matches!(
        decode_bytes(&software(), &bytes, &request),
        Err(ImageError::TooLarge { .. })
    ));
}

#[test]
fn a_decode_target_downsamples_but_never_upsamples() {
    let bytes = checker_png(16);

    let request = DecodeRequest::default().with_target(Some(PixelSize {
        width: 8,
        height: 8,
    }));
    let small = decode_bytes(&software(), &bytes, &request).expect("downsample");
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
    let clamped = decode_bytes(&software(), &bytes, &request).expect("upsample request");
    assert_eq!(
        (clamped.image.width(), clamped.image.height()),
        (16, 16),
        "a larger target clamps to the source instead of upscaling"
    );
}

#[test]
fn grayscale_and_rgb_pngs_normalise_to_rgba8() {
    // `png`'s EXPAND leaves four possible 8-bit layouts; all four must come out
    // as tightly packed RGBA8 or the atlas upload is garbage.
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
    let gray = decode_bytes(&software(), &bytes, &DecodeRequest::default()).expect("gray decode");
    assert_eq!(gray.image.pixels().len(), 2 * 2 * 4);
    assert_eq!(&gray.image.pixels()[0..4], &[0, 0, 0, 255]);
    assert_eq!(&gray.image.pixels()[4..8], &[64, 64, 64, 255]);

    let rgb = encode_rgb(2, 2, &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    let decoded = decode_bytes(&software(), &rgb, &DecodeRequest::default()).expect("rgb decode");
    assert_eq!(decoded.image.pixels().len(), 2 * 2 * 4);
    assert_eq!(&decoded.image.pixels()[0..4], &[255, 0, 0, 255]);
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
fn every_registered_backend_agrees_on_every_fixture() {
    // The cross-backend contract: the platform backends differ in alpha
    // encoding and in what they do with orientation and truncation, so
    // dimensions and alpha *presence* must match even where bytes do not.
    let detected = BackendRegistry::detect();
    let software = software();

    for name in ["checker-16.jpg", "checker-16.webp", "exif-rot90.jpg"] {
        let bytes = fixture(name);
        let reference = probe_bytes(&software, &bytes).unwrap_or_else(|error| {
            panic!("software probe of {name}: {error}");
        });
        let candidate = probe_bytes(&detected, &bytes).unwrap_or_else(|error| {
            panic!("detected-backend probe of {name}: {error}");
        });
        assert_eq!(
            reference.natural_size, candidate.natural_size,
            "{name}: backends disagree on the natural size, which layout consumes"
        );
        assert_eq!(reference.format, candidate.format, "{name}");

        let decoded = decode_bytes(&detected, &bytes, &DecodeRequest::default())
            .unwrap_or_else(|error| panic!("detected-backend decode of {name}: {error}"));
        assert_eq!(
            (decoded.image.width(), decoded.image.height()),
            (reference.natural_size.width, reference.natural_size.height),
            "{name}: decoded pixels must match the probed natural size"
        );
    }

    // Truncation is rejected identically no matter who would have decoded it.
    assert!(matches!(
        decode_bytes(
            &detected,
            &fixture("truncated.png"),
            &DecodeRequest::default()
        ),
        Err(ImageError::Truncated { .. })
    ));
}

#[test]
fn an_apng_decodes_animation_frame_zero_not_its_fallback_image() {
    // This file's default image carries no preceding `fcTL`, so per the APNG
    // spec it is a fallback for non-APNG decoders and is NOT part of the
    // animation. `Reader::next_frame` hands that fallback back first, so a
    // decoder that simply takes it returns the transparent placeholder instead
    // of the red frame 0 — silently violating the documented frame-0 policy.
    let response = decode_bytes(
        &software(),
        &fixture("apng-fallback.png"),
        &DecodeRequest::default(),
    )
    .expect("decode APNG");

    assert!(response.header.animated, "acTL present means animated");
    assert_eq!(
        (response.image.width(), response.image.height()),
        (4, 4),
        "the frame is composited onto the full canvas"
    );

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
