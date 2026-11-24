//! The box of an SVG `svg` element.
//!
//! Every SVG-namespace `svg` element is a replaced element: its subtree is
//! drawn into its box rather than laid out, so it lays out as a leaf
//! (`layout::host` forces `DisplayMode::Leaf` on replaced content) and its
//! descendants generate no box. Its natural size follows its `width`,
//! `height` and `viewBox` attributes by the rules an SVG image's root
//! follows ([`vector_sizes`]: CSS Images 3 default sizing, 300 × 150 when
//! nothing is authored), kept in step on
//! creation and on every change of the three attributes through
//! `Document::set_natural_size`.
//!
//! `width` and `height` are also CSS properties of `svg` (presentation
//! attributes, [`super::presentation`]), so an authored size that is not an
//! absolute length (`width="100%"`) sizes the box through CSS and gives the
//! element no natural size on that axis.

use super::attributes::{absolute_length, vector_sizes, view_box};
use crate::layout::{NaturalSize, Size};
use crate::tree::document::{Document, NodeId};

/// Whether `attribute` of `element` is one an `svg` element's natural size
/// is computed from.
pub(crate) fn sizes_root(element: &str, attribute: &str) -> bool {
    element == super::SVG_TAG && matches!(attribute, "width" | "height" | "viewBox")
}

impl<T> Document<T> {
    /// Puts the natural size its attributes give on the SVG `svg` element
    /// `id`.
    pub(crate) fn update_svg_natural_size(&mut self, id: NodeId) {
        let node = self.live(id);
        let natural = natural_size(
            node.attribute("width").as_deref(),
            node.attribute("height").as_deref(),
            node.attribute("viewBox").as_deref(),
        );
        self.set_natural_size(id, natural);
    }
}

/// The natural size of an `svg` element with these attributes.
fn natural_size(
    width: Option<&str>,
    height: Option<&str>,
    view_box_value: Option<&str>,
) -> NaturalSize {
    let absolute_width = width.and_then(absolute_length);
    let absolute_height = height.and_then(absolute_length);
    let view_box = view_box_value
        .and_then(view_box)
        .map(|view_box| (view_box.w, view_box.h));
    let ((natural_width, natural_height), _viewport) =
        vector_sizes(absolute_width, absolute_height, view_box);
    // An axis authored with anything but an absolute length is CSS's.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a whole-px natural size from `vector_sizes`, far below f32's integer range"
    )]
    let axis = |authored: Option<&str>, absolute: Option<f32>, natural: u32| {
        (authored.is_none() || absolute.is_some()).then_some(natural as f32)
    };
    let width = axis(width, absolute_width, natural_width);
    let height = axis(height, absolute_height, natural_height);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a viewBox ratio fits an f32"
    )]
    let ratio = match view_box {
        Some((box_width, box_height)) => Some((box_width / box_height) as f32),
        None => width.zip(height).map(|(width, height)| width / height),
    };
    NaturalSize::new(Size::new(width, height), ratio)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::natural_size;
    use crate::layout::Size;

    #[test]
    fn natural_sizes() {
        let sized = natural_size(Some("24"), Some("24"), None);
        assert_eq!(sized.dimensions(), Size::new(Some(24.0), Some(24.0)));
        assert_eq!(sized.aspect_ratio(), Some(1.0));

        let defaulted = natural_size(None, None, None);
        assert_eq!(defaulted.dimensions(), Size::new(Some(300.0), Some(150.0)));

        let boxed = natural_size(None, None, Some("0 0 40 20"));
        assert_eq!(boxed.dimensions(), Size::new(Some(300.0), Some(150.0)));
        assert_eq!(boxed.aspect_ratio(), Some(2.0));

        let one_axis = natural_size(Some("60"), None, Some("0 0 40 20"));
        assert_eq!(one_axis.dimensions(), Size::new(Some(60.0), Some(30.0)));

        let percent = natural_size(Some("100%"), None, None);
        assert_eq!(percent.dimensions(), Size::new(None, Some(150.0)));
        assert_eq!(percent.aspect_ratio(), None);
    }
}
