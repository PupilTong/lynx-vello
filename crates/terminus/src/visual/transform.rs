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
use stylo::values::computed::transform::{
    Rotate, Scale, Transform as ComputedTransform, Translate,
};
use stylo::values::generics::transform::{
    create_perspective_matrix, get_normalized_vector_and_angle,
};

use crate::vello::kurbo::Affine;

/// Perspective applied to direct child stacking contexts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParentPerspective {
    pub depth: f32,
    pub center: Point2D<f32>,
}

impl ParentPerspective {
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

/// A stacking-context root's matrix around its transform list: every other
/// factor, all of which hold still between commits, so [`Self::with_list`]
/// folds any list — the committed one at build, a sampled one at compose —
/// in the same f32 order.
#[derive(Debug, Clone)]
pub(crate) struct ContextMatrix {
    /// The border box `%` in a list resolves against; `transform-box` is
    /// Gecko-only, so it is always this.
    reference: Rect<CSSPixelLength>,
    /// The transform origin, resolved against the border box.
    origin: Point2D<f32>,
    /// The factors applied after the list, in order: the motion-path
    /// sample, the individual `scale`, `rotate` and `translate`, the origin
    /// and the offset in the parent. Unfolded, so the committed world rounds
    /// exactly as it did before curves shared the fold.
    after: [Transform3D<f32>; 6],
    perspective: Option<Transform3D<f32>>,
    origin_z: f32,
}

impl ContextMatrix {
    pub(crate) fn of(
        style: &ComputedValues,
        border_box: Size2D<f32>,
        offset_in_parent: Point2D<f32>,
        parent_perspective: Option<ParentPerspective>,
    ) -> Self {
        let box_style = style.get_box();
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

        let offset = super::motion::offset_sample(style, border_box).map_or_else(
            Transform3D::identity,
            |sample| {
                Transform3D::rotation(0.0, 0.0, 1.0, euclid::Angle::radians(sample.angle)).then(
                    &Transform3D::translation(
                        sample.position.x - origin_x,
                        sample.position.y - origin_y,
                        0.0,
                    ),
                )
            },
        );
        Self {
            reference: Rect::new(
                Point2D::origin(),
                Size2D::new(
                    CSSPixelLength::new(border_box.width),
                    CSSPixelLength::new(border_box.height),
                ),
            ),
            origin: Point2D::new(origin_x, origin_y),
            after: [
                offset,
                individual_scale(&box_style.scale),
                individual_rotate(&box_style.rotate),
                individual_translate(&box_style.translate, border_box),
                Transform3D::translation(origin_x, origin_y, origin_z),
                Transform3D::translation(offset_in_parent.x, offset_in_parent.y, 0.0),
            ],
            perspective: parent_perspective.map(|perspective| perspective.matrix()),
            origin_z,
        }
    }

    /// The root's matrix with `list` as its transform, flattened.
    pub(crate) fn with_list(&self, list: &ComputedTransform) -> Transform3D<f32> {
        let mut matrix = Transform3D::translation(-self.origin.x, -self.origin.y, -self.origin_z)
            .then(&self.list_matrix(list).0);
        for factor in &self.after {
            matrix = matrix.then(factor);
        }
        if let Some(perspective) = &self.perspective {
            matrix = matrix.then(perspective);
        }
        flatten(matrix)
    }

    /// `list`'s own matrix against the border box, and whether it holds a 3D
    /// function.
    pub(crate) fn list_matrix(&self, list: &ComputedTransform) -> (Transform3D<f32>, bool) {
        list.to_transform_3d_matrix(Some(&self.reference))
            .expect("computed transform lists with a reference box always convert")
    }

    /// Whether some interpolation toward or away from `list` can leave the
    /// plane a 2D affine composes in: a list whose matrix carries a
    /// perspective term, or any 3D function under the parent's perspective.
    #[expect(
        clippy::float_cmp,
        reason = "exactly the entries `Transform3D::is_2d` requires of a 2D matrix"
    )]
    pub(crate) fn projective(&self, list: &ComputedTransform) -> bool {
        let (matrix, has_3d) = self.list_matrix(list);
        (self.perspective.is_some() && has_3d)
            || matrix.m14 != 0.0
            || matrix.m24 != 0.0
            || matrix.m34 != 0.0
            || matrix.m44 != 1.0
    }

    /// The border box.
    pub(crate) fn reference_size(&self) -> Size2D<f32> {
        Size2D::new(
            self.reference.size.width.px(),
            self.reference.size.height.px(),
        )
    }

    /// The translation to the transform origin, in CSS px.
    pub(crate) fn origin(&self) -> Affine {
        Affine::translate((f64::from(self.origin.x), f64::from(self.origin.y)))
    }
}

#[cfg(test)]
impl ContextMatrix {
    /// A root at the origin with a zero-sized box and no other factor.
    pub(crate) fn identity() -> Self {
        Self {
            reference: Rect::zero(),
            origin: Point2D::origin(),
            after: [Transform3D::identity(); 6],
            perspective: None,
            origin_z: 0.0,
        }
    }
}

/// The 2D affine part of `matrix`: exactly it when `matrix` is 2D, and the
/// part a planar composition reads otherwise.
pub(crate) fn planar(matrix: &Transform3D<f32>) -> Affine {
    Affine::new([
        f64::from(matrix.m11),
        f64::from(matrix.m12),
        f64::from(matrix.m21),
        f64::from(matrix.m22),
        f64::from(matrix.m41),
        f64::from(matrix.m42),
    ])
}

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
