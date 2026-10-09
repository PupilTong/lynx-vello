//! Gradients: a `linearGradient` or `radialGradient` element, its `href`
//! chain folded in, as a specification a brush is made from against a
//! bounding box.

use std::str::FromStr;
use std::sync::Arc;

use svgtypes::{Length, LengthUnit};

use super::parse::Ids;
use super::style::{Axis, Viewport, parse_opacity, resolve_length, transform};
use crate::vello::kurbo::{Affine, Rect};
use crate::vello::peniko::color::DynamicColor;
use crate::vello::peniko::{Brush, Color, ColorStop, Extend, Gradient};

/// Which space a gradient's coordinates are in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Units {
    /// Fractions of the painted shape's bounding box (the default).
    ObjectBoundingBox,
    /// User units of the painted element.
    UserSpaceOnUse,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Geometry {
    Linear {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    Radial {
        cx: f64,
        cy: f64,
        r: f64,
        fx: f64,
        fy: f64,
        fr: f64,
    },
}

/// A gradient with every attribute resolved: coordinates in [`Units`],
/// stops with their opacity folded in, the spread method and the
/// `gradientTransform`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GradientSpec {
    pub(super) geometry: Geometry,
    pub(super) units: Units,
    pub(super) transform: Affine,
    pub(super) extend: Extend,
    /// Offsets in the unit interval, non-decreasing; colours with the
    /// stop's own opacity applied.
    pub(super) stops: Vec<(f32, Color)>,
}

/// How many `href` hops a chain is followed for before it is taken as a
/// cycle.
const MAX_CHAIN: usize = 16;

/// Builds the specification of the gradient element `node`, following its
/// `href` chain for missing attributes and stops. `None` for an element
/// that is not a gradient.
pub(super) fn gradient<'a>(
    node: roxmltree::Node<'a, 'a>,
    ids: &Ids<'a>,
    viewport: Viewport,
    font_size: f64,
) -> Option<Arc<GradientSpec>> {
    let name = node.tag_name().name();
    if name != "linearGradient" && name != "radialGradient" {
        return None;
    }
    // The chain, this element first; a hop to something that is not a
    // gradient, or back to a visited one, ends it.
    let mut chain = vec![node];
    while chain.len() < MAX_CHAIN {
        let last = chain[chain.len() - 1];
        let Some(target) = href(last, ids) else { break };
        let kind = target.tag_name().name();
        if (kind != "linearGradient" && kind != "radialGradient")
            || chain.iter().any(|seen| seen.id() == target.id())
        {
            break;
        }
        chain.push(target);
    }
    let attribute = |name: &str| chain.iter().find_map(|node| node.attribute(name));
    let units = match attribute("gradientUnits") {
        Some("userSpaceOnUse") => Units::UserSpaceOnUse,
        _ => Units::ObjectBoundingBox,
    };
    let coordinate = |name: &str, default: &str, axis: Axis| -> Option<f64> {
        let value = attribute(name).unwrap_or(default);
        let length = Length::from_str(value.trim()).ok()?;
        let resolved = match units {
            // A percentage is a fraction of the box; a number is one
            // already, and an absolute unit makes no sense in bounding-box
            // space, so its number is read as a fraction too, as browsers do.
            Units::ObjectBoundingBox => match length.unit {
                LengthUnit::Percent => length.number / 100.0,
                _ => length.number,
            },
            Units::UserSpaceOnUse => resolve_length(length, font_size, viewport, axis),
        };
        resolved.is_finite().then_some(resolved)
    };
    let geometry = if name == "linearGradient" {
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
    // Stops come from the first gradient in the chain that has any.
    let stops = chain
        .iter()
        .map(|node| stops(*node))
        .find(|stops| !stops.is_empty())
        .unwrap_or_default();
    Some(Arc::new(GradientSpec {
        geometry,
        units,
        transform: gradient_transform,
        extend,
        stops,
    }))
}

/// The element `node`'s `href` (or `xlink:href`) points at, when it is a
/// fragment reference to an element of the same document.
pub(super) fn href<'a>(
    node: roxmltree::Node<'a, 'a>,
    ids: &Ids<'a>,
) -> Option<roxmltree::Node<'a, 'a>> {
    let value = node
        .attribute("href")
        .or_else(|| node.attribute(("http://www.w3.org/1999/xlink", "href")))?;
    let id = value.trim().strip_prefix('#')?;
    ids.get(id).copied()
}

/// `node`'s `stop` children: offsets clamped to the unit interval and made
/// non-decreasing, colours with `stop-opacity` folded in.
///
/// A `stop-color: currentColor` is the stop's own `color`, which inherits
/// through the stop's own ancestors (the gradient, its `defs`, …), never
/// from an element the gradient paints: one gradient is one set of stops
/// for every shape that references it.
fn stops(node: roxmltree::Node<'_, '_>) -> Vec<(f32, Color)> {
    let mut stops: Vec<(f32, Color)> = Vec::new();
    let mut previous = 0.0_f32;
    for stop in node.children().filter(|child| child.has_tag_name("stop")) {
        let offset = stop
            .attribute("offset")
            .and_then(parse_opacity)
            .unwrap_or(0.0)
            .max(previous);
        previous = offset;
        let declared = super::style::declarations(stop);
        let stop_color = match declared.get("stop-color") {
            None | Some("inherit") => svgtypes::Color::black(),
            Some("currentColor") => inherited_color(stop),
            Some(value) => {
                svgtypes::Color::from_str(value).unwrap_or_else(|_| svgtypes::Color::black())
            }
        };
        let opacity = declared
            .get("stop-opacity")
            .and_then(parse_opacity)
            .unwrap_or(1.0);
        stops.push((offset, with_opacity(stop_color, opacity)));
    }
    stops
}

/// The computed `color` of `node`: the nearest of it and its ancestors that
/// declares a valid one, else the initial black. Read only for a stop whose
/// `stop-color` is `currentColor`. As in
/// [`Style::resolve`](super::style::Style::resolve), a value that does not
/// parse as a colour (`inherit`, `currentColor` included) leaves the
/// parent's.
fn inherited_color(node: roxmltree::Node<'_, '_>) -> svgtypes::Color {
    node.ancestors()
        .filter(roxmltree::Node::is_element)
        .find_map(|element| {
            super::style::declarations(element)
                .get("color")
                .and_then(|value| svgtypes::Color::from_str(value).ok())
        })
        .unwrap_or_else(svgtypes::Color::black)
}

/// `color` with its alpha multiplied by `opacity`, rounded to RGBA8 the
/// way every colour here is, so a stop's and a paint's opacity fold the
/// same whichever is applied first.
pub(super) fn fold_opacity(color: Color, opacity: f32) -> Color {
    let rgba = color.to_rgba8();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a unit-interval opacity scaled to 0..=255 and rounded"
    )]
    let alpha = (f32::from(rgba.a) * opacity).round() as u8;
    Color::from_rgba8(rgba.r, rgba.g, rgba.b, alpha)
}

/// `color` with its alpha multiplied by `opacity`, as RGBA8.
pub(super) fn with_opacity(color: svgtypes::Color, opacity: f32) -> Color {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a product of two unit-interval values scaled to 0..=255 and rounded"
    )]
    let alpha = (f32::from(color.alpha) / 255.0 * opacity * 255.0).round() as u8;
    Color::from_rgba8(color.red, color.green, color.blue, alpha)
}

/// The brush and brush transform that paints `spec` over a shape whose
/// bounding box is `bbox` (needed for [`Units::ObjectBoundingBox`]), with
/// the paint's `opacity` folded into every stop. `None` when the gradient
/// paints nothing: no stops, or bounding-box units over a shape with no
/// area, which the specification says disables the paint.
pub(super) fn brush(
    spec: &GradientSpec,
    bbox: Option<Rect>,
    opacity: f32,
) -> Option<(Brush, Option<Affine>)> {
    let (first, rest) = spec.stops.split_first()?;
    let faded = |color: Color| fold_opacity(color, opacity);
    let transform = match spec.units {
        Units::ObjectBoundingBox => {
            let bbox = bbox?;
            if !(bbox.width() > 0.0 && bbox.height() > 0.0) {
                return None;
            }
            Affine::translate((bbox.x0, bbox.y0))
                * Affine::scale_non_uniform(bbox.width(), bbox.height())
                * spec.transform
        }
        Units::UserSpaceOnUse => spec.transform,
    };
    if rest.is_empty() {
        return Some((Brush::Solid(faded(first.1)), None));
    }
    let gradient = match spec.geometry {
        Geometry::Linear { x1, y1, x2, y2 } => Gradient::new_linear((x1, y1), (x2, y2)),
        Geometry::Radial {
            cx,
            cy,
            r,
            fx,
            fy,
            fr,
        } => {
            if r <= 0.0 {
                // A zero-radius radial gradient is its last stop's colour.
                let last = spec.stops[spec.stops.len() - 1].1;
                return Some((Brush::Solid(faded(last)), None));
            }
            #[expect(clippy::cast_possible_truncation, reason = "peniko takes f32 radii")]
            let (fr, r) = (fr.max(0.0) as f32, r as f32);
            Gradient::new_two_point_radial((fx, fy), fr, (cx, cy), r)
        }
    };
    let stops: Vec<ColorStop> = spec
        .stops
        .iter()
        .map(|(offset, color)| ColorStop {
            offset: *offset,
            color: DynamicColor::from_alpha_color(faded(*color)),
        })
        .collect();
    Some((
        Brush::Gradient(
            gradient
                .with_extend(spec.extend)
                .with_stops(stops.as_slice()),
        ),
        Some(transform),
    ))
}
