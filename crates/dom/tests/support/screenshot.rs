//! Shared capture harness for the screenshot binaries.
//!
//! Every screenshot fixture in this crate gets the same font environment —
//! the vendored Roboto face and nothing else — whether or not it draws text.
//! A fixture that resolved a *host* font could not have a committed golden at
//! all: `flashbulb` goldens carry no platform suffix, and its tolerance
//! absorbs rasterizer noise, not a different typeface.

#![allow(dead_code)]

use flashbulb::vello::peniko::Color;
use flashbulb::{Image, capture_document, headless};

use crate::html;

/// The vendored text fixture; see `crates/hughie/tests/fixtures/README.md`.
pub const ROBOTO: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Roboto-Regular.ttf");

/// Renders `fragment` at `width` × `height` CSS pixels over white.
pub fn capture(test: &str, fragment: &str, width: f32, height: f32) -> Image {
    let mut gpu = headless(test);
    let mut doc = html::parse(fragment, width, height);
    assert_eq!(
        doc.dom.register_fonts(ROBOTO),
        1,
        "the vendored Roboto fixture must register exactly one face"
    );
    capture_document(&mut gpu, &mut doc.dom, Color::WHITE).expect("headless screenshot render")
}

/// [`capture`] for a document the caller has already built. Replaced content
/// must already be registered in the document's own image store.
///
/// The fragment path above cannot serve this: an `<img>` case has to publish
/// natural sizes and decoded pixels itself, which is not expressible as inline
/// HTML.
pub fn capture_prebuilt_document<T: Sync>(test: &str, document: &mut dom::Document<T>) -> Image {
    let mut gpu = headless(test);
    capture_document(&mut gpu, document, Color::WHITE).expect("headless screenshot render")
}

/// Compares against the golden at `name`, or accepts it under
/// `FLASHBULB_UPDATE_SNAPSHOTS=1`. The store is per *crate*, so every
/// screenshot binary here shares one `tests/screenshots` tree.
pub fn assert_golden(name: &[&str], actual: &Image) {
    flashbulb::screenshots_in(env!("CARGO_MANIFEST_DIR")).assert_matches(name, actual);
}
