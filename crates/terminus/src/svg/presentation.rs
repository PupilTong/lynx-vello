//! SVG presentation attributes (SVG 2 §6.6): attributes of an SVG element
//! that set a CSS property at the presentational-hint origin.
//!
//! After `set_attribute` or `remove_attribute` stores an SVG element's
//! attribute, [`Document::reflect_svg_attribute`] writes the property it maps
//! to into the element's presentational-hint block, or removes it when the
//! attribute is gone or its value does not parse — the attribute is then as
//! if absent. Three groups, as Gecko (`dom/svg/SVGElement.cpp`,
//! `IsNoNamespaceAttrMapped`, `UpdateMappedDeclarationBlock`) and Chromium
//! (`core/svg/svg_element.cc`, `CssPropertyIdForSVGAttributeName`, and the
//! animated properties with a presentation-attribute mapping) have them:
//!
//! - attributes named as their CSS property, on every SVG element ([`property_attribute`]);
//! - geometry attributes that are CSS properties on some elements only ([`geometry_attribute`]);
//! - attributes whose grammar is SVG's rather than CSS's: `transform` (an SVG transform list,
//!   folded into one matrix) and `d` on `path` (path data, kept up to its first error).
//!
//! Values of the first two groups parse as their property with unitless
//! lengths allowed (`ParsingMode::ALLOW_UNITLESS_LENGTH`), through the one
//! declaration parser the `style` attribute and
//! `Document::set_presentational_hint` use.

use stylo::properties::{PropertyDeclaration, PropertyDeclarationBlock, SourcePropertyDeclaration};
use stylo::servo_arc::Arc;
use stylo::shared_lock::Locked;
use stylo::values::generics::transform::{Matrix, Transform as GenericTransform};
use stylo::values::specified::svg_path::SVGPathData;
use stylo::values::specified::transform::TransformOperation;
use stylo::values::specified::{DProperty, Number};
use stylo::{LocalName, OwnedSlice};
use stylo_traits::ParsingMode;

use crate::style::engine::{NoStyleUpdate, PropertyUpdate, StyleEngine};
use crate::tree::document::{Document, NodeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mapping {
    Property(&'static str),
    Transform,
    PathData,
}

impl Mapping {
    const fn property(self) -> &'static str {
        match self {
            Self::Property(property) => property,
            Self::Transform => "transform",
            Self::PathData => "d",
        }
    }
}

const PROPERTY_ATTRIBUTES: [&str; 37] = [
    "clip-path",
    "clip-rule",
    "color",
    "cursor",
    "direction",
    "display",
    "fill",
    "fill-opacity",
    "fill-rule",
    "filter",
    "font-family",
    "font-size",
    "font-stretch",
    "font-style",
    "font-weight",
    "image-rendering",
    "letter-spacing",
    "mask",
    "opacity",
    "overflow",
    "paint-order",
    "pointer-events",
    "stop-color",
    "stop-opacity",
    "stroke",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-opacity",
    "stroke-width",
    "text-anchor",
    "text-decoration",
    "text-overflow",
    "transform-origin",
    "visibility",
];

fn property_attribute(attribute: &str) -> Option<&'static str> {
    PROPERTY_ATTRIBUTES
        .iter()
        .copied()
        .find(|property| *property == attribute)
}

fn geometry_attribute(element: &str, attribute: &str) -> Option<&'static str> {
    let properties: &[&'static str] = match element {
        "circle" => &["cx", "cy", "r"],
        "ellipse" => &["cx", "cy", "rx", "ry"],
        "rect" => &["x", "y", "width", "height", "rx", "ry"],
        "svg" => &["x", "y", "width", "height"],
        "use" => &["x", "y"],
        _ => &[],
    };
    properties
        .iter()
        .copied()
        .find(|property| *property == attribute)
}

fn mapping(element: &str, attribute: &str) -> Option<Mapping> {
    match attribute {
        "transform" => Some(Mapping::Transform),
        "d" if element == "path" => Some(Mapping::PathData),
        _ => property_attribute(attribute)
            .or_else(|| geometry_attribute(element, attribute))
            .map(Mapping::Property),
    }
}

impl<T> Document<T> {
    pub(crate) fn reflect_svg_attribute(&mut self, id: NodeId, name: &LocalName) {
        let node = self.live(id);
        debug_assert!(node.in_svg_namespace(), "only an SVG element reflects");
        let element = node.tag_name().expect("an element has a local name");
        let attribute: &str = name.0.as_ref();
        let sizes_root = super::root::sizes_root(element, attribute);
        let update = mapping(element, attribute).and_then(|mapping| {
            hint_update(
                self.style_engine(),
                node.presentational_hints.as_ref(),
                mapping,
                node.plain_attr(name),
            )
        });
        if let Some(update) = update {
            self.apply_presentational_hint_update(id, update);
        }
        if sizes_root {
            self.update_svg_natural_size(id);
        }
        self.note_svg_change(id);
    }
}

fn hint_update(
    engine: &StyleEngine,
    block: Option<&Arc<Locked<PropertyDeclarationBlock>>>,
    mapping: Mapping,
    value: Option<&str>,
) -> Option<PropertyUpdate> {
    if let Some(value) = value {
        let declaration = match mapping {
            Mapping::Property(property) => match engine.parse_style_update(
                block,
                property,
                value,
                ParsingMode::ALLOW_UNITLESS_LENGTH,
            ) {
                Ok(update) => return Some(update),
                Err(NoStyleUpdate::Unchanged) => return None,
                Err(NoStyleUpdate::Invalid) => None,
            },
            Mapping::Transform => transform(value),
            Mapping::PathData => path_data(value),
        };
        if let Some(declaration) = declaration {
            return engine
                .declaration_update(block, SourcePropertyDeclaration::with_one(declaration));
        }
    }
    engine
        .parse_style_update(block, mapping.property(), "", ParsingMode::DEFAULT)
        .ok()
}

fn transform(value: &str) -> Option<PropertyDeclaration> {
    if value.trim().is_empty() {
        return None;
    }
    let coefficients = super::attributes::transform_list(value)?.as_coeffs();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "CSS numbers are f32; a matrix entry past its range is past any drawing"
    )]
    let entry = |index: usize| Number::new(coefficients[index] as f32);
    let matrix = TransformOperation::Matrix(Matrix {
        a: entry(0),
        b: entry(1),
        c: entry(2),
        d: entry(3),
        e: entry(4),
        f: entry(5),
    });
    Some(PropertyDeclaration::Transform(GenericTransform(
        OwnedSlice::from(vec![matrix]),
    )))
}

fn path_data(value: &str) -> Option<PropertyDeclaration> {
    let (path, _complete) = SVGPathData::parse_bytes(value.as_bytes());
    (!path.commands().is_empty()).then(|| PropertyDeclaration::D(DProperty::Path(path)))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Mapping, PROPERTY_ATTRIBUTES, mapping};
    use crate::test_common::Doc;

    #[test]
    fn every_mapped_property_exists() {
        let doc = Doc::new();
        for property in PROPERTY_ATTRIBUTES
            .into_iter()
            .chain(["cx", "cy", "r", "rx", "ry", "x", "y", "width", "height"])
            .chain(["transform", "d"])
        {
            assert!(
                doc.dom.supports_style_property(property),
                "`{property}` is not a property of this build"
            );
        }
    }

    #[test]
    fn geometry_is_per_element() {
        assert_eq!(mapping("circle", "r"), Some(Mapping::Property("r")));
        assert_eq!(mapping("rect", "r"), None);
        assert_eq!(mapping("text", "x"), None);
        assert_eq!(mapping("line", "x1"), None);
        assert_eq!(mapping("svg", "viewBox"), None);
        assert_eq!(mapping("use", "width"), None);
        assert_eq!(mapping("g", "d"), None);
        assert_eq!(mapping("path", "d"), Some(Mapping::PathData));
        assert_eq!(mapping("g", "fill"), Some(Mapping::Property("fill")));
    }
}
