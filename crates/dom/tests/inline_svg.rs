//! The standard `<svg>` element, as a subset: an `svg` element and its
//! descendants are DOM nodes, and the root's subtree is serialised and
//! parsed into a vector image at the start of every `Document::layout`
//! that follows a mutation inside it.
//!
//! The golden under `tests/screenshots/svg/` is a regression golden of our
//! own output, not a browser reference. Refresh with:
//! `FLASHBULB_UPDATE_SNAPSHOTS=1 cargo test -p dom --test inline_svg`.

#[path = "support/html.rs"]
mod html;
mod paint_common;
#[path = "support/screenshot.rs"]
mod screenshot;

use dom::{ImageRole, NodeId};
use flashbulb::TestImages;
use paint_common::Doc;

const WIDTH: f32 = 260.0;
const HEIGHT: f32 = 80.0;

fn page() -> Doc {
    Doc::with_css_sized(
        "page { display: flex; position: relative; width: 260px; height: 80px;
                background-color: #ffffff; }
         svg { display: flex; position: absolute; top: 16px; }
         .c0 { left: 16px; } .c1 { left: 80px; } .c2 { left: 144px; } .c3 { left: 208px; }",
        WIDTH,
        HEIGHT,
    )
}

/// Creates `tag` under `parent` with `attributes`, in order.
fn element(doc: &mut Doc, parent: NodeId, tag: &str, attributes: &[(&str, &str)]) -> NodeId {
    let id = doc.dom.create_element(tag, ());
    for (name, value) in attributes {
        doc.dom.set_attribute(id, name, value);
    }
    doc.dom.append_child(parent, id);
    id
}

/// A 48x48 `svg` at column `class` holding one red 20x20 square drawn
/// through a 24x24 `viewBox` (so 40x40 px, inset 4 px), and the square.
fn red_square(doc: &mut Doc, class: &str) -> (NodeId, NodeId) {
    let root = doc.root;
    let svg = element(
        doc,
        root,
        "svg",
        &[
            ("class", class),
            ("viewBox", "0 0 24 24"),
            ("width", "48"),
            ("height", "48"),
        ],
    );
    let path = element(
        doc,
        svg,
        "path",
        &[("d", "M2 2 H22 V22 H2 Z"), ("fill", "#ff0000")],
    );
    (svg, path)
}

fn size(doc: &Doc, node: NodeId) -> (f32, f32) {
    let layout = doc.dom.rounded_layout(node).expect("laid out");
    (layout.size.width, layout.size.height)
}

fn source(doc: &Doc, node: NodeId) -> String {
    doc.dom
        .image_source(node, ImageRole::Source)
        .expect("an inline SVG root is bound to a source")
        .to_owned()
}

fn capture(test: &str, doc: &mut Doc) -> flashbulb::Image {
    screenshot::capture_prebuilt_document(test, &mut doc.dom, &dom::NoImages)
}

/// The RGBA of the pixel at (`x`, `y`).
fn pixel(image: &flashbulb::Image, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width() + x) * 4) as usize;
    image.pixels()[at..at + 4]
        .try_into()
        .expect("four channels")
}

fn is_close(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= 8)
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A 48x48 root at column `c1` whose rounded rect fills with a
/// `linearGradient` referenced by `url(#g)`.
fn gradient_cell(doc: &mut Doc) -> NodeId {
    let root = doc.root;
    let gradient = element(
        doc,
        root,
        "svg",
        &[("class", "c1"), ("width", "48"), ("height", "48")],
    );
    let defs = element(doc, gradient, "defs", &[]);
    let linear = element(
        doc,
        defs,
        "linearGradient",
        &[
            ("id", "g"),
            ("x1", "0"),
            ("y1", "0"),
            ("x2", "1"),
            ("y2", "1"),
        ],
    );
    for (offset, colour) in [("0", "#f59e0b"), ("1", "#7c3aed")] {
        element(
            doc,
            linear,
            "stop",
            &[("offset", offset), ("stop-color", colour)],
        );
    }
    element(
        doc,
        gradient,
        "rect",
        &[
            ("width", "48"),
            ("height", "48"),
            ("rx", "8"),
            ("fill", "url(#g)"),
        ],
    );
    gradient
}

/// A root at column `c2` sized by `width` and a square `viewBox`, coloured
/// by a `<style>` sheet, with a nested `svg` holding a circle; answers the
/// root and the nested `svg`.
fn styled_cell(doc: &mut Doc) -> (NodeId, NodeId) {
    let root = doc.root;
    let styled = element(
        doc,
        root,
        "svg",
        &[("class", "c2"), ("viewBox", "0 0 48 48"), ("width", "48")],
    );
    let style = element(doc, styled, "style", &[]);
    let sheet = doc
        .dom
        .create_text_node(".a { fill: #2563eb; } svg > .b { fill: #16a34a; }", ());
    doc.dom.append_child(style, sheet);
    element(
        doc,
        styled,
        "rect",
        &[("class", "a"), ("width", "48"), ("height", "48")],
    );
    let nested = element(
        doc,
        styled,
        "svg",
        &[
            ("x", "12"),
            ("y", "12"),
            ("width", "24"),
            ("height", "24"),
            ("viewBox", "0 0 2 2"),
        ],
    );
    element(
        doc,
        nested,
        "circle",
        &[("class", "b"), ("cx", "1"), ("cy", "1"), ("r", "1")],
    );
    (styled, nested)
}

/// A 36x48 root at column `c3`: a stroked circle in a half-opaque group.
fn grouped_cell(doc: &mut Doc) -> NodeId {
    let root = doc.root;
    let grouped = element(
        doc,
        root,
        "svg",
        &[("class", "c3"), ("width", "36"), ("height", "48")],
    );
    let group = element(doc, grouped, "g", &[("opacity", "0.5")]);
    element(
        doc,
        group,
        "circle",
        &[
            ("cx", "18"),
            ("cy", "24"),
            ("r", "14"),
            ("fill", "#be123c"),
            ("stroke", "#1f2937"),
            ("stroke-width", "4"),
        ],
    );
    grouped
}

/// Four roots: a path through a `viewBox`; a gradient referenced by
/// `url(#id)`; a `<style>` sheet and a nested `svg`; a group with opacity
/// and a stroked circle. Each lays out at its `width`/`height` attributes.
#[test]
fn inline_svgs_lay_out_at_their_attribute_size_and_draw() {
    let mut doc = page();
    let (square, _) = red_square(&mut doc, "c0");
    let gradient = gradient_cell(&mut doc);
    let (styled, nested) = styled_cell(&mut doc);
    let grouped = grouped_cell(&mut doc);

    let image = capture(
        "inline_svgs_lay_out_at_their_attribute_size_and_draw",
        &mut doc,
    );
    assert_eq!(size(&doc, square), (48.0, 48.0));
    assert_eq!(size(&doc, gradient), (48.0, 48.0));
    assert_eq!(
        size(&doc, styled),
        (48.0, 48.0),
        "a `width` and a square `viewBox` give the height"
    );
    assert_eq!(size(&doc, grouped), (36.0, 48.0));
    assert_eq!(
        doc.dom.image_source(nested, ImageRole::Source),
        None,
        "a nested `svg` is part of its root's document"
    );
    assert!(is_close(pixel(&image, 16 + 24, 16 + 24), RED));
    assert!(is_close(pixel(&image, 16 + 1, 16 + 1), WHITE));
    screenshot::assert_golden(&["svg", "inline"], &image);
}

/// A mutation inside a root re-renders it at the next layout, under a new
/// source, and the superseded source leaves the registry.
#[test]
fn a_mutated_descendant_re_renders_on_the_next_layout() {
    let mut doc = page();
    let (svg, path) = red_square(&mut doc, "c0");
    let image = capture(
        "a_mutated_descendant_re_renders_on_the_next_layout",
        &mut doc,
    );
    assert!(is_close(pixel(&image, 16 + 24, 16 + 24), RED));
    let first = source(&doc, svg);
    assert!(doc.dom.knows_image_source(&first));

    doc.dom.set_attribute(path, "fill", "#0000ff");
    doc.dom.set_attribute(path, "d", "M12 12 H22 V22 H12 Z");
    assert!(doc.dom.needs_render(), "a mutation inside a root is visual");
    let image = capture(
        "a_mutated_descendant_re_renders_on_the_next_layout",
        &mut doc,
    );
    let second = source(&doc, svg);
    assert_ne!(first, second, "every refresh mints a new generation");
    assert!(
        !doc.dom.knows_image_source(&first),
        "the superseded generation is forgotten"
    );
    assert!(doc.dom.knows_image_source(&second));
    assert!(is_close(pixel(&image, 16 + 36, 16 + 36), BLUE));
    assert!(
        is_close(pixel(&image, 16 + 12, 16 + 12), WHITE),
        "the path's new `d` no longer covers the top-left"
    );

    // A text node inside a `<style>` is part of the markup too.
    let style = element(&mut doc, svg, "style", &[]);
    let sheet = doc.dom.create_text_node("path { fill: #ff0000 }", ());
    doc.dom.append_child(style, sheet);
    doc.dom.remove_attribute(path, "fill");
    let image = capture(
        "a_mutated_descendant_re_renders_on_the_next_layout",
        &mut doc,
    );
    assert!(is_close(pixel(&image, 16 + 36, 16 + 36), RED));
    doc.dom.set_text_node_data(sheet, "path { fill: #0000ff }");
    let image = capture(
        "a_mutated_descendant_re_renders_on_the_next_layout",
        &mut doc,
    );
    assert!(is_close(pixel(&image, 16 + 36, 16 + 36), BLUE));

    // A removed child is gone from the picture.
    doc.dom.remove_element(path);
    let image = capture(
        "a_mutated_descendant_re_renders_on_the_next_layout",
        &mut doc,
    );
    assert!(is_close(pixel(&image, 16 + 36, 16 + 36), WHITE));
}

/// `width` and `height` are presentational hints: author CSS overrides
/// them, and without them the natural size the document reports applies.
#[test]
fn the_size_attributes_are_hints_over_the_natural_size() {
    let mut doc = page();
    let root = doc.root;
    let svg = element(
        &mut doc,
        root,
        "svg",
        &[
            ("class", "c0"),
            ("viewBox", "0 0 40 20"),
            ("width", "80"),
            ("height", "80"),
        ],
    );
    element(
        &mut doc,
        svg,
        "rect",
        &[("width", "40"), ("height", "20"), ("fill", "#ff0000")],
    );
    doc.dom.render();
    assert_eq!(size(&doc, svg), (80.0, 80.0));

    doc.dom.remove_attribute(svg, "width");
    doc.dom.render();
    assert_eq!(
        size(&doc, svg),
        (160.0, 80.0),
        "the height and the viewBox ratio"
    );

    doc.dom.remove_attribute(svg, "height");
    doc.dom.render();
    assert_eq!(
        size(&doc, svg),
        (300.0, 150.0),
        "the viewBox ratio fitted into 300x150"
    );

    doc.dom.set_attribute(svg, "height", "2em");
    doc.dom.set_attribute(svg, "width", "not a length");
    doc.dom.render();
    assert_eq!(
        size(&doc, svg),
        (64.0, 32.0),
        "a CSS length is handed to the parser as written, an invalid one leaves no hint"
    );

    doc.dom.set_attribute(svg, "width", " 100. ");
    doc.dom.render();
    assert_eq!(
        size(&doc, svg),
        (100.0, 32.0),
        "a bare number is px, written from the parsed value"
    );

    doc.dom.set_inline_style(svg, "width: 20px; height: 30px");
    doc.dom.render();
    assert_eq!(size(&doc, svg), (20.0, 30.0), "author CSS wins");
}

/// A `viewBox` with a zero size disables rendering: the root lays out at its
/// size attributes and draws nothing, and the commit goes through.
#[test]
fn a_zero_size_view_box_draws_nothing() {
    let mut doc = page();
    let (svg, _) = red_square(&mut doc, "c0");
    doc.dom.set_attribute(svg, "viewBox", "0 0 0 0");
    let image = capture("a_zero_size_view_box_draws_nothing", &mut doc);
    assert_eq!(size(&doc, svg), (48.0, 48.0));
    assert!(is_close(pixel(&image, 16 + 24, 16 + 24), WHITE));
    assert!(doc.dom.committed_frame().is_some());
}

/// An `svg` inserted into another stops being a root: it gives up the
/// source it held as one, and its root's document draws it.
#[test]
fn a_nested_svg_belongs_to_its_root() {
    let mut doc = page();
    let root = doc.root;
    let outer = element(
        &mut doc,
        root,
        "svg",
        &[("class", "c0"), ("width", "48"), ("height", "48")],
    );
    let inner = doc.dom.create_element("svg", ());
    doc.dom.set_attribute(inner, "width", "48");
    doc.dom.set_attribute(inner, "height", "48");
    let rect = element(
        &mut doc,
        inner,
        "rect",
        &[("width", "48"), ("height", "48"), ("fill", "#0000ff")],
    );
    doc.dom.render();
    let detached = source(&doc, inner);
    assert!(
        doc.dom.knows_image_source(&detached),
        "a detached svg is a root of its own"
    );
    let first = source(&doc, outer);

    doc.dom.append_child(outer, inner);
    let image = capture("a_nested_svg_belongs_to_its_root", &mut doc);
    assert_eq!(doc.dom.image_source(inner, ImageRole::Source), None);
    assert!(!doc.dom.knows_image_source(&detached));
    assert_ne!(source(&doc, outer), first);
    assert!(is_close(pixel(&image, 16 + 24, 16 + 24), BLUE));

    let second = source(&doc, outer);
    doc.dom.set_attribute(rect, "fill", "#ff0000");
    let image = capture("a_nested_svg_belongs_to_its_root", &mut doc);
    assert_ne!(
        source(&doc, outer),
        second,
        "a nested mutation marks the root"
    );
    assert!(is_close(pixel(&image, 16 + 24, 16 + 24), RED));
}

/// The synthetic source is the document's own: no walk and no bind asks
/// the host for it, and nothing is read through `FrameImages`.
#[test]
fn the_host_is_never_asked_for_an_inline_source() {
    let mut doc = page();
    let (svg, _) = red_square(&mut doc, "c0");
    let images = TestImages::new();
    flashbulb::render_with_images(&mut doc.dom, &images);
    let image = screenshot::capture_prebuilt_document(
        "the_host_is_never_asked_for_an_inline_source",
        &mut doc.dom,
        &images,
    );
    assert!(is_close(pixel(&image, 16 + 24, 16 + 24), RED));
    let inline = source(&doc, svg);
    assert!(inline.starts_with("inline-svg:"), "{inline}");
    assert!(!images.was_asked_for(&inline));
    assert!(images.reads().is_empty(), "{:?}", images.reads());
}

/// Freeing a root forgets its source: the registry keeps no entry for a
/// node that no longer exists.
#[test]
fn a_freed_root_leaves_no_registry_entry() {
    let mut doc = page();
    let (svg, path) = red_square(&mut doc, "c0");
    doc.dom.render();
    let inline = source(&doc, svg);
    assert!(doc.dom.knows_image_source(&inline));

    doc.dom.remove_element(svg);
    doc.dom.drop_element(svg);
    doc.dom.drop_element(path);
    doc.dom.render();
    assert!(!doc.dom.knows_image_source(&inline));
}
