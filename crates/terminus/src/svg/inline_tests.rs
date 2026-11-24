//! An outermost `svg` element's recording as paint items of the ordinary
//! visual order (`visual::svg`), painted by the ordinary paint walk
//! (`paint::svg`): what the plan holds, what the walk draws against what the
//! image path draws for the same markup, where a root lands, what records
//! it again, and what a point inside it hits.

#![allow(clippy::float_cmp)]

use std::sync::Arc;

use euclid::default::Point2D;
use seldon::text::FontBlob;
use stylo::queries::values::PrefersColorScheme;

use super::image::{IsolatedSvg, VectorDocument};
use crate::kurbo::{Affine, Rect};
use crate::paint::compose::{CapturedShape, ComposeOp};
use crate::paint::display_list::{DisplayList, Draw, Item as ListItem, Region, Shape};
use crate::render::image::{DocumentKind, ImageEvent, NoImages};
use crate::render::picture::PictureKey;
use crate::test_common::{Doc, device_with};
use crate::tree::document::NodeId;
use crate::tree::xml::{XmlMarkup, XmlNamespace};
use crate::visual::{LayerKind, PaintItemKind};

const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

/// The page every inline root here sits on: positioned at (20, 30) on a
/// 400 × 300 viewport, its text in the family an SVG image's sheet gives
/// text with none of its own (`svg/ua.rs`), so the two paths shape alike.
const PAGE: &str = "page { display: flex; position: relative; width: 400px; height: 300px; }
     svg { display: flex; position: absolute; left: 20px; top: 30px; font-family: sans-serif; }";

/// A page at `ratio` device px per CSS px with Ahem registered, `css` after
/// [`PAGE`], holding `markup` as an inline root; answers the root.
fn page(markup: &str, ratio: f32, css: &str) -> (Doc, NodeId) {
    let mut doc = Doc::with_device(device_with(400.0, 300.0, ratio, PrefersColorScheme::Light));
    doc.add_css(&format!("{PAGE} {css}"));
    assert_eq!(doc.dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    doc.dom.add_svg_user_agent_stylesheet();
    let markup =
        XmlMarkup::parse(markup.as_bytes(), Some(XmlNamespace::Svg)).expect("the markup parses");
    let root = doc.dom.append_svg_markup(doc.root, &markup, || ());
    (doc, root)
}

fn by_id(doc: &Doc, id: &str) -> NodeId {
    doc.dom
        .query_selector(doc.root, &format!("#{id}"))
        .expect("parses")
        .expect("an element with that id")
}

/// The composed scene of `doc`'s committed frame.
fn scene(doc: &mut Doc) -> DisplayList {
    doc.dom.commit();
    doc.dom.scene(&NoImages).clone()
}

/// What the image path draws for `markup`: parsed, built as an isolated
/// document, recorded, shaped through `doc`'s own text context (as an image
/// applied to `doc` is), planned and painted with nothing culled.
fn image_list(doc: &mut Doc, markup: &str) -> DisplayList {
    let parsed = VectorDocument::parse(markup.as_bytes()).expect("the image parses");
    let image =
        IsolatedSvg::build(&parsed, 0.0).image(&mut doc.dom.layout_state_mut().text_context);
    DisplayList::clone(image.scene())
}

/// `scene` without what is not the root's recording: every clip that is
/// not a path (the root's content clip, which the walk pushes again inside
/// every group, and the page's own clips) and its pop. A recorded clip is
/// always a path, a recorded layer never a clip.
fn recorded_part(scene: &DisplayList) -> Vec<&ListItem> {
    let mut open = Vec::new();
    let mut kept = Vec::new();
    for item in scene.items() {
        match item {
            ListItem::PushClip { region, .. } => {
                let recorded = matches!(region.shape, Shape::Path { .. });
                open.push(recorded);
                if recorded {
                    kept.push(item);
                }
            }
            ListItem::PushLayer { .. } | ListItem::PushShadow(_) => {
                open.push(true);
                kept.push(item);
            }
            ListItem::Pop => {
                if open.pop().unwrap_or(true) {
                    kept.push(item);
                }
            }
            _ => kept.push(item),
        }
    }
    kept
}

/// The tolerance a coefficient of the walk's transform is held to against
/// the image's mapped by the root's placement: the walk's matrices are the
/// frame's f32 world matrices, so each coefficient carries one f32
/// rounding, about 6e-8 of its size; 1e-5 of it plus 1e-4 device px holds
/// that with room and is far below what a misplaced drawing shows.
fn close(walk: Affine, expected: Affine) -> bool {
    walk.as_coeffs()
        .iter()
        .zip(expected.as_coeffs())
        .all(|(a, b)| (a - b).abs() <= 1e-4 + 1e-5 * b.abs())
}

fn same_shape(a: &DisplayList, left: Shape, b: &DisplayList, right: Shape) -> bool {
    match (left, right) {
        (Shape::Rect(left), Shape::Rect(right)) => left == right,
        (
            Shape::RoundedRect { rect, radii },
            Shape::RoundedRect {
                rect: other,
                radii: other_radii,
            },
        ) => a.rounded(rect, radii) == b.rounded(other, other_radii),
        (Shape::Path { start, end }, Shape::Path { start: s, end: e }) => {
            a.path(start, end) == b.path(s, e)
        }
        _ => false,
    }
}

fn same_draw(
    walk: &DisplayList,
    left: &Draw,
    image: &DisplayList,
    right: &Draw,
    map: Affine,
) -> bool {
    left.rule == right.rule
        && close(
            walk.transform(left.transform),
            map * image.transform(right.transform),
        )
        && left.paint == right.paint
        && walk.paint_transform(left.paint_transform)
            == image.paint_transform(right.paint_transform)
        && same_shape(walk, left.shape, image, right.shape)
}

fn same_region(
    walk: &DisplayList,
    left: &Region,
    image: &DisplayList,
    right: &Region,
    map: Affine,
) -> bool {
    left.rule == right.rule
        && close(
            walk.transform(left.transform),
            map * image.transform(right.transform),
        )
        && same_shape(walk, left.shape, image, right.shape)
}

/// Whether item `left` of the walk's `walk` list draws what item `right` of
/// the image's `image` list draws, `map` (the root's placement in device
/// px) applied to the image's transforms.
pub(super) fn same_item(
    walk: &DisplayList,
    left: &ListItem,
    image: &DisplayList,
    right: &ListItem,
    map: Affine,
) -> bool {
    match (left, right) {
        (ListItem::Fill(left), ListItem::Fill(right)) => same_draw(walk, left, image, right, map),
        (ListItem::Stroke(left, style), ListItem::Stroke(right, other)) => {
            style == other && same_draw(walk, left, image, right, map)
        }
        (
            ListItem::PushClip {
                region,
                isolated,
                isolable,
            },
            ListItem::PushClip {
                region: other,
                isolated: other_isolated,
                isolable: other_isolable,
            },
        ) => {
            isolated == other_isolated
                && isolable == other_isolable
                && same_region(walk, region, image, other, map)
        }
        (
            ListItem::PushLayer {
                region,
                blend,
                alpha,
            },
            ListItem::PushLayer {
                region: other,
                blend: other_blend,
                alpha: other_alpha,
            },
        ) => {
            blend == other_blend
                && alpha == other_alpha
                && same_region(walk, region, image, other, map)
        }
        (ListItem::Pop, ListItem::Pop) => true,
        (ListItem::Glyphs(left), ListItem::Glyphs(right)) => {
            let (left, right) = (&walk.runs()[*left as usize], &image.runs()[*right as usize]);
            left.font == right.font
                && left.font_size == right.font_size
                && close(left.transform, map * right.transform)
                && left.glyph_transform == right.glyph_transform
                && left.paint == right.paint
                && left.paint_transform == right.paint_transform
                && left.style == right.style
                && left.hint == right.hint
                && left.embolden == right.embolden
                && walk.glyphs_of(left) == image.glyphs_of(right)
                && walk.coords_of(left) == image.coords_of(right)
        }
        _ => false,
    }
}

/// Compares the recorded part of an inline root's scene with the image's
/// list for the same markup, and answers the difference with both lists.
fn compare(markup: &str) -> Result<usize, String> {
    const RATIO: f32 = 2.0;
    let (mut doc, root) = page(markup, RATIO, "");
    let walk = scene(&mut doc);
    let image = image_list(&mut doc, markup);
    let placement = doc
        .dom
        .svg_recording(root)
        .and_then(|recording| doc.dom.svg_root_placement(root, recording))
        .map_or(Affine::IDENTITY, |placement| placement.map);
    let map = Affine::scale(f64::from(RATIO)) * Affine::translate((20.0, 30.0)) * placement;
    let recorded = recorded_part(&walk);
    let differs = recorded.len() != image.items().len()
        || recorded
            .iter()
            .zip(image.items())
            .any(|(left, right)| !same_item(&walk, left, &image, right, map));
    if differs {
        return Err(format!(
            "{markup}\nthe walk's recorded part ({} items): {recorded:#?}\nthe image's list ({} items): {:#?}",
            recorded.len(),
            image.items().len(),
            image.items()
        ));
    }
    Ok(recorded.len())
}

/// Every document of `svg/record_tests.rs` written as one literal: a body
/// goes into a 100 × 100 root, a full document stands as it is.
const RECORD_CORPUS: &[&str] = &[
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="#ff0000" fill-opacity="0.5"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M0 10 L90 10 L90 60" fill="none" stroke="#0000ff" stroke-width="4"
            stroke-linecap="round" stroke-linejoin="bevel" stroke-miterlimit="7"
            stroke-dasharray="5 3" stroke-dashoffset="2"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="none" stroke="#0000ff" stroke-dasharray="1 2 3"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="none" stroke="#0000ff" stroke-width="0"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="#ff0000" stroke="#0000ff" stroke-width="2"
            paint-order="stroke"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="#ff0000" visibility="hidden"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse">
              <rect width="2" height="2" fill="#ff0000"/></pattern></defs>
            <rect width="10" height="10" fill="url(#p)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g visibility="hidden">
              <rect width="10" height="10" fill="#ff0000"/>
              <rect width="10" height="10" fill="#0000ff" visibility="visible"/>
            </g>
            <g display="none"><rect width="10" height="10" fill="#ff0000"/></g></svg>"##,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect x="1" y="2" width="10" height="20" rx="3"/>
            <circle cx="5" cy="6" r="4"/>
            <ellipse cx="5" cy="6" rx="4"/>
            <line x1="0" y1="1" x2="2" y2="3"/>
            <polyline points="0,0 10,0 10,10"/>
            <polygon points="0 0 10 0 10 10"/></svg>"#,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect x="10%" y="1in" width="50%" height="1em" font-size="12" stroke="#000000" stroke-width="2em"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><radialGradient id="g" gradientUnits="userSpaceOnUse"
              cx="50" cy="50" r="40" fx="40" fy="45" fr="5" spreadMethod="repeat"
              gradientTransform="translate(3 4)">
              <stop offset="0" stop-color="#ff0000"/>
              <stop offset="1" stop-color="#0000ff"/>
            </radialGradient></defs>
            <rect width="100" height="100" fill="url(#g)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><linearGradient id="g" x1="0" y1="0" x2="100%" y2="1">
              <stop offset="0" stop-color="#ff0000"/>
              <stop offset="1" stop-color="#0000ff"/>
            </linearGradient></defs>
            <rect x="10" y="20" width="40" height="20" fill="url(#g)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/>
            <stop offset="1" stop-color="#0000ff"/></linearGradient></defs>
            <line x1="0" y1="5" x2="50" y2="5" fill="none" stroke="url(#g)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs>
              <linearGradient id="stops" spreadMethod="reflect">
                <stop offset="0" stop-color="#ff0000"/>
                <stop offset="1" stop-color="#0000ff"/>
              </linearGradient>
              <linearGradient id="mid" href="#stops" gradientUnits="userSpaceOnUse" x2="30"/>
              <linearGradient id="g" xlink:href="#mid" xmlns:xlink="http://www.w3.org/1999/xlink" y2="10"/>
            </defs>
            <rect width="100" height="20" fill="url(#g)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/></defs>
            <rect width="10" height="10" fill="url(#a)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000" stop-opacity="0.5"/></linearGradient></defs>
            <rect width="10" height="10" fill="url(#g)"/>
            <rect width="10" height="10" fill="url(#missing) #0000ff"/>
            <rect width="10" height="10" fill="url(#missing)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs color="#0000ff">
              <linearGradient id="defs"><stop offset="0" stop-color="currentColor"/></linearGradient>
              <linearGradient id="own" color="#ff0000"><stop offset="0" stop-color="currentColor"/></linearGradient>
            </defs>
            <rect width="10" height="10" color="#00ff00" fill="url(#defs)"/>
            <rect width="10" height="10" color="#ff00ff" fill="url(#defs)"/>
            <rect width="10" height="10" color="#0000ff" fill="url(#own)"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g opacity="0.5" transform="translate(5 6)">
              <rect x="10" y="10" width="20" height="20" fill="#ff0000"/>
            </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g opacity="0.5">
              <rect x="10" y="10" width="20" height="20" fill="none" stroke="#ff0000" stroke-width="4" stroke-linejoin="round"/>
            </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g mix-blend-mode="multiply" isolation="isolate">
              <rect width="20" height="20" fill="#ff0000"/>
            </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="20" height="20" fill="#ff0000" opacity="0.5"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs>
            <g filter="url(#f)"><rect width="20" height="20" fill="#ff0000"/></g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><mask id="m"><rect width="10" height="10" fill="#ffffff"/></mask></defs>
            <g mask="url(#m)"><rect width="20" height="20" fill="#ff0000"/></g>
            <rect y="30" width="20" height="20" fill="#0000ff"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><clipPath id="c" transform="translate(5 5)">
              <rect x="1" y="2" width="30" height="30" clip-rule="evenodd"/>
            </clipPath></defs>
            <g clip-path="url(#c)" transform="scale(2)">
              <rect width="40" height="40" fill="#ff0000"/>
            </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><clipPath id="c"><circle cx="20" cy="20" r="10"/></clipPath></defs>
            <g clip-path="url(#c)" opacity="0.25">
              <rect width="40" height="40" fill="#ff0000"/>
            </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs>
              <clipPath id="outer"><rect width="15" height="40"/></clipPath>
              <clipPath id="inner" clip-path="url(#outer)"><rect width="40" height="15"/></clipPath>
            </defs>
            <g clip-path="url(#inner)"><rect width="40" height="40" fill="#ff0000"/></g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><clipPath id="c" clipPathUnits="objectBoundingBox"><rect width="0.5" height="1"/></clipPath></defs>
            <g clip-path="url(#c)"><rect x="10" y="10" width="40" height="20" fill="#ff0000"/></g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs>
              <rect id="r" width="10" height="10"/>
              <symbol id="s" viewBox="0 0 10 10"><rect width="10" height="10" fill="#ff0000"/></symbol>
            </defs>
            <use href="#r" x="5" y="6" fill="#0000ff"/>
            <use href="#s" x="20" y="20" width="20" height="40"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g id="g"><use href="#g"/><rect width="10" height="10" fill="#ff0000"/></g>
            <use id="u" href="#u"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><svg x="10" y="10" width="40" height="20" viewBox="0 0 10 10" preserveAspectRatio="xMaxYMid meet">
              <rect width="10" height="10" fill="#ff0000"/>
            </svg></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><svg width="40" height="20" viewBox="0 0 10 10" preserveAspectRatio="none">
              <rect width="10" height="10" fill="#ff0000"/>
            </svg></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><switch>
              <rect systemLanguage="xx" width="10" height="10" fill="#ff0000"/>
              <rect requiredFeatures="f" width="10" height="10" fill="#ff0000"/>
              <rect width="10" height="10" fill="#0000ff"/>
              <rect width="10" height="10" fill="#ff0000"/>
            </switch></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><style>
              rect { fill: #0000ff; }
              .a { fill: #00ff00 !important; stroke: #ff0000; stroke-width: 2; }
              #b { fill: #00ff00 !important; display: none; }
            </style>
            <rect width="10" height="10"/>
            <rect class="a" width="10" height="10" fill="#ff0000"/>
            <rect id="b" width="10" height="10" fill="#0000ff"/>
            <g><rect class="a" width="10" height="10" fill="none"/></g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" style="fill: #0000ff"/>
            <rect width="10" height="10" fill="#ff0000"
                  style="fill: #0000ff !important; stroke: #0000ff; display: none"/>
            <g style="opacity: 0.5; mix-blend-mode: multiply">
              <rect width="10" height="10" fill="#ff0000"/>
            </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g fill="#ff0000" color="#0000ff" stroke="currentColor" stroke-width="0">
              <rect width="10" height="10" fill="inherit"/>
              <rect width="10" height="10" fill="currentColor"/>
              <rect width="10" height="10" fill="currentColor" color="#ff0000"/>
              <g opacity="0.5"><rect width="10" height="10" opacity="inherit"/></g>
            </g></svg>"##,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><image x="10" y="20" width="40" height="40"
              href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='10'%3E%3Crect width='10' height='10' fill='%23ff0000'/%3E%3C/svg%3E"/>
            <image x="0" y="0" width="10" height="10" href="/etc/hosts"/></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="10 20 100 50"><rect width="5" height="5"/></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><text x="10 99" y="20" font-size="10" font-family="Ahem, serif" font-weight="bold" letter-spacing="1">ab<tspan dx="2" dy="3" font-size="2em" font-weight="lighter">c</tspan><tspan x="50" y="60" text-anchor="middle" font-style="italic">d e</tspan></text></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><text>Label: <tspan x="100">value</tspan></text></svg>"#,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><text x="10" y="20" font-size="10" font-family="Ahem" fill="#ff0000">ab<tspan dx="2" dy="3">c</tspan><tspan x="50" y="60" text-anchor="middle">de</tspan><tspan x="80" y="90" text-anchor="end">fgh</tspan></text></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs>
            <text x="10" y="20" font-size="10" font-family="Ahem" fill="url(#g)" stroke="#0000ff" stroke-width="1">ab</text></svg>"##,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g opacity="0.5" transform="translate(5 5)"><text x="10" y="20" font-size="10" font-family="Ahem">ab</text></g></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><text x="0" y="10">dropped</text></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><text x="0" y="10" font-family="Ahem">a</text><rect width="1" height="1"/></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="currentColor"/><text>a</text></svg>"#,
];

/// The parametric documents of `svg/record_tests.rs`, one value each, and
/// the documents of `tests/svg_images.rs`.
const MORE_CORPUS: &[&str] = &[
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M0 0 H50 V50 H0 Z M10 10 H40 V40 H10 Z" fill-rule="evenodd" fill="#ff0000"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><linearGradient id="g" gradientUnits="userSpaceOnUse"
          x1="10" y1="0" x2="40" y2="0" spreadMethod="reflect">
          <stop offset="0" stop-color="#ff0000"/>
          <stop offset="1" stop-color="#0000ff" stop-opacity="0.5"/>
        </linearGradient></defs>
        <rect width="100" height="20" fill="url(#g)" fill-opacity="0.5"/></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><clipPath id="c">
          <rect width="10" height="10"/>
          <rect x="20" y="20" width="10" height="10" transform="rotate(10)"/>
        </clipPath></defs>
        <g clip-path="url(#c)" opacity="0.5">
          <rect width="40" height="40" fill="#ff0000"/>
        </g></svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="24" viewBox="0 0 48 24">
  <rect width="48" height="24" fill="#dbeafe"/>
  <circle cx="12" cy="12" r="8" fill="#dc2626"/>
  <path d="M26 20 L34 4 L42 20 Z" fill="#16a34a" stroke="#14532d" stroke-width="2" stroke-linejoin="round"/>
  <rect x="1" y="1" width="46" height="22" fill="none" stroke="#1f2937" stroke-width="2"/>
</svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#f59e0b"/>
      <stop offset="1" stop-color="#7c3aed"/>
    </linearGradient>
    <clipPath id="c"><rect x="2" y="2" width="16" height="16" rx="4"/></clipPath>
  </defs>
  <rect width="20" height="20" fill="url(#g)" clip-path="url(#c)"/>
  <circle cx="10" cy="10" r="5" fill="none" stroke="#ffffff" stroke-width="1.5" stroke-dasharray="2 1.5"/>
</svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">
  <path d="M20 2 L25 15 L38 15 L27 23 L31 37 L20 29 L9 37 L13 23 L2 15 L15 15 Z" fill="#000000"/>
  <circle cx="35" cy="5" r="4" fill="#000000" fill-opacity="0.5"/>
</svg>"##,
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
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
</svg>"##,
];

/// The one corpus document whose two lists differ. Its only shape lies
/// outside its `viewBox`, so outside the root's content clip: the walk's
/// cull leaves its item unencoded, since none of its ink can reach the
/// screen, while the image's walk, which culls nothing, draws it and the
/// texture's edge cuts it away. [`a_shape_outside_the_view_box_is_planned_and_culled`] pins that.
const CULLED: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="10 20 100 50"><rect width="5" height="5"/></svg>"#;

/// For every corpus document, the recorded part of the inline root's
/// scene is the image's list, item for item, under the root's placement
/// ([`close`] states the tolerance). [`CULLED`] is the one exception.
#[test]
fn an_inline_root_paints_what_the_image_of_its_markup_encodes() {
    let mut failures = Vec::new();
    let mut compared = 0;
    for markup in RECORD_CORPUS.iter().chain(MORE_CORPUS) {
        if *markup == CULLED {
            continue;
        }
        match compare(markup) {
            Ok(items) => compared += items,
            Err(difference) => failures.push(difference),
        }
    }
    assert!(
        compared > 100,
        "the corpus draws: {compared} items compared"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// [`CULLED`]'s shape is a planned item that the walk does not encode,
/// and the image's list draws it.
#[test]
fn a_shape_outside_the_view_box_is_planned_and_culled() {
    assert!(RECORD_CORPUS.contains(&CULLED));
    let difference = compare(CULLED).expect_err("the walk culls the shape");
    assert!(
        difference.contains("the walk's recorded part (0 items)")
            && difference.contains("the image's list (1 items)"),
        "{difference}"
    );
    let (mut doc, root) = page(CULLED, 1.0, "");
    let order = doc.dom.build_paint_order();
    let planned: Vec<_> = order
        .items()
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item.kind, PaintItemKind::Svg { .. }))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(planned.len(), 1, "the shape is planned");
    let encoded = crate::paint::walker::encoded_items(&doc.dom, &order);
    assert!(!encoded[planned[0]], "and not encoded");
    let at = order
        .items()
        .iter()
        .position(|item| item.node == root)
        .expect("the root's box");
    assert!(encoded[at], "the root's own box is");
}

/// Two shapes and a group with `opacity`: the root's box item, then one
/// item per shape in document order, and one group scope around the
/// group's two shapes.
#[test]
fn the_plan_appends_each_recorded_item_and_one_scope_per_group() {
    let (mut doc, root) = page(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
          <rect id="a" width="10" height="10" fill="#ff0000"/>
          <g id="g" opacity="0.5">
            <circle id="b" cx="30" cy="20" r="5" fill="#00ff00"/>
            <rect id="c" x="50" y="5" width="10" height="20" fill="#0000ff"/>
          </g>
        </svg>"##,
        1.0,
        "",
    );
    let order = doc.dom.build_paint_order();
    let (a, g, b, c) = (
        by_id(&doc, "a"),
        by_id(&doc, "g"),
        by_id(&doc, "b"),
        by_id(&doc, "c"),
    );
    let items: Vec<_> = order
        .items()
        .iter()
        .map(|item| (item.node, item.kind, item.hit_testable))
        .collect();
    let at = items
        .iter()
        .position(|(node, kind, _)| *node == root && *kind == PaintItemKind::ElementBox)
        .expect("the root's box item");
    let svg = |index| PaintItemKind::Svg { root, index };
    assert_eq!(
        items[at + 1..],
        [(a, svg(0), false), (b, svg(2), false), (c, svg(3), false)],
        "{items:?}"
    );
    let layers = order.layers();
    assert_eq!(layers.len(), 1, "{layers:?}");
    assert_eq!(layers[0].node, g);
    assert_eq!(layers[0].kind, LayerKind::Svg { root, index: 1 });
    assert_eq!(layers[0].items, at + 2..at + 4);
    // The first rect's item is its box on the page: the root at (20, 30).
    let first = order.matrix(order.items()[at + 1].transform);
    assert_eq!((first.m41, first.m42), (20.0, 30.0));
    assert_eq!(
        order.items()[at + 1].size,
        euclid::default::Size2D::new(10.0, 10.0)
    );
}

/// A page with no `svg` element plans exactly what it did before: one item
/// per box and one layer per group.
#[test]
fn a_page_without_svg_plans_as_before() {
    let mut doc = Doc::with_css(
        "page { display: flex; width: 400px; height: 300px; }
         view { display: flex; width: 20px; height: 20px; }
         .faded { opacity: 0.5; }",
    );
    let root = doc.root;
    doc.el(root, "view");
    let faded = doc.el(root, "view.faded");
    doc.el(faded, "view");
    assert!(!doc.dom.has_svg_roots());
    let order = doc.dom.build_paint_order();
    assert_eq!(order.items().len(), 4, "{:?}", order.items());
    assert_eq!(order.layers().len(), 1);
    assert_eq!(order.layers()[0].kind, LayerKind::Element);
    assert_eq!(order.clips().len(), 0);
}

/// `clip-path: url()` on a group is a clip scope shaped by the recorded
/// path, a program push of that path around the group's items.
#[test]
fn a_clip_path_on_a_group_is_a_clip_scope_of_its_path() {
    let (mut doc, root) = page(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
          <defs><clipPath id="c"><circle cx="20" cy="20" r="10"/></clipPath></defs>
          <g id="g" clip-path="url(#c)"><rect width="40" height="40" fill="#ff0000"/></g>
        </svg>"##,
        1.0,
        "",
    );
    let frame = doc.dom.commit();
    let layers = frame.order.layers();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].node, by_id(&doc, "g"));
    assert!(matches!(layers[0].kind, LayerKind::Svg { root: at, .. } if at == root));
    let paths: Vec<_> = frame
        .presentation
        .program
        .iter()
        .filter_map(|op| match op {
            ComposeOp::Push {
                clip_only: true,
                shape: CapturedShape::Path(path),
                transform,
                ..
            } => Some((Arc::clone(path), *transform)),
            _ => None,
        })
        .collect();
    assert_eq!(paths.len(), 1, "one recorded clip push");
    let (path, transform) = &paths[0];
    let bounds = transform.transform_rect_bbox(crate::kurbo::Shape::bounding_box(&**path));
    // The circle's box, on the page at the root's place.
    let expected = Rect::new(30.0, 40.0, 50.0, 60.0);
    assert!(
        (bounds.x0 - expected.x0).abs() < 1e-3
            && (bounds.y0 - expected.y0).abs() < 1e-3
            && (bounds.x1 - expected.x1).abs() < 1e-3
            && (bounds.y1 - expected.y1).abs() < 1e-3,
        "{bounds:?}"
    );
    assert!(!frame.draws_vectors());
}

/// Text in a root draws its glyph run, and the bounds layer around it
/// takes the text's box, which only shaping knows.
#[test]
fn text_in_a_root_draws_its_run_inside_a_layer_that_holds_it() {
    let (mut doc, _) = page(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><g opacity="0.5" transform="translate(5 5)"><text x="10" y="20" font-size="10" font-family="Ahem">ab</text></g></svg>"#,
        1.0,
        "",
    );
    let scene = scene(&mut doc);
    let recorded = recorded_part(&scene);
    let [
        ListItem::PushLayer { region, alpha, .. },
        ListItem::Glyphs(run),
        ListItem::Pop,
    ] = recorded.as_slice()
    else {
        panic!("a layer around one glyph run: {recorded:?}");
    };
    assert_eq!(*alpha, 0.5);
    assert_eq!(scene.glyphs_of(&scene.runs()[*run as usize]).len(), 2);
    let Shape::Rect(rect) = region.shape else {
        panic!("a bounds layer: {region:?}");
    };
    // Ahem's em box: "ab" at 10 px from (10, 20), 8 px over the baseline
    // and 2 under, in the group's coordinates.
    assert_eq!(rect, Rect::new(10.0, 12.0, 30.0, 22.0));
    let on_page = scene.transform(region.transform).transform_rect_bbox(rect);
    assert_eq!(on_page, Rect::new(35.0, 47.0, 55.0, 57.0));
}

/// A root whose box differs from its `viewBox` in ratio lands its drawing
/// where an `<image>` of the same markup, at the same place and size,
/// lands its picture.
#[test]
fn a_view_box_root_lands_where_the_image_did() {
    const MARKUP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40" viewBox="0 0 10 10" preserveAspectRatio="xMaxYMid meet"><rect width="10" height="10" fill="#ff0000"/></svg>"##;
    const CSS: &str = "svg, image { width: 90px; height: 50px; padding: 3px; }
                       image { display: flex; position: absolute; left: 20px; top: 30px; }";
    let (mut doc, _) = page(MARKUP, 2.0, CSS);
    let walk = scene(&mut doc);
    let recorded = recorded_part(&walk);
    let [ListItem::Fill(fill)] = recorded.as_slice() else {
        panic!("one fill: {recorded:?}");
    };
    let inline = walk
        .transform(fill.transform)
        .transform_rect_bbox(Rect::new(0.0, 0.0, 10.0, 10.0));

    let mut image = Doc::with_device(device_with(400.0, 300.0, 2.0, PrefersColorScheme::Light));
    image.add_css(&format!("{PAGE} {CSS}"));
    let node = image.el(image.root, "image");
    let source = "app:///box.svg";
    let event = ImageEvent::parse_document(Arc::from(source), MARKUP.as_bytes(), DocumentKind::Svg);
    image.dom.apply_image_events(&[event]);
    let natural = image.dom.request_image(source).natural_size();
    image
        .dom
        .set_picture(node, Some((PictureKey::Url(Arc::from(source)), natural)));
    let frame = image.dom.commit();
    let [draw] = frame.vector_draws() else {
        panic!("one vector draw");
    };
    let placed = draw.transform
        * Affine::translate(draw.anchor.to_vec2())
        * draw.placement(draw.extent, draw.extent);
    let expected = placed.transform_rect_bbox(Rect::new(0.0, 0.0, 10.0, 10.0));
    assert!(
        [
            (inline.x0, expected.x0),
            (inline.y0, expected.y0),
            (inline.x1, expected.x1),
            (inline.y1, expected.y1)
        ]
        .iter()
        .all(|(a, b)| (a - b).abs() < 1e-3),
        "{inline:?} against {expected:?}"
    );
    // `xMaxYMid meet` in the 90 × 50 content box at (23, 33): a 50 px
    // square at its right edge, in device px.
    assert_eq!(expected, Rect::new(126.0, 66.0, 226.0, 166.0));
}

/// A shape's attribute change records the root again and the next frame
/// draws the change.
#[test]
fn an_attribute_change_records_again_and_the_next_list_differs() {
    let (mut doc, _) = page(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect id="r" width="10" height="10" fill="#ff0000"/></svg>"##,
        1.0,
        "",
    );
    let before = scene(&mut doc);
    let rect = by_id(&doc, "r");
    doc.dom.set_attribute(rect, "width", "30");
    assert!(doc.dom.needs_render());
    let after = scene(&mut doc);
    assert!(crate::paint::equivalence::compare_scenes(&before, &after).is_err());
    assert!(crate::paint::equivalence::compare_scenes(&after, &scene(&mut doc)).is_ok());
}

/// A point over a shape and a point over empty space inside the root both
/// answer the root: the recorded items are not hit-testable.
#[test]
fn points_inside_the_root_hit_the_root() {
    let (mut doc, root) = page(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="10" height="10" fill="#ff0000"/></svg>"##,
        1.0,
        "",
    );
    doc.dom.commit();
    for point in [Point2D::new(25.0, 35.0), Point2D::new(90.0, 100.0)] {
        assert_eq!(
            doc.dom.elements_from_point(point).first(),
            Some(&root),
            "{point:?}"
        );
    }
    assert_ne!(
        doc.dom
            .elements_from_point(Point2D::new(300.0, 250.0))
            .first(),
        Some(&root)
    );
}

/// A frame whose only picture is an inline root has no vector op.
#[test]
fn an_inline_root_makes_no_vector_draw() {
    let (mut doc, _) = page(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><circle cx="50" cy="50" r="40" fill="#ff0000"/></svg>"##,
        1.0,
        "",
    );
    let frame = doc.dom.commit();
    assert!(!frame.draws_vectors());
    assert!(
        frame
            .presentation
            .program
            .iter()
            .all(|op| !matches!(op, ComposeOp::Vector { .. }))
    );
    assert!(
        frame
            .order
            .items()
            .iter()
            .any(|item| matches!(item.kind, PaintItemKind::Svg { .. }))
    );
}
