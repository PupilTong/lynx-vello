//! Image decoding, done entirely by the platform.
//!
//! This crate contains no codec and links no decoding library. macOS decodes
//! through `ImageIO` and Linux through the desktop's gdk-pixbuf loaded at
//! runtime — each the same decoder the platform's own image views use, with
//! the same format coverage, and each able to downsample *during* decode so a
//! 4000px photo shown at 200px never materialises at full size. Together they
//! are this crate's [`vant::ImageDecoder`], [`NativeDecoder`]; the browser's
//! decoder is `bobcat-wasm`'s. The shape every one answers in, [`Bitmap`],
//! and the one sizing rule they share live in `vant::image`.
//!
//! # Animated images
//!
//! What an animated GIF, APNG or WebP plays as is the decoder's answer
//! ([`frames`]). `ImageIO` answers every frame, decoded by index
//! ([`decode_frame`]). gdk-pixbuf answers one frame, for now: its animation
//! interface steps a frame iterator by wall-clock time and has no decode by
//! index, so a file there is drawn as its first frame, which is what its
//! ordinary decode produces.

pub use vant::image::animation::ImageFrames;
pub use vant::image::{Bitmap, DecodeError, target_size};

#[cfg(not(target_arch = "wasm32"))]
use crate::image_header::ImageHeader;
#[cfg(not(target_arch = "wasm32"))]
use crate::mime::ImageFormat;

#[cfg(all(unix, not(target_os = "macos")))]
mod gdk_pixbuf;
#[cfg(target_os = "macos")]
mod imageio;

/// The native platform decoder as a [`vant::ImageDecoder`]: [`decode`] and
/// [`available`].
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeDecoder;

#[cfg(not(target_arch = "wasm32"))]
impl vant::ImageDecoder for NativeDecoder {
    /// The native decoders take the header the format sniff found; the
    /// format itself they read from the bytes.
    fn decode_blocking(
        &self,
        bytes: &[u8],
        _format: ImageFormat,
        header: Option<ImageHeader>,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        decode(bytes, header, max)
    }

    fn available(&self) -> Result<(), DecodeError> {
        available()
    }

    fn frames_blocking(
        &self,
        bytes: &[u8],
        _format: ImageFormat,
        _header: Option<ImageHeader>,
    ) -> Result<ImageFrames, DecodeError> {
        frames(bytes)
    }

    fn decode_frame_blocking(
        &self,
        bytes: &bytes::Bytes,
        _format: ImageFormat,
        header: Option<ImageHeader>,
        index: usize,
        max: (u32, u32),
    ) -> Result<Bitmap, DecodeError> {
        decode_frame(bytes, header, index, max)
    }

    fn forget_frames(&self, bytes: &bytes::Bytes) {
        forget_frames(bytes);
    }
}

/// Decodes `bytes` with the platform decoder, downsampling to fit inside
/// `max` (per axis, in pixels). `header` is what the container's header said
/// about the image, if it could be read — the size a decoder that scales
/// during load needs before it has seen the pixels.
///
/// Blocking; runs on a thread of the fetcher's blocking pool, holding that
/// job's decode permit, or — for a restore inside a read — on the painter's
/// thread with no permit at all.
#[cfg(not(target_arch = "wasm32"))]
pub fn decode(
    bytes: &[u8],
    header: Option<ImageHeader>,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    #[cfg(target_os = "macos")]
    {
        imageio::decode(bytes, header, max)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        gdk_pixbuf::decode(bytes, header, max)
    }
    #[cfg(not(unix))]
    {
        let _ = (bytes, header, max);
        Err(DecodeError::Unavailable(
            "no platform decoder is wired for this operating system".to_owned(),
        ))
    }
}

/// What an animated image's frames are, read without decoding a pixel:
/// every frame on macOS, one on Linux, where gdk-pixbuf has no decode by
/// frame index.
#[cfg(not(target_arch = "wasm32"))]
pub fn frames(bytes: &[u8]) -> Result<ImageFrames, DecodeError> {
    #[cfg(target_os = "macos")]
    {
        imageio::frames(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
        Ok(ImageFrames::still())
    }
}

/// Decodes frame `index` of `bytes` at most `max` large: the whole canvas as
/// shown at that frame. Frame 0 is what [`decode`] decodes; any other is
/// refused where [`frames`] answers one frame. On macOS the source stays open
/// for `bytes` between frames until [`forget_frames`].
#[cfg(not(target_arch = "wasm32"))]
pub fn decode_frame(
    bytes: &bytes::Bytes,
    header: Option<ImageHeader>,
    index: usize,
    max: (u32, u32),
) -> Result<Bitmap, DecodeError> {
    #[cfg(target_os = "macos")]
    {
        imageio::decode_frame(bytes, header, index, max)
    }
    #[cfg(not(target_os = "macos"))]
    {
        if index == 0 {
            return decode(bytes, header, max);
        }
        Err(DecodeError::Unsupported(format!(
            "this platform decodes the first frame only, not frame {index}"
        )))
    }
}

/// No more frames of `bytes` are asked for: on macOS the source kept open
/// for them is closed; elsewhere nothing was kept.
#[cfg(not(target_arch = "wasm32"))]
pub fn forget_frames(bytes: &bytes::Bytes) {
    #[cfg(target_os = "macos")]
    imageio::forget_frames(bytes);
    #[cfg(not(target_os = "macos"))]
    let _ = bytes;
}

/// Whether a platform decoder can be reached at all, so a host can report
/// the gap once at startup rather than once per image.
#[cfg(not(target_arch = "wasm32"))]
pub fn available() -> Result<(), DecodeError> {
    #[cfg(target_os = "macos")]
    {
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        gdk_pixbuf::available()
    }
    #[cfg(not(unix))]
    {
        Err(DecodeError::Unavailable(
            "no platform decoder is wired for this operating system".to_owned(),
        ))
    }
}

/// Reconciles a header's size with what the decoder produced: an EXIF
/// orientation the header probe cannot see may have transposed the image,
/// and the intrinsic size layout is told must be the oriented one.
///
/// gdk-pixbuf's helper: `ImageIO` reports the oriented size itself.
#[cfg(all(unix, not(target_os = "macos")))]
fn oriented_source_size(header: (u32, u32), decoded: (u32, u32)) -> (u32, u32) {
    let (header_landscape, decoded_landscape) = (header.0 > header.1, decoded.0 > decoded.1);
    if header.0 != header.1 && decoded.0 != decoded.1 && header_landscape != decoded_landscape {
        (header.1, header.0)
    } else {
        header
    }
}

/// Packs a decoder's rows — `channels` bytes per pixel, `stride` bytes per
/// row — into tightly packed RGBA8, filling alpha where the source has none.
///
/// gdk-pixbuf's helper: `ImageIO` draws straight into an RGBA8 context.
#[cfg(all(unix, not(target_os = "macos")))]
fn pack_rgba(
    pixels: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    channels: usize,
) -> Option<Vec<u8>> {
    if !(3..=4).contains(&channels) || stride < width.checked_mul(channels)? {
        return None;
    }
    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for row in 0..height {
        let start = row.checked_mul(stride)?;
        let source = pixels.get(start..start + width * channels)?;
        for pixel in source.chunks_exact(channels) {
            rgba.extend_from_slice(&pixel[..3]);
            rgba.push(if channels == 4 { pixel[3] } else { 255 });
        }
    }
    Some(rgba)
}

/// The gdk-pixbuf helpers, which only Linux builds.
#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn rows_pack_from_rgb_and_rgba_with_stride() {
        let rgb = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        assert_eq!(
            pack_rgba(&rgb, 2, 2, 8, 3).unwrap(),
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
        let rgba = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(pack_rgba(&rgba, 2, 1, 8, 4).unwrap(), rgba);
        assert!(
            pack_rgba(&rgba, 2, 1, 4, 4).is_none(),
            "a stride shorter than a row"
        );
        assert!(
            pack_rgba(&rgba, 2, 2, 8, 4).is_none(),
            "fewer rows than claimed"
        );
        assert!(
            pack_rgba(&rgba, 1, 1, 8, 2).is_none(),
            "an unknown channel count"
        );
    }

    #[test]
    fn a_transposing_orientation_is_reconciled_from_the_decoded_shape() {
        assert_eq!(oriented_source_size((4000, 3000), (200, 150)), (4000, 3000));
        assert_eq!(oriented_source_size((4000, 3000), (150, 200)), (3000, 4000));
        assert_eq!(oriented_source_size((100, 100), (50, 80)), (100, 100));
    }
}

/// The platform decoder itself, driven with a PNG the `png` crate encodes.
///
/// These require the platform decoder — `ImageIO` on macOS, gdk-pixbuf on
/// Linux — and fail rather than skip when it is missing, so a green run
/// always means pixels were decoded and compared.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod platform_tests {
    use super::*;

    /// A width x height image whose quadrants are red, green, blue, and
    /// half-transparent white.
    fn quadrant_png(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let pixel = match (x < width / 2, y < height / 2) {
                    (true, true) => [255, 0, 0, 255],
                    (false, true) => [0, 255, 0, 255],
                    (true, false) => [0, 0, 255, 255],
                    (false, false) => [255, 255, 255, 128],
                };
                rgba.extend_from_slice(&pixel);
            }
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&rgba).expect("png data");
        }
        bytes
    }

    fn pixel(bitmap: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * bitmap.width + x) * 4) as usize;
        bitmap.rgba[start..start + 4]
            .try_into()
            .expect("four bytes")
    }

    /// Straight-alpha colour of a pixel, whichever way the platform stores it.
    fn straight(bitmap: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let [red, green, blue, alpha] = pixel(bitmap, x, y);
        if bitmap.premultiplied && alpha != 0 && alpha != 255 {
            let unmultiply = |channel: u8| {
                u8::try_from((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha))
                    .unwrap_or(255)
            };
            [unmultiply(red), unmultiply(green), unmultiply(blue), alpha]
        } else {
            [red, green, blue, alpha]
        }
    }

    fn close(actual: [u8; 4], expected: [u8; 4]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 3)
    }

    #[test]
    fn the_platform_decoder_is_present() {
        available().expect("this platform's image decoder must be reachable");
    }

    #[test]
    fn a_png_decodes_at_full_size_with_exact_colours() {
        let bytes = quadrant_png(64, 32);
        let header = crate::image_header::probe(&bytes);
        let bitmap = decode(&bytes, header, (u32::MAX, u32::MAX)).expect("decode");
        assert_eq!((bitmap.width, bitmap.height), (64, 32));
        assert_eq!((bitmap.source_width, bitmap.source_height), (64, 32));
        assert_eq!(bitmap.byte_len(), 64 * 32 * 4);
        assert!(close(straight(&bitmap, 0, 0), [255, 0, 0, 255]));
        assert!(close(straight(&bitmap, 63, 0), [0, 255, 0, 255]));
        assert!(close(straight(&bitmap, 0, 31), [0, 0, 255, 255]));
        assert!(close(straight(&bitmap, 63, 31), [255, 255, 255, 128]));
    }

    #[test]
    fn a_bound_downsamples_during_decode_and_keeps_the_intrinsic_size() {
        let bytes = quadrant_png(64, 32);
        for header in [crate::image_header::probe(&bytes), None] {
            let bitmap = decode(&bytes, header, (16, 16)).expect("decode");
            assert_eq!(
                (bitmap.width, bitmap.height),
                (16, 8),
                "header probe: {}",
                header.is_some()
            );
            assert_eq!((bitmap.source_width, bitmap.source_height), (64, 32));
            assert!(close(straight(&bitmap, 1, 1), [255, 0, 0, 255]));
            assert!(close(straight(&bitmap, 14, 6), [255, 255, 255, 128]));
        }
    }

    /// The animated fixtures (`tests/fixtures/animated/`, written by
    /// `tests/animated_fixtures.rs`).
    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/animated")
                .join(name),
        )
        .expect("an animated fixture")
    }

    /// A still image is one frame on every platform.
    #[test]
    fn a_still_png_is_one_frame() {
        assert_eq!(
            frames(&quadrant_png(8, 8)).expect("frames"),
            ImageFrames::still()
        );
    }

    /// Every animated fixture decodes its first frame, whatever the
    /// platform answers about the rest. The WebP only where `ImageIO`
    /// decodes it: gdk-pixbuf reads WebP only with a loader module a Linux
    /// host need not have.
    #[test]
    fn every_animated_fixture_decodes_its_first_frame() {
        let names: &[&str] = if cfg!(target_os = "macos") {
            &["rgb-twice.gif", "rgb-twice.png", "rgb-thrice.webp"]
        } else {
            &["rgb-twice.gif", "rgb-twice.png"]
        };
        for &name in names {
            let bytes = fixture(name);
            let bitmap = decode(&bytes, crate::image_header::probe(&bytes), (64, 64))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!((bitmap.width, bitmap.height), (8, 8), "{name}");
            assert!(close(straight(&bitmap, 4, 4), [255, 0, 0, 255]), "{name}");
        }
    }

    /// gdk-pixbuf answers one frame for an animated file, and decodes no
    /// frame past the first: an explicit answer, not one inherited.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn linux_answers_one_frame_for_an_animated_file() {
        use vant::ImageDecoder as _;

        for name in ["rgb-twice.gif", "rgb-twice.png", "rgb-thrice.webp"] {
            let bytes = bytes::Bytes::from(fixture(name));
            assert_eq!(
                NativeDecoder
                    .frames_blocking(&bytes, crate::mime::ImageFormat::Gif, None)
                    .expect("frames"),
                ImageFrames::still(),
                "{name}"
            );
            assert!(matches!(
                NativeDecoder.decode_frame_blocking(
                    &bytes,
                    crate::mime::ImageFormat::Gif,
                    None,
                    1,
                    (64, 64)
                ),
                Err(DecodeError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn malformed_bytes_are_a_decode_error_not_a_crash() {
        assert!(matches!(
            decode(b"\x89PNG\r\n\x1a\nnot really", None, (64, 64)),
            Err(DecodeError::Malformed(_) | DecodeError::Unsupported(_))
        ));
        assert!(matches!(
            decode(b"", None, (64, 64)),
            Err(DecodeError::Malformed(_) | DecodeError::Unsupported(_))
        ));
    }
}

/// `ImageIO` against the animated fixtures: frame counts, the delays and
/// loop counts the files state, and each frame decoded by index as the whole
/// canvas at that frame.
#[cfg(all(test, target_os = "macos"))]
mod imageio_animation_tests {
    use std::time::Duration;

    use vant::ImageDecoder as _;
    use vant::image::animation::Loops;

    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/animated")
                .join(name),
        )
        .expect("an animated fixture")
    }

    const fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn at(bitmap: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * bitmap.width + x) * 4) as usize;
        bitmap.rgba[start..start + 4]
            .try_into()
            .expect("four bytes")
    }

    #[test]
    fn imageio_reads_each_format_s_delays_and_loop_count() {
        let cases = [
            (
                "rgb-twice.gif",
                vec![ms(100), ms(200), ms(300)],
                Loops::from_plays(2),
            ),
            ("fast-forever.gif", vec![ms(10), ms(50)], Loops::Infinite),
            ("rgb-once.gif", vec![ms(100); 3], Loops::ONCE),
            (
                "rgb-twice.png",
                vec![ms(5), ms(200), ms(300)],
                Loops::from_plays(2),
            ),
            (
                "rgb-thrice.webp",
                vec![ms(100), ms(200), ms(300)],
                Loops::from_plays(3),
            ),
        ];
        for (name, delays, loops) in cases {
            let frames = NativeDecoder
                .frames_blocking(&fixture(name), ImageFormat::Other, None)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(frames.loops(), loops, "{name}");
            // `ImageIO` reads a delay as a double of seconds.
            let stated: Vec<Duration> = frames
                .stated_delays()
                .iter()
                .map(|delay| ms(u64::try_from(delay.as_micros().div_ceil(1000)).expect("short")))
                .collect();
            assert_eq!(stated, delays, "{name}: the delays as stated, unclamped");
        }
    }

    #[test]
    fn imageio_decodes_each_frame_as_the_whole_canvas_at_it() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        const BLUE: [u8; 4] = [0, 0, 255, 255];
        for (name, gif) in [
            ("rgb-twice.gif", true),
            ("rgb-twice.png", false),
            ("rgb-thrice.webp", false),
        ] {
            let bytes = bytes::Bytes::from(fixture(name));
            let colours: Vec<([u8; 4], [u8; 4])> = (0..3)
                .map(|index| {
                    let bitmap = NativeDecoder
                        .decode_frame_blocking(&bytes, ImageFormat::Other, None, index, (64, 64))
                        .unwrap_or_else(|error| panic!("{name} frame {index}: {error}"));
                    assert_eq!((bitmap.width, bitmap.height), (8, 8));
                    assert_eq!((bitmap.source_width, bitmap.source_height), (8, 8));
                    (at(&bitmap, 1, 4), at(&bitmap, 6, 4))
                })
                .collect();
            let last = if gif {
                // The GIF's last frame covers the left half only: the right
                // half is the green frame before it, composited under it.
                (BLUE, GREEN)
            } else {
                (BLUE, BLUE)
            };
            assert_eq!(colours, [(RED, RED), (GREEN, GREEN), last], "{name}");
            assert!(
                NativeDecoder
                    .decode_frame_blocking(&bytes, ImageFormat::Other, None, 3, (64, 64))
                    .is_err(),
                "{name}: there is no frame 3"
            );
            NativeDecoder.forget_frames(&bytes);
        }
    }

    /// A bound downsamples every frame, as it does the first.
    #[test]
    fn imageio_downsamples_a_frame_to_the_bound() {
        let bytes = bytes::Bytes::from(fixture("rgb-thrice.webp"));
        let bitmap = decode_frame(&bytes, None, 2, (4, 4)).expect("frame 2");
        forget_frames(&bytes);
        assert_eq!((bitmap.width, bitmap.height), (4, 4));
        assert_eq!((bitmap.source_width, bitmap.source_height), (8, 8));
        assert_eq!(at(&bitmap, 2, 2), [0, 0, 255, 255]);
    }
}
