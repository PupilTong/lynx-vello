//! End-to-end replaced content: encoded bytes → decoded pixels → natural size
//! into layout → `object-fit` geometry at paint.
//!
//! Structural (no GPU): asserts on the scene encoding and on the layout the
//! natural size produces. The pixel-level check is the `replaced-object-fit`
//! screenshot golden. Decoding here is the `png` crate driven directly — the
//! real decode pipeline lives above this layer, in the engine and its
//! embedder, and this test's subject is replaced-content layout and paint,
//! not codecs.

mod paint_common;

use std::sync::Arc;

use dom::layout::{NaturalSize, Size};
use flashbulb::TestImages;
use paint_common::Doc;

const PAGE: &str = "page { display: flex; position: relative; width: 800px; height: 600px; }
    img { display: flex; position: absolute; left: 0; top: 0; }
    .box { width: 200px; height: 100px; }";

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
    images: Arc<TestImages>,
}

impl Harness {
    fn new(css: &str) -> Self {
        let mut doc = Doc::with_css(&format!("{PAGE} {css}"));
        let images = Arc::new(TestImages::new());
        doc.dom
            .set_image_store(Arc::clone(&images) as Arc<dyn dom::ImageStore>);
        Self { doc, images }
    }

    fn img(&mut self, class: &str, bytes: &[u8]) -> dom::NodeId {
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("png header");
        let mut rgba = vec![0u8; reader.output_buffer_size().expect("png size")];
        let info = reader.next_frame(&mut rgba).expect("png frame");
        rgba.truncate(info.buffer_size());

        let root = self.doc.root;
        let node = self.doc.el_tag(root, "img", class);
        #[allow(clippy::cast_precision_loss)]
        let natural = NaturalSize::from_size(Size::new(info.width as f32, info.height as f32));
        let source = format!("app:///{}.png", node.to_bits());
        self.images
            .insert_rgba8(&source, info.width, info.height, rgba);
        self.doc.dom.set_natural_size(node, natural);
        self.doc.dom.set_image_source(node, Some(&source));
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
fn natural_size_updates_change_auto_sized_layout() {
    let mut h = Harness::new("");
    let node = h.img("", &checker_png(4));
    h.doc.dom.layout();
    let size = |h: &Harness| h.doc.dom.rounded_layout(node).expect("laid out").size;
    assert_eq!(size(&h), Size::new(4.0, 4.0));

    h.doc
        .dom
        .set_natural_size(node, NaturalSize::from_size(Size::new(4.0, 4.0)));
    h.doc.dom.layout();
    assert_eq!(size(&h), Size::new(4.0, 4.0));

    h.doc
        .dom
        .set_natural_size(node, NaturalSize::from_size(Size::new(9.0, 9.0)));
    h.doc.dom.layout();
    assert_eq!(size(&h), Size::new(9.0, 9.0));
}

#[test]
fn a_node_with_no_registered_pixels_paints_nothing_but_still_lays_out() {
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
