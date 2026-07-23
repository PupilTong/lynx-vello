//! Matrix construction for stacking-context roots.
//!
//! Every matrix here is in euclid's row-vector convention (`a.then(&b)`
//! applies `a` first), matching how stylo composes transform lists. Because
//! `transform-style` is always `Flat` in this build, each stacking context's
//! accumulated matrix is flattened before descendants compose onto it, so
//! world matrices stay 2D-projective (z fully decoupled) even when 3D
//! functions (`matrix3d`, `translateZ`, `rotateX`…) appear in a list.

use euclid::default::{Point2D, Rect, Size2D, Transform3D};
use stylo::properties::ComputedValues;
use stylo::values::computed::CSSPixelLength;
use stylo::values::computed::transform::{Rotate, Scale, Translate};
use stylo::values::generics::transform::{
    create_perspective_matrix, get_normalized_vector_and_angle,
};

/// The perspective a parent stacking context applies to its **direct
/// box-tree children**'s matrices (display:contents levels dissolve): `T(−center) ·
/// perspective(depth) · T(center)` in the parent's border-box space. `perspective-origin` is not
/// compiled in this build, so the center is the initial `50% 50%`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParentPerspective {
    pub depth: f32,
    pub center: Point2D<f32>,
}

impl ParentPerspective {
    /// Reads the perspective a style applies to its children, if any.
    pub(crate) fn of(style: &ComputedValues, border_box: Size2D<f32>) -> Option<Self> {
        match style.get_box().perspective {
            stylo::values::generics::box_::Perspective::None => None,
            stylo::values::generics::box_::Perspective::Length(ref depth) => Some(Self {
                depth: depth.0.px(),
                center: Point2D::new(border_box.width / 2.0, border_box.height / 2.0),
            }),
        }
    }

    fn matrix(&self) -> Transform3D<f32> {
        Transform3D::translation(-self.center.x, -self.center.y, 0.0)
            .then(&create_perspective_matrix(self.depth))
            .then(&Transform3D::translation(self.center.x, self.center.y, 0.0))
    }
}

/// Projects a matrix onto the z = 0 plane: keeps the x/y/w behavior for
/// z = 0 inputs and decouples z entirely. Correct here because every element
/// flattens (`transform-style` is always `Flat`).
pub(crate) fn flatten(mut matrix: Transform3D<f32>) -> Transform3D<f32> {
    matrix.m13 = 0.0;
    matrix.m23 = 0.0;
    matrix.m43 = 0.0;
    matrix.m31 = 0.0;
    matrix.m32 = 0.0;
    matrix.m34 = 0.0;
    matrix.m33 = 1.0;
    matrix
}

/// A stacking-context root's flattened matrix mapping its border-box-local
/// coordinates into its parent stacking context's space.
///
/// Composition per css-transforms-1 §"Transform Rendering" and
/// css-transforms-2 §6, spelled in `.then` order (first applied first):
/// `T(−origin) → transform list → scale → rotate → translate → T(origin)
///  → T(offset in parent) → parent perspective`.
/// Percentages (translate functions, transform-origin) resolve against the
/// border box.
pub(crate) fn stacking_context_matrix(
    style: &ComputedValues,
    border_box: Size2D<f32>,
    offset_in_parent: Point2D<f32>,
    parent_perspective: Option<ParentPerspective>,
) -> Transform3D<f32> {
    let box_style = style.get_box();

    let reference = Rect::new(
        euclid::default::Point2D::origin(),
        Size2D::new(
            CSSPixelLength::new(border_box.width),
            CSSPixelLength::new(border_box.height),
        ),
    );
    let (list, _has_3d) = box_style
        .transform
        .to_transform_3d_matrix(Some(&reference))
        .expect("computed transform lists with a reference box always convert");

    let origin = &box_style.transform_origin;
    let origin_x = origin
        .horizontal
        .resolve(CSSPixelLength::new(border_box.width))
        .px();
    let origin_y = origin
        .vertical
        .resolve(CSSPixelLength::new(border_box.height))
        .px();
    let origin_z = origin.depth.px();

    let mut matrix = Transform3D::translation(-origin_x, -origin_y, -origin_z)
        .then(&list)
        .then(&individual_scale(&box_style.scale))
        .then(&individual_rotate(&box_style.rotate))
        .then(&individual_translate(&box_style.translate, border_box))
        .then(&Transform3D::translation(origin_x, origin_y, origin_z))
        .then(&Transform3D::translation(
            offset_in_parent.x,
            offset_in_parent.y,
            0.0,
        ));
    if let Some(perspective) = parent_perspective {
        matrix = matrix.then(&perspective.matrix());
    }
    flatten(matrix)
}

// The individual transform properties are storage-only in the fork (always
// `None` until a grammar rebase exposes them); the arms mirror stylo's own
// `ToMatrix` conventions so they are correct the day that happens.

fn individual_translate(translate: &Translate, border_box: Size2D<f32>) -> Transform3D<f32> {
    match translate {
        Translate::None => Transform3D::identity(),
        Translate::Translate(x, y, z) => Transform3D::translation(
            x.resolve(CSSPixelLength::new(border_box.width)).px(),
            y.resolve(CSSPixelLength::new(border_box.height)).px(),
            z.px(),
        ),
    }
}

fn individual_rotate(rotate: &Rotate) -> Transform3D<f32> {
    match *rotate {
        Rotate::None => Transform3D::identity(),
        Rotate::Rotate(angle) => {
            Transform3D::rotation(0.0, 0.0, 1.0, euclid::Angle::radians(angle.radians()))
        }
        Rotate::Rotate3D(x, y, z, angle) => {
            let (x, y, z, radians) = get_normalized_vector_and_angle(x, y, z, angle.radians());
            Transform3D::rotation(x, y, z, euclid::Angle::radians(radians))
        }
    }
}

fn individual_scale(scale: &Scale) -> Transform3D<f32> {
    match *scale {
        Scale::None => Transform3D::identity(),
        Scale::Scale(x, y, z) => Transform3D::scale(x, y, z),
    }
}
