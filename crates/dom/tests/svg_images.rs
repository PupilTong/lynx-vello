//! SVG documents as vector images: `<image src>`, `background-image` and
//! `mask-image` naming an SVG document, and an element handed a document as
//! its markup (`Document::set_image_document`, what the Lynx
//! `<svg content>` calls), over the full test pipeline —
//! `flashbulb::TestImages::insert_svg` reports the document's bytes the way a
//! host does, `flashbulb::pump_images` parses an element's markup the way
//! the wasm32 runtime does, `Document::apply_image_events` files either in
//! the registry, the paint walk records one vector draw per visible tile, and
//! the headless renderer's raster cache bakes each draw's texture before the
//! frame composes.
//!
//! The captures pin that a vector image is never read through
//! `FrameImages`, and the structural tests the natural size layout reads
//! and the synthetic source markup is filed under; the per-tile op emission
//! and the empty-document rule are unit tests beside the producers
//! (`paint/background.rs`), the registry's half of the synthetic source unit
//! tests beside it (`render/image.rs`). The goldens under
//! `tests/screenshots/svg/` are regression goldens of our own output, not
//! browser references. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test svg_images`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

use dom::{DocumentKind, ImageOutcome, ImageRole, NodeId};
use flashbulb::TestImages;
use paint_common::Doc;

const SCREEN_WIDTH: f32 = 393.0;
const SCREEN_HEIGHT: f32 = 727.0;

/// A 2:1 icon with its own outline, so a cell shows exactly where the
/// document's viewport landed and whether its ratio survived.
const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="24" viewBox="0 0 48 24">
  <rect width="48" height="24" fill="#dbeafe"/>
  <circle cx="12" cy="12" r="8" fill="#dc2626"/>
  <path d="M26 20 L34 4 L42 20 Z" fill="#16a34a" stroke="#14532d" stroke-width="2" stroke-linejoin="round"/>
  <rect x="1" y="1" width="46" height="22" fill="none" stroke="#1f2937" stroke-width="2"/>
</svg>"##;

/// A square tile: an object-bounding-box gradient, a dashed ring, and a
/// clip-path, so a tiled background exercises three walker rules at once.
const TILE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#f59e0b"/>
      <stop offset="1" stop-color="#7c3aed"/>
    </linearGradient>
    <clipPath id="c"><rect x="2" y="2" width="16" height="16" rx="4"/></clipPath>
  </defs>
  <rect width="20" height="20" fill="url(#g)" clip-path="url(#c)"/>
  <circle cx="10" cy="10" r="5" fill="none" stroke="#ffffff" stroke-width="1.5" stroke-dasharray="2 1.5"/>
</svg>"##;

/// An opaque star and, in the top-right corner, a half-transparent disc, as
/// a mask: what shows through is the star's silhouette at full strength and
/// the disc at half.
const STAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <path d="M20 2 L25 15 L38 15 L27 23 L31 37 L20 29 L9 37 L13 23 L2 15 L15 15 Z" fill="#000000"/>
  <circle cx="35" cy="5" r="4" fill="#000000" fill-opacity="0.5"/>
</svg>"##;

/// Fine detail that a bitmap scaled tenfold would show as blocks: hairline
/// strokes, a radial gradient with a focal point, small circles.
const DETAIL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <defs>
    <radialGradient id="r" cx="12" cy="12" r="11" fx="8" fy="8" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#fde68a"/>
      <stop offset="1" stop-color="#b45309"/>
    </radialGradient>
  </defs>
  <circle cx="12" cy="12" r="11" fill="url(#r)"/>
  <path d="M4 12 H20 M12 4 V20" stroke="#1f2937" stroke-width="0.5"/>
  <circle cx="7" cy="17" r="1.5" fill="#1d4ed8"/>
  <circle cx="17" cy="7" r="1.5" fill="#be123c"/>
</svg>"##;

/// The `mode` rules of the Lynx `<image>` UA sheet
/// (`crates/bobcat-core/src/main/tree/image.rs`), so these cells read the
/// way a page's `<image mode>` does.
const IMAGE_MODES: &str = r#"
    image[mode="aspectFit"] { object-fit: contain; }
    image[mode="aspectFill"] { object-fit: cover; }
    image[mode="center"] { object-fit: none; }
"#;

fn page(css: &str) -> Doc {
    Doc::with_css_sized(
        &format!(
            "page {{ display: flex; position: relative; width: 393px; height: 727px;
                     background-color: #e5e7eb; }}
             image, view {{ display: flex; position: absolute; }}
             {IMAGE_MODES} {css}"
        ),
        SCREEN_WIDTH,
        SCREEN_HEIGHT,
    )
}

fn capture(test: &str, doc: &mut Doc, images: &TestImages) -> flashbulb::Image {
    flashbulb::render_with_images(&mut doc.dom, images);
    let image = screenshot::capture_prebuilt_document(test, &mut doc.dom, images);
    assert!(
        images.reads().is_empty(),
        "a vector image is never read through `FrameImages`: {:?}",
        images.reads()
    );
    image
}

/// `<image src="icon.svg">` under every `mode`, plus the clips a replaced
/// element adds: a rounded content box and a border.
#[test]
fn image_element_modes_match_reference() {
    let mut doc = page(
        "image { width: 110px; height: 80px; background-color: #ffffff; }
         .c0 { left: 10px; } .c1 { left: 141px; } .c2 { left: 272px; }
         .r0 { top: 10px; } .r1 { top: 100px; } .r2 { top: 190px; }
         .rounded { border-radius: 24px; }
         .bordered { border: 6px solid #1f2937; }
         .wide { width: 160px; height: 40px; }",
    );
    let images = TestImages::new();
    images.insert_svg("app:///icon.svg", ICON);
    let cells: &[(&str, Option<&str>)] = &[
        ("r0 c0", None),
        ("r0 c1", Some("scaleToFill")),
        ("r0 c2", Some("aspectFit")),
        ("r1 c0", Some("aspectFill")),
        ("r1 c1", Some("center")),
        ("r1 c2 wide", Some("aspectFit")),
        ("r2 c0 rounded", Some("aspectFill")),
        ("r2 c1 bordered", Some("aspectFit")),
        ("r2 c2 rounded bordered", None),
    ];
    let root = doc.root;
    for (class, mode) in cells {
        let node = doc.el_tag(root, "image", class);
        if let Some(mode) = mode {
            doc.dom.set_attribute(node, "mode", mode);
        }
        doc.dom
            .set_image_source(node, ImageRole::Source, Some("app:///icon.svg"));
    }
    let actual = capture("image_element_modes_match_reference", &mut doc, &images);
    screenshot::assert_golden(&["svg", "image-modes"], &actual);
}

/// `background-image: url(tile.svg)` under `background-size`,
/// `background-repeat` and `background-position`, plus a rounded clip that
/// cuts tiles and a `content-box` clip.
#[test]
fn background_image_tiles_match_reference() {
    let mut doc = page(
        "view { width: 170px; height: 110px; background-color: #ffffff;
                background-image: url(app:///tile.svg); }
         .c0 { left: 12px; } .c1 { left: 211px; }
         .r0 { top: 12px; } .r1 { top: 142px; } .r2 { top: 272px; } .r3 { top: 402px; }
         .sized { background-size: 30px 30px; }
         .centred { background-repeat: no-repeat; background-position: center;
                    background-size: 60px 60px; }
         .row { background-repeat: repeat-x; background-size: 24px; }
         .contain { background-repeat: no-repeat; background-size: contain; }
         .rounded { border-radius: 36px; background-size: 40px 40px; }
         .round { background-repeat: round; background-size: 32px 32px; }
         .clipped { border: 8px solid #1f2937; padding: 10px; box-sizing: border-box;
                    background-clip: content-box; background-origin: content-box;
                    background-size: 25px; }",
    );
    let images = TestImages::new();
    images.insert_svg("app:///tile.svg", TILE);
    let root = doc.root;
    for class in [
        "r0 c0",
        "r0 c1 sized",
        "r1 c0 centred",
        "r1 c1 row",
        "r2 c0 contain",
        "r2 c1 rounded",
        "r3 c0 round",
        "r3 c1 clipped",
    ] {
        doc.el_tag(root, "view", class);
    }
    let actual = capture("background_image_tiles_match_reference", &mut doc, &images);
    screenshot::assert_golden(&["svg", "background"], &actual);
}

/// `mask-image: url(star.svg)`: one star, a repeated star, and a star mask
/// over a rounded box with a gradient background.
#[test]
fn mask_image_matches_reference() {
    let mut doc = page(
        "view { width: 160px; height: 160px; background-color: #2563eb;
                mask-image: url(app:///star.svg); }
         .c0 { left: 20px; } .c1 { left: 210px; }
         .r0 { top: 20px; } .r1 { top: 210px; }
         .one { mask-repeat: no-repeat; mask-size: contain; }
         .tiled { mask-size: 40px 40px; }
         .offset { mask-repeat: no-repeat; mask-size: 100px 100px; mask-position: right bottom; }
         .gradient { border-radius: 40px; mask-repeat: no-repeat; mask-size: cover;
                     background-image: linear-gradient(135deg, #f43f5e, #22c55e); }",
    );
    let images = TestImages::new();
    images.insert_svg("app:///star.svg", STAR);
    let root = doc.root;
    for class in ["r0 c0 one", "r0 c1 tiled", "r1 c0 offset", "r1 c1 gradient"] {
        doc.el_tag(root, "view", class);
    }
    let actual = capture("mask_image_matches_reference", &mut doc, &images);
    screenshot::assert_golden(&["svg", "mask"], &actual);
}

/// One document drawn at 24 px and at 240 px: the large one has the same
/// edges a 24 px draw has, ten times as long, because the walker encodes
/// paths, not a bitmap.
#[test]
fn one_document_at_two_sizes_matches_reference() {
    let mut doc = page(
        ".small { left: 20px; top: 20px; width: 24px; height: 24px; }
         .large { left: 70px; top: 20px; width: 240px; height: 240px; }",
    );
    let images = TestImages::new();
    images.insert_svg("app:///detail.svg", DETAIL);
    let root = doc.root;
    for class in ["small", "large"] {
        let node = doc.el_tag(root, "image", class);
        doc.dom
            .set_image_source(node, ImageRole::Source, Some("app:///detail.svg"));
    }
    let actual = capture(
        "one_document_at_two_sizes_matches_reference",
        &mut doc,
        &images,
    );
    screenshot::assert_golden(&["svg", "two-sizes"], &actual);
}

/// The natural size layout reads follows CSS Images 3 default sizing: both
/// dimensions; one plus the `viewBox` ratio; the `viewBox` ratio fitted into
/// 300x150; neither, 300x150.
#[test]
fn an_unsized_image_lays_out_at_the_documents_natural_size() {
    let cases = [
        (r#"width="40" height="30""#, (40.0, 30.0)),
        (r#"width="40" viewBox="0 0 20 10""#, (40.0, 20.0)),
        (r#"height="30" viewBox="0 0 20 10""#, (60.0, 30.0)),
        (r#"viewBox="0 0 10 10""#, (150.0, 150.0)),
        (r#"viewBox="0 0 40 10""#, (300.0, 75.0)),
        (r#"width="50%" height="20""#, (300.0, 20.0)),
        ("", (300.0, 150.0)),
    ];
    for (attributes, expected) in cases {
        let images = TestImages::new();
        images.insert_svg(
            "app:///sized.svg",
            &format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" {attributes}><rect width="5" height="5"/></svg>"#
            ),
        );
        let mut doc = page("");
        let root = doc.root;
        let node = doc.el_tag(root, "image", "");
        doc.dom
            .set_image_source(node, ImageRole::Source, Some("app:///sized.svg"));
        flashbulb::render_with_images(&mut doc.dom, &images);
        let layout = doc.dom.rounded_layout(node).expect("laid out");
        assert_eq!(
            (f64::from(layout.size.width), f64::from(layout.size.height)),
            expected,
            "<svg {attributes}>"
        );
    }
}

/// The source `node` presents, owned, so the document can be borrowed again.
fn source_of(doc: &Doc, node: NodeId) -> String {
    doc.dom
        .image_source(node, ImageRole::Source)
        .expect("the element has a source")
        .to_owned()
}

/// The border-box size `node` laid out at.
fn size_of(doc: &mut Doc, node: NodeId) -> (f32, f32) {
    doc.dom.layout();
    let layout = doc.dom.rounded_layout(node).expect("laid out");
    (layout.size.width, layout.size.height)
}

/// Parses and applies every pending document, as the wasm32 runtime does.
fn parse_pending(doc: &mut Doc) -> Vec<ImageOutcome> {
    doc.dom.apply_pending_documents()
}

/// An element handed [`ICON`] as markup draws exactly what an `<image>`
/// whose URL the host answers with the same document draws: the markup goes
/// through the same parse, the same kind of registry entry and the same
/// raster cache, and the host is never asked for it.
#[test]
fn a_document_set_as_markup_draws_as_one_loaded_from_a_url() {
    const TEST: &str = "a_document_set_as_markup_draws_as_one_loaded_from_a_url";
    let css = ".icon { left: 20px; top: 20px; width: 160px; height: 80px; }";

    let url_images = TestImages::new();
    url_images.insert_svg("app:///icon.svg", ICON);
    let mut by_url = page(css);
    let root = by_url.root;
    let node = by_url.el_tag(root, "image", "icon");
    by_url
        .dom
        .set_image_source(node, ImageRole::Source, Some("app:///icon.svg"));
    let by_url = capture(TEST, &mut by_url, &url_images);

    let markup_images = TestImages::new();
    let mut by_markup = page(css);
    let root = by_markup.root;
    let node = by_markup.el_tag(root, "image", "icon");
    by_markup
        .dom
        .set_image_document(node, ImageRole::Source, ICON.as_bytes(), DocumentKind::Svg);
    let source = source_of(&by_markup, node);
    let by_markup = capture(TEST, &mut by_markup, &markup_images);
    assert!(
        !markup_images.was_asked_for(&source),
        "the host is never asked for markup"
    );

    assert_eq!(
        (by_markup.width(), by_markup.height()),
        (by_url.width(), by_url.height())
    );
    assert!(
        by_markup.pixels() == by_url.pixels(),
        "markup and URL draw the same picture"
    );
}

/// Two elements handed identical markup share one synthetic source and one
/// parse, and both settle with the document's natural size when it applies.
#[test]
fn identical_markup_on_two_elements_is_one_source_and_one_parse() {
    let mut doc = page("");
    let root = doc.root;
    let first = doc.el_tag(root, "image", "");
    let second = doc.el_tag(root, "image", "");
    for node in [first, second] {
        assert_eq!(
            doc.dom
                .set_image_document(node, ImageRole::Source, ICON.as_bytes(), DocumentKind::Svg),
            None,
            "pending until the parse applies"
        );
    }
    let source = source_of(&doc, first);
    assert!(source.starts_with("svg-content:"), "{source}");
    assert_eq!(source_of(&doc, second), source);
    assert!(
        doc.dom.take_wanted_images().is_empty(),
        "nothing for the host"
    );

    let pending = doc.dom.take_pending_documents();
    assert_eq!(pending.len(), 1, "one parse for both elements");
    assert!(doc.dom.take_pending_documents().is_empty(), "drained once");
    let events: Vec<_> = pending
        .into_iter()
        .map(|(source, bytes, kind)| dom::ImageEvent::parse_document(source, &bytes, kind))
        .collect();
    assert_eq!(
        doc.dom.apply_image_events(&events),
        [first, second].map(|node| ImageOutcome::Loaded {
            node,
            width: 48,
            height: 24,
        })
    );
    assert_eq!(size_of(&mut doc, first), (48.0, 24.0));
    assert_eq!(size_of(&mut doc, second), (48.0, 24.0));

    // A third element on a settled markup is answered at the bind.
    let third = doc.el_tag(root, "image", "");
    assert_eq!(
        doc.dom
            .set_image_document(third, ImageRole::Source, ICON.as_bytes(), DocumentKind::Svg),
        Some(ImageOutcome::Loaded {
            node: third,
            width: 48,
            height: 24,
        })
    );
    assert!(doc.dom.take_pending_documents().is_empty());
}

/// New markup rebinds the element to a new source and forgets the old one,
/// whose bytes are never parsed if the parse had not started; setting the
/// same markup again is no change at all.
#[test]
fn changed_markup_rebinds_and_forgets_the_old_source() {
    let mut doc = page("");
    let root = doc.root;
    let node = doc.el_tag(root, "image", "");
    doc.dom
        .set_image_document(node, ImageRole::Source, ICON.as_bytes(), DocumentKind::Svg);
    let icon = source_of(&doc, node);
    doc.dom
        .set_image_document(node, ImageRole::Source, TILE.as_bytes(), DocumentKind::Svg);
    let tile = source_of(&doc, node);
    assert_ne!(icon, tile);
    assert!(!doc.dom.knows_image_source(&icon), "the old source is gone");

    let pending = doc.dom.take_pending_documents();
    assert_eq!(
        pending
            .iter()
            .map(|(source, ..)| &**source)
            .collect::<Vec<_>>(),
        [tile.as_str()],
        "only the markup the element still presents is parsed"
    );
    doc.dom.apply_image_events(
        &pending
            .into_iter()
            .map(|(source, bytes, kind)| dom::ImageEvent::parse_document(source, &bytes, kind))
            .collect::<Vec<_>>(),
    );
    assert_eq!(size_of(&mut doc, node), (20.0, 20.0));

    assert_eq!(
        doc.dom
            .set_image_document(node, ImageRole::Source, TILE.as_bytes(), DocumentKind::Svg),
        None,
        "the same markup again changes nothing"
    );
    assert!(doc.dom.take_pending_documents().is_empty());

    // Back to the first markup: a new parse, and the settled one is gone.
    doc.dom
        .set_image_document(node, ImageRole::Source, ICON.as_bytes(), DocumentKind::Svg);
    assert!(!doc.dom.knows_image_source(&tile));
    assert_eq!(
        parse_pending(&mut doc),
        [ImageOutcome::Loaded {
            node,
            width: 48,
            height: 24,
        }]
    );
    assert_eq!(size_of(&mut doc, node), (48.0, 24.0));
}

/// A synthetic source outlives every element but the last presenting it:
/// freeing one of two keeps it, freeing the second forgets it, and a parse
/// that lands afterwards applies to nothing.
#[test]
fn freeing_the_last_element_forgets_its_source() {
    let mut doc = page("");
    let root = doc.root;
    let first = doc.el_tag(root, "image", "");
    let second = doc.el_tag(root, "image", "");
    for node in [first, second] {
        doc.dom
            .set_image_document(node, ImageRole::Source, ICON.as_bytes(), DocumentKind::Svg);
    }
    let source = source_of(&doc, first);
    let pending = doc.dom.take_pending_documents();

    doc.dom.drop_element(first);
    assert!(doc.dom.knows_image_source(&source), "one element is left");
    doc.dom.drop_element(second);
    assert!(
        !doc.dom.knows_image_source(&source),
        "forgotten with the last element"
    );

    let events: Vec<_> = pending
        .into_iter()
        .map(|(source, bytes, kind)| dom::ImageEvent::parse_document(source, &bytes, kind))
        .collect();
    assert!(doc.dom.apply_image_events(&events).is_empty());
    assert!(!doc.dom.knows_image_source(&source), "and stays forgotten");
}
