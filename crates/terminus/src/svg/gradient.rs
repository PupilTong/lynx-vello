//! A `linearGradient` or `radialGradient` element of a cascaded SVG subtree
//! as the specification a brush is made from
//! ([`GradientSpec`]).
//!
//! The attributes (`gradientUnits`, the coordinates, `spreadMethod`,
//! `gradientTransform`) and the stops come from the first gradient along the
//! `href` chain that has them, followed for at most [`MAX_CHAIN`] hops
//! ([`spec`] reads the attributes). A stop's colour is its own computed
//! `stop-color`, `currentcolor` resolved against the stop's own computed
//! `color`, with its computed `stop-opacity` folded in.
//!
//! Each reference resolves the gradient anew, against the referencing
//! element's viewport and the gradient's own font size, so two referencers
//! in different viewports each get their own coordinates.

use std::str::FromStr;

use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use svgtypes::{Length, LengthUnit};

use super::attributes::{Axis, Viewport, href, parse_opacity, resolve_length, transform};
use super::paint_server::{Geometry, GradientSpec, Units, fold_opacity};
use crate::kurbo::Affine;
use crate::paint::convert::resolve_color;
use crate::peniko::{Color, Extend};
use crate::tree::document::{Document, NodeId};

pub(super) fn is_gradient(name: &str) -> bool {
    matches!(name, "linearGradient" | "radialGradient")
}

pub(super) fn gradient<T>(
    document: &Document<T>,
    ids: &FxHashMap<&str, NodeId>,
    id: NodeId,
    viewport: Viewport,
) -> Option<GradientSpec> {
    let node = document.get(id)?;
    let linear = node.tag_name() == Some("linearGradient");
    let mut chain: SmallVec<[NodeId; 4]> = SmallVec::new();
    chain.push(id);
    while chain.len() < MAX_CHAIN {
        let last = document.get(chain[chain.len() - 1])?;
        let Some(target) = href(last)
            .and_then(|href| href.trim().strip_prefix('#'))
            .and_then(|id| ids.get(id))
            .copied()
        else {
            break;
        };
        let is_gradient_target = document
            .get(target)
            .and_then(|node| node.tag_name())
            .is_some_and(is_gradient);
        if !is_gradient_target || chain.contains(&target) {
            break;
        }
        chain.push(target);
    }
    let attribute = |name: &str| {
        chain
            .iter()
            .find_map(|&link| document.get(link).and_then(|node| plain(node, name)))
    };
    let stops = chain
        .iter()
        .map(|&link| stops(document, link))
        .find(|stops| !stops.is_empty())
        .unwrap_or_default();
    let font_size = document
        .get(id)
        .and_then(|node| node.layout_computed_style())
        .map_or(16.0, |style| {
            f64::from(style.get_font().font_size.computed_size().px())
        });
    spec(linear, attribute, viewport, font_size, stops)
}

pub(super) fn plain<'a, T>(node: &'a crate::tree::node::Node<T>, name: &str) -> Option<&'a str> {
    match node.attribute(name)? {
        std::borrow::Cow::Borrowed(value) => Some(value),
        std::borrow::Cow::Owned(_) => None,
    }
}

fn stops<T>(document: &Document<T>, id: NodeId) -> Vec<(f32, Color)> {
    let Some(node) = document.get(id) else {
        return Vec::new();
    };
    let mut stops = Vec::new();
    let mut previous = 0.0_f32;
    for &child in node.child_ids() {
        let Some(stop) = document.get(child) else {
            continue;
        };
        if !stop.in_svg_namespace() || stop.tag_name() != Some("stop") {
            continue;
        }
        let offset = stop_offset(stop.attribute("offset").as_deref(), previous);
        previous = offset;
        let color = stop.layout_computed_style().map_or(Color::BLACK, |style| {
            let svg = style.get_svg();
            fold_opacity(resolve_color(style, &svg.stop_color), svg.stop_opacity)
        });
        stops.push((offset, color));
    }
    stops
}

pub(super) fn spec<'v>(
    linear: bool,
    attribute: impl Fn(&str) -> Option<&'v str>,
    viewport: Viewport,
    font_size: f64,
    stops: Vec<(f32, Color)>,
) -> Option<GradientSpec> {
    let units = match attribute("gradientUnits") {
        Some("userSpaceOnUse") => Units::UserSpaceOnUse,
        _ => Units::ObjectBoundingBox,
    };
    let coordinate = |name: &str, default: &str, axis: Axis| -> Option<f64> {
        let value = attribute(name).unwrap_or(default);
        let length = Length::from_str(value.trim()).ok()?;
        let resolved = match units {
            Units::ObjectBoundingBox => match length.unit {
                LengthUnit::Percent => length.number / 100.0,
                _ => length.number,
            },
            Units::UserSpaceOnUse => resolve_length(length, font_size, viewport, axis),
        };
        resolved.is_finite().then_some(resolved)
    };
    let geometry = if linear {
        Geometry::Linear {
            x1: coordinate("x1", "0%", Axis::Horizontal)?,
            y1: coordinate("y1", "0%", Axis::Vertical)?,
            x2: coordinate("x2", "100%", Axis::Horizontal)?,
            y2: coordinate("y2", "0%", Axis::Vertical)?,
        }
    } else {
        let cx = coordinate("cx", "50%", Axis::Horizontal)?;
        let cy = coordinate("cy", "50%", Axis::Vertical)?;
        Geometry::Radial {
            cx,
            cy,
            r: coordinate("r", "50%", Axis::Diagonal)?,
            fx: attribute("fx").map_or(Some(cx), |_| coordinate("fx", "", Axis::Horizontal))?,
            fy: attribute("fy").map_or(Some(cy), |_| coordinate("fy", "", Axis::Vertical))?,
            fr: coordinate("fr", "0%", Axis::Diagonal)?,
        }
    };
    let extend = match attribute("spreadMethod") {
        Some("reflect") => Extend::Reflect,
        Some("repeat") => Extend::Repeat,
        _ => Extend::Pad,
    };
    let gradient_transform = attribute("gradientTransform").map_or(Affine::IDENTITY, transform);
    Some(GradientSpec {
        geometry,
        units,
        transform: gradient_transform,
        extend,
        stops,
    })
}

pub(super) const MAX_CHAIN: usize = 16;

pub(super) fn fragment_id(href: &str) -> Option<&str> {
    href.trim().strip_prefix('#')
}

pub(super) fn stop_offset(value: Option<&str>, previous: f32) -> f32 {
    value.and_then(parse_opacity).unwrap_or(0.0).max(previous)
}
