//! End-to-end replaced content: real encoded bytes → `crates/image` decode →
//! natural size into layout → `object-fit` geometry at paint.
//!
//! Structural (no GPU): asserts on the scene encoding and on the layout the
//! natural size produces. The pixel-level check is the `replaced-object-fit`
//! screenshot golden.

mod paint_common;

use dom::layout::{NaturalSize, Size};
use image::{BackendRegistry, DecodeRequest, decode_bytes};
use paint_common::Doc;

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
    img { display: flex; position: absolute; left: 0; top: 0; }
    .box { width: 200px; height: 100px; }";

/// A 4x4 PNG with four distinguishable quadrants, encoded in-process so no
/// binary fixture is needed for the common case.
fn checker_png(side: u32) -> Vec<u8> {
    let half = side / 2;
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let pixel = match (x < half, y < half) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 255, 255],
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

struct Harness {
    doc: Doc,
}

impl Harness {
    fn new(css: &str) -> Self {
        Self {
            doc: Doc::with_css(&format!("{PAGE} {css}")),
        }
    }

    /// The whole pipeline for one `<img>`: decode, publish the natural size to
    /// layout, publish the pixels to paint.
    fn img(&mut self, class: &str, bytes: &[u8]) -> dom::NodeId {
        let decoded = decode_bytes(
            &BackendRegistry::software_only(),
            bytes,
            &DecodeRequest::default(),
        )
        .expect("decode");

        let root = self.doc.root;
        let node = self.doc.el_tag(root, "img", class);
        #[allow(clippy::cast_precision_loss)]
        let natural = NaturalSize::from_size(Size::new(
            decoded.header.natural_size.width as f32,
            decoded.header.natural_size.height as f32,
        ));
        self.doc.dom.set_natural_size(node, natural);
        self.doc
            .dom
            .images_mut()
            .insert_node(node, decoded.image.to_image_data());
        node
    }

    fn stats(&mut self) -> (usize, u32) {
        self.doc.dom.render();
        let scene = self.doc.dom.scene();
        let encoding = scene.encoding();
        (encoding.draw_tags.len(), encoding.n_open_clips)
    }
}

#[test]
fn a_decoded_image_paints_and_leaves_every_layer_closed() {
    let mut h = Harness::new("");
    h.img("box", &checker_png(4));
    let (draws, open) = h.stats();
    assert!(draws > 0, "replaced content must encode at least one draw");
    assert_eq!(open, 0, "every pushed layer must be popped");
}

#[test]
fn the_natural_size_reaches_layout_when_no_size_is_specified() {
    // The auto/auto case: with no CSS width or height, the used size IS the
    // decoded intrinsic size. This is the whole reason the decode has to reach
    // the document at all.
    let mut h = Harness::new("");
    let node = h.img("", &checker_png(8));
    h.doc.dom.layout();

    let layout = h.doc.dom.rounded_layout(node).expect("laid out");
    assert_eq!(
        (layout.size.width, layout.size.height),
        (8.0, 8.0),
        "an unsized replaced element takes its intrinsic size"
    );
}

#[test]
fn the_natural_size_round_trips_through_the_document() {
    let mut h = Harness::new("");
    let node = h.img("box", &checker_png(4));
    let natural = h.doc.dom.natural_size(node);
    assert_eq!(
        natural.dimensions(),
        Size::new(Some(4.0), Some(4.0)),
        "paint reads this back to resolve object-fit"
    );
}

#[test]
fn republishing_an_identical_natural_size_does_not_dirty_layout() {
    // A loader that re-decodes after a cache eviction publishes the same value
    // again; that has to be free, which is why `set_natural_size` compares
    // exactly rather than with native Lynx's 5% aspect epsilon.
    let mut h = Harness::new("box");
    let node = h.img("box", &checker_png(4));
    h.doc.dom.layout();
    assert_eq!(
        h.doc.dom.layout_cache_is_empty(node),
        Some(false),
        "laid out once, so the measurement cache is populated"
    );

    h.doc
        .dom
        .set_natural_size(node, NaturalSize::from_size(Size::new(4.0, 4.0)));
    assert_eq!(
        h.doc.dom.layout_cache_is_empty(node),
        Some(false),
        "an equal natural size must be a structural no-op"
    );

    // A different one must invalidate.
    h.doc
        .dom
        .set_natural_size(node, NaturalSize::from_size(Size::new(9.0, 9.0)));
    assert_eq!(
        h.doc.dom.layout_cache_is_empty(node),
        Some(true),
        "a changed natural size must invalidate the box cache"
    );
}

#[test]
fn a_node_with_no_registered_pixels_paints_nothing_but_still_lays_out() {
    // The not-yet-loaded state: layout has the natural size (from a header
    // probe) while the pixels are still in flight. That frame must not panic
    // and must not draw a placeholder.
    let mut h = Harness::new("");
    let root = h.doc.root;
    let node = h.doc.el_tag(root, "img", "");
    h.doc
        .dom
        .set_natural_size(node, NaturalSize::from_size(Size::new(40.0, 20.0)));

    let (_, open) = h.stats();
    assert_eq!(open, 0);
    let layout = h.doc.dom.rounded_layout(node).expect("laid out");
    assert_eq!((layout.size.width, layout.size.height), (40.0, 20.0));
}

#[test]
fn every_object_fit_value_paints_without_unbalancing_layers() {
    // `contain`/`scale-down` underflow the content box, `cover` and an
    // oversized `none` overflow it — the overflowing cases take the clip path,
    // and a clip that is pushed and not popped corrupts every later item.
    for fit in ["fill", "contain", "cover", "none", "scale-down"] {
        let mut h = Harness::new(&format!(".box {{ object-fit: {fit}; }}"));
        h.img("box", &checker_png(4));
        let (draws, open) = h.stats();
        assert!(draws > 0, "object-fit: {fit} must draw");
        assert_eq!(open, 0, "object-fit: {fit} left a layer open");
    }
}

#[test]
fn object_position_and_image_rendering_are_accepted_together() {
    let mut h = Harness::new(
        ".box { object-fit: none; object-position: 100% 0%; image-rendering: pixelated; }",
    );
    h.img("box", &checker_png(4));
    let (draws, open) = h.stats();
    assert!(draws > 0);
    assert_eq!(open, 0);
}
