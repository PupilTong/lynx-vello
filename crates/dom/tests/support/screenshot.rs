//! Shared capture harness for the screenshot binaries.
//!
//! Every screenshot fixture in this crate draws a **vendored** face and
//! nothing else — Roboto through [`capture`], whether or not the fixture draws
//! text, and [`AHEM`] where a binary wants em squares instead. A fixture that
//! resolved a *host* font could not have a committed golden at all:
//! `flashbulb` goldens carry no platform suffix, and its tolerance absorbs
//! rasterizer noise, not a different typeface.

#![allow(dead_code)]

use flashbulb::vello::peniko::Color;
use flashbulb::{Image, capture_document, headless};

use crate::html;

pub(super) const ROBOTO: &[u8] =
    include_bytes!("../../../hughie/tests/fixtures/Roboto-Regular.ttf");

/// The vendored Ahem face, for the rare golden whose subject is *geometry* —
/// solid em squares make a cut point, a retreat and a clamp visible at a
/// glance, where a proportional face only shows that some glyphs are there.
/// It is vendored too, so a golden drawn with it is as reproducible as a
/// Roboto one; it is simply the wrong face for a picture about rendering.
pub(super) const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

pub(super) fn capture(test: &str, fragment: &str, width: f32, height: f32) -> Image {
    let mut gpu = headless(test);
    let mut doc = html::parse(fragment, width, height);
    assert_eq!(
        doc.dom.register_fonts(dom::FontBlob::from_static(ROBOTO)),
        1,
        "the vendored Roboto fixture must register exactly one face"
    );
    capture_document(&mut gpu, &mut doc.dom, Color::WHITE, &dom::NoImages)
        .expect("headless screenshot render")
}

pub(super) fn capture_prebuilt_document<T: Sync>(
    test: &str,
    document: &mut dom::Document<T>,
    pixels: &impl dom::FrameImages,
) -> Image {
    let mut gpu = headless(test);
    capture_document(&mut gpu, document, Color::WHITE, pixels).expect("headless screenshot render")
}

pub(super) fn assert_golden(name: &[&str], actual: &Image) {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR")).assert_matches(name, actual);
}
