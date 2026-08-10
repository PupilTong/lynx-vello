//! The decode contract, exercised through an injected decoder double.
//!
//! The engine designs the contract and ships no codec, so these tests inject
//! `support::PngDouble` exactly the way an embedder injects a real decoder.
//! What is asserted here is the *contract*: identification, the capability
//! gate, the framing gate, the caps, and target semantics. How a real codec
//! behaves against real JPEG/WebP/EXIF bytes is the implementation's business
//! and is tested where the implementations live — `bobcat-cli`'s
//! `image_decoders` module.

mod support;

use std::sync::Arc;

use bobcat_core::image::{
    DecodeRequest, Decoder, ImageError, ImageFormat, PixelSize, decode_bytes, probe_bytes, sniff,
};
use support::{checker_png, checker_rgba, decoder};

#[test]
fn decodes_a_png_round_trip_byte_for_byte() {
    // PNG is lossless and the double neither converts nor premultiplies, so
    // this is an exact assertion.
    let side = 4;
    let source = checker_rgba(side);
    let decoder: Arc<dyn Decoder> = decoder();
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
fn a_truncated_container_is_rejected_before_any_decoder_runs() {
    // Platform decoders disagree about truncation (ImageIO decodes a cut PNG
    // to a transparent full-size image and calls the source complete), so the
    // framing gate fires in the contract layer, before whichever decoder is
    // injected — which is why it must fire for the double too.
    let decoder = decoder();
    let complete = checker_png(4);
    let bytes = &complete[..complete.len() - 8];
    assert_eq!(sniff(bytes), Some(ImageFormat::Png), "still identifiable");

    let error = decode_bytes(decoder.as_ref(), bytes, &DecodeRequest::default())
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
        probe_bytes(decoder.as_ref(), bytes),
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

#[test]
fn an_identified_but_unclaimed_format_is_refused_as_unsupported() {
    // The double claims PNG alone, so every other identified container is the
    // `Unsupported` case — distinct from `UnknownFormat`, and reported before
    // the decoder is asked anything.
    let decoder = decoder();
    for (bytes, format) in [
        (b"GIF89a\x10\x00\x10\x00".to_vec(), ImageFormat::Gif),
        (vec![0xFF, 0xD8, 0xFF, 0xD9], ImageFormat::Jpeg),
    ] {
        assert_eq!(sniff(&bytes), Some(format));
        let error = decode_bytes(decoder.as_ref(), &bytes, &DecodeRequest::default())
            .expect_err("unclaimed format");
        assert!(
            matches!(error, ImageError::Unsupported { format: reported } if reported == format),
            "expected Unsupported({format}), got {error:?}"
        );
        assert!(matches!(
            probe_bytes(decoder.as_ref(), &bytes),
            Err(ImageError::Unsupported { .. })
        ));
    }
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
