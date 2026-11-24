//! What the paint walk draws against what the image path draws for the
//! same markup: the comparison of two display lists' items, the image's
//! transforms mapped by a placement.

#![allow(clippy::float_cmp)]

use crate::kurbo::Affine;
use crate::paint::display_list::{DisplayList, Draw, Item as ListItem, Region, Shape};

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
