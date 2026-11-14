//! The animated fixtures in `tests/fixtures/animated/`, and how each was
//! made: this file writes every one of them byte for byte, with no encoder
//! library beyond the `png` crate the workspace already has, and checks
//! that the checked-in files are exactly what it writes.
//!
//! Run with `BOBCAT_WRITE_FIXTURES=1` to write them again after changing a
//! writer here; the run then fails, so a rewritten fixture is never accepted
//! unseen.
//!
//! Every frame is 8 × 8 and one solid colour, so a decoder's answer is
//! checked by sampling one pixel; `rgb-twice.gif`'s last frame covers only
//! the left half of the canvas, so it also checks that a frame is decoded as
//! the whole canvas at that frame, earlier frames composited under it.

use std::path::{Path, PathBuf};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/animated")
}

/// The GIF: 8 × 8, a global table of red, green, blue and white, the
/// NETSCAPE2.0 extension with `repeat` when given, and each frame as
/// `(delay in hundredths of a second, left, width, colour index)` covering
/// the full height from `left`, disposal "do not dispose".
///
/// The pixels are LZW-coded the simplest legal way: a clear code before
/// every two literals, so the code table never grows past the three-bit
/// codes a four-colour table starts with.
fn gif(repeat: Option<u16>, frames: &[(u16, u16, u16, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&8_u16.to_le_bytes());
    out.extend_from_slice(&8_u16.to_le_bytes());
    // A global table of 2^(1+1) entries, colour resolution 2 bits.
    out.extend_from_slice(&[0b1001_0001, 0, 0]);
    for colour in [RED, GREEN, BLUE, WHITE] {
        out.extend_from_slice(&colour[..3]);
    }
    if let Some(repeat) = repeat {
        out.extend_from_slice(&[0x21, 0xFF, 0x0B]);
        out.extend_from_slice(b"NETSCAPE2.0");
        out.extend_from_slice(&[0x03, 0x01]);
        out.extend_from_slice(&repeat.to_le_bytes());
        out.push(0);
    }
    for &(delay, left, width, index) in frames {
        // Graphic control: disposal 1 (do not dispose), no transparency.
        out.extend_from_slice(&[0x21, 0xF9, 0x04, 0b0000_0100]);
        out.extend_from_slice(&delay.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.push(0x2C);
        for value in [left, 0, width, 8] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.push(0);
        out.push(2);
        let codes = lzw(&vec![index; usize::from(width) * 8]);
        for block in codes.chunks(255) {
            out.push(u8::try_from(block.len()).expect("a sub-block"));
            out.extend_from_slice(block);
        }
        out.push(0);
    }
    out.push(0x3B);
    out
}

/// `indices` LZW-coded at a minimum code size of 2: clear is 4, end is 5,
/// every code three bits wide, packed least significant bit first.
fn lzw(indices: &[u8]) -> Vec<u8> {
    let mut codes = Vec::new();
    for pair in indices.chunks(2) {
        codes.push(4);
        codes.extend(pair.iter().copied());
    }
    codes.push(5);
    let mut bytes = Vec::new();
    let (mut accumulator, mut bits) = (0_u32, 0);
    for code in codes {
        accumulator |= u32::from(code) << bits;
        bits += 3;
        while bits >= 8 {
            bytes.push((accumulator & 0xFF) as u8);
            accumulator >>= 8;
            bits -= 8;
        }
    }
    if bits > 0 {
        bytes.push((accumulator & 0xFF) as u8);
    }
    bytes
}

/// The APNG, with the `png` crate: 8 × 8 RGBA, each frame the whole canvas
/// in one colour, shown for `(numerator, denominator)` seconds, played
/// `plays` times.
fn apng(plays: u32, frames: &[([u8; 4], u16, u16)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 8, 8);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .set_animated(u32::try_from(frames.len()).expect("a few frames"), plays)
            .expect("animated");
        let mut writer = encoder.write_header().expect("png header");
        for &(colour, numerator, denominator) in frames {
            writer
                .set_frame_delay(numerator, denominator)
                .expect("a frame delay");
            writer
                .write_image_data(&colour.repeat(64))
                .expect("a frame");
        }
        writer.finish().expect("the last frame");
    }
    bytes
}

/// Bits packed least significant first, as VP8L reads them.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        for bit in 0..count {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if value >> bit & 1 == 1 {
                *self.bytes.last_mut().expect("a byte") |= 1 << (self.used % 8);
            }
            self.used += 1;
        }
    }
}

/// One VP8L (lossless WebP) bitstream of an 8 × 8 image in `colour`.
///
/// No transform, no colour cache, no meta prefix codes, and five "simple"
/// prefix codes of one symbol each (green, red, blue, alpha, distance): a
/// one-symbol code is zero bits long, so the pixels take no bits at all.
fn vp8l(colour: [u8; 4]) -> Vec<u8> {
    let mut bits = Bits::default();
    bits.put(0x2F, 8);
    bits.put(7, 14);
    bits.put(7, 14);
    bits.put(1, 1);
    bits.put(0, 3);
    // No transform, no colour cache, no meta prefix codes.
    bits.put(0, 1);
    bits.put(0, 1);
    bits.put(0, 1);
    let [red, green, blue, alpha] = colour;
    for symbol in [green, red, blue, alpha] {
        // Simple, one symbol, eight bits wide.
        bits.put(1, 1);
        bits.put(0, 1);
        bits.put(1, 1);
        bits.put(u32::from(symbol), 8);
    }
    // The distance code: simple, one symbol, one bit wide, symbol 0.
    bits.put(1, 1);
    bits.put(0, 1);
    bits.put(0, 1);
    bits.put(0, 1);
    bits.bytes
}

/// A RIFF chunk, padded to an even length.
fn chunk(name: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name);
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("a small chunk")
            .to_le_bytes(),
    );
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// Three little-endian bytes.
fn u24(value: u32) -> [u8; 3] {
    let [a, b, c, _] = value.to_le_bytes();
    [a, b, c]
}

/// The animated WebP: an 8 × 8 canvas, each frame the whole canvas in one
/// colour (a VP8L bitstream), shown for its milliseconds without blending,
/// played `plays` times.
fn webp(plays: u16, frames: &[([u8; 4], u32)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WEBP");
    let mut vp8x = vec![0b0001_0010, 0, 0, 0];
    vp8x.extend_from_slice(&u24(7));
    vp8x.extend_from_slice(&u24(7));
    body.extend(chunk(*b"VP8X", &vp8x));
    let mut anim = vec![255, 255, 255, 255];
    anim.extend_from_slice(&plays.to_le_bytes());
    body.extend(chunk(*b"ANIM", &anim));
    for &(colour, millis) in frames {
        let mut anmf = Vec::new();
        anmf.extend_from_slice(&u24(0));
        anmf.extend_from_slice(&u24(0));
        anmf.extend_from_slice(&u24(7));
        anmf.extend_from_slice(&u24(7));
        anmf.extend_from_slice(&u24(millis));
        // Do not blend, do not dispose.
        anmf.push(0b0000_0010);
        anmf.extend(chunk(*b"VP8L", &vp8l(colour)));
        body.extend(chunk(*b"ANMF", &anmf));
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(
        &u32::try_from(body.len())
            .expect("a small file")
            .to_le_bytes(),
    );
    out.extend(body);
    out
}

/// Every fixture, by file name.
fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        // Red 100 ms, green 200 ms, then blue over the left half 300 ms;
        // NETSCAPE repeat 1, which browsers play twice.
        (
            "rgb-twice.gif",
            gif(Some(1), &[(10, 0, 8, 0), (20, 0, 8, 1), (30, 0, 4, 2)]),
        ),
        // White 10 ms (played as 100 ms), red 50 ms; NETSCAPE repeat 0,
        // forever.
        (
            "fast-forever.gif",
            gif(Some(0), &[(1, 0, 8, 3), (5, 0, 8, 0)]),
        ),
        // Red, green and blue, without the NETSCAPE extension: played once.
        (
            "rgb-once.gif",
            gif(None, &[(10, 0, 8, 0), (10, 0, 8, 1), (10, 0, 8, 2)]),
        ),
        // Red 5 ms (played as 100 ms), green 200 ms, blue 300 ms; played
        // twice.
        (
            "rgb-twice.png",
            apng(2, &[(RED, 5, 1000), (GREEN, 200, 1000), (BLUE, 300, 1000)]),
        ),
        // Red 100 ms, green 200 ms, blue 300 ms; played three times.
        (
            "rgb-thrice.webp",
            webp(3, &[(RED, 100), (GREEN, 200), (BLUE, 300)]),
        ),
    ]
}

#[test]
fn the_animated_fixtures_are_what_this_file_writes() {
    let write = std::env::var_os("BOBCAT_WRITE_FIXTURES").is_some();
    let mut stale = Vec::new();
    for (name, bytes) in fixtures() {
        let path = dir().join(name);
        if write {
            std::fs::create_dir_all(dir()).expect("the fixture directory");
            std::fs::write(&path, &bytes).expect("a fixture");
            stale.push(format!("{name} written"));
        } else if std::fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
            stale.push(format!("{name} differs from what this file writes"));
        }
    }
    assert!(stale.is_empty(), "{stale:#?}");
}
