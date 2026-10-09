//! SVG documents as vector images: the engine's own converter.
//!
//! One converter serves every SVG the engine draws, whether a host reported
//! its bytes (`<image src="x.svg">`, CSS `url(x.svg)`) or a page wrote the
//! markup itself (`docs/svg-lynx-component-design.md`, contract C). It has
//! two halves that run on different threads:
//!
//! - [`parse`] turns the bytes into a [`VectorDocument`]: one `roxmltree` parse, one walk, and a
//!   flat list of [`Item`]s in paint order, in viewport units, every transform absolute and every
//!   paint a final peniko brush. Geometry is resolved here, so the parse needs no document state
//!   and can run on a blocking pool. `Send + Sync`.
//! - [`encode`] turns a document into a [`Scene`](crate::vello::Scene) on the document thread.
//!   [`opens_blend`] is the vello #1198 test the layer around a whole-image draw needs.
//!
//! # The supported subset
//!
//! Elements: `svg` (root and nested: `x`, `y`, `width`, `height`, `viewBox`,
//! `preserveAspectRatio`), `g`, `a` (as `g`), `defs`, `symbol`, `use`
//! (`href`/`xlink:href`, `x`, `y`, `width`/`height` for a `symbol` or `svg`
//! target, recursion refused), `path`, `rect` (`rx`/`ry`), `circle`,
//! `ellipse`, `line`, `polyline`, `polygon`, `clipPath` (`clipPathUnits`,
//! nested `clip-path`), `linearGradient`, `radialGradient`, `stop`,
//! `switch` (its first child with no `systemLanguage`, `requiredFeatures` or
//! `requiredExtensions`). `text` (for now, below), `image`, `mask`,
//! `filter`, `pattern`, `marker`, `style`, `title`, `desc`, `metadata` and
//! every unknown element produce nothing.
//!
//! Properties, each from its presentation attribute only: `fill`,
//! `fill-opacity`, `fill-rule`, `stroke`, `stroke-width`, `stroke-opacity`,
//! `stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit`,
//! `stroke-dasharray`, `stroke-dashoffset`, `paint-order`, `color`,
//! `display`, `visibility`, `opacity`, `mix-blend-mode`, `isolation`,
//! `clip-path`, `clip-rule`, `transform`, `font-size`, and
//! `stop-color`/`stop-opacity` on a `stop`. CSS inside the document is not
//! read, by ruling (`docs/svg-lynx-component-design.md`, "CSS inside SVG"):
//! a `style` element's text and a `style` attribute change nothing, as in
//! native Lynx, whose SVG renderer has no `style` element. Lengths take
//! user units, `px`, `%` (of the viewport width or height, or of its
//! normalised diagonal for `r` and `stroke-width`), `pt`, `pc`, `mm`, `cm`,
//! `in`, and `em`/`ex` of the element's own font size (`ex` is half an em).
//! A malformed path renders up to its first error, as the SVG specification
//! asks.
//!
//! Nesting is bounded: the walk goes 256 levels deep at most, counting an
//! element inside another and a `use` expanding its target each as one
//! level, and skips whatever lies below with its subtree. Markup nested
//! deeper than that is removed before `roxmltree` parses it, and a document
//! whose entities' replacement text could nest markup past the bound is
//! refused. Both the walk and `roxmltree` recurse once per level, and
//! natively the parse runs on a blocking pool thread with a 2 MiB stack.
//!
//! # Inheritance
//!
//! SVG's: every property above except `display`, `opacity`,
//! `mix-blend-mode`, `isolation`, `clip-path`, `transform` and the `stop-*`
//! pair inherits its *computed* value, so an inherited `stroke-width: 1em`
//! is the parent's px and an inherited `fill: currentColor` is still the
//! keyword, resolved against each element's own `color`. `inherit` takes
//! the parent's value for every property, inherited or not. A `use`'s
//! target inherits from the `use`, not from where it sits. The initial
//! values are the specification's (`fill: black`, `stroke: none`,
//! `stroke-width: 1`, `font-size: 16`, …).
//!
//! # Groups, layers and clips
//!
//! The drawing rules of the former `usvg` walker are kept:
//!
//! - An element with `opacity` below one or a non-`normal` `mix-blend-mode` draws inside one
//!   compositing layer. Its shape is the clip path when the element's `clip-path` has exactly one
//!   shape child, otherwise the bounding box of what it draws (strokes included), in the element's
//!   own coordinates. A clip with opacity one and a normal blend is a clip layer alone. `isolation:
//!   isolate` by itself pushes nothing, and `filter` is ignored: the content draws unfiltered.
//! - A `clipPath` with one shape child clips with that shape under its `clip-rule`; with several,
//!   with the concatenation of every child under `nonzero`, an approximation of the union that
//!   differs where children overlap with opposite winding. A `clipPath` that is itself clipped
//!   pushes that clip first. `clipPathUnits="objectBoundingBox"` scales the clip by the clipped
//!   element's bounding box. A `text` child of a `clipPath` contributes nothing (glyph outlines are
//!   not available at parse).
//! - An element with a `mask` is skipped with its whole subtree: drawing it unmasked would show
//!   what the author hid.
//! - A blend layer never opens directly inside a clip layer (vello [#1198](https://github.com/linebender/vello/issues/1198)):
//!   where a clip layer would enclose a non-`normal` blend with no layer between them, the
//!   innermost clip layer is a full `Normal` layer instead ([`Item::PushClip`]'s `isolate`), and
//!   [`opens_blend`] answers the same question for the layer a draw of the whole image opens.
//! - A nested `svg` and a `symbol` clip to their viewport (`overflow: hidden`) and map their
//!   `viewBox` by their `preserveAspectRatio`. The root's `preserveAspectRatio` is not applied
//!   here: it is carried as [`VectorDocument::aspect`] for the painter, which maps the viewport
//!   onto the destination box.
//!
//! # Paint servers
//!
//! Solid colours fold the paint opacity into RGBA8. A stop's
//! `currentColor` is the stop's own `color`, inherited through the
//! gradient's ancestors, whichever element the gradient paints. Gradients
//! inherit attributes and stops along their `href` chain, map `spreadMethod` to
//! [`Extend`](crate::vello::peniko::Extend), take `gradientTransform` as the
//! brush transform, and resolve `objectBoundingBox` units against the shape's
//! `kurbo` bounding box;
//! `userSpaceOnUse` percentages resolve against the viewport. A radial
//! gradient's focal circle is peniko's start circle and its outer circle the
//! end circle. A gradient with one stop is that colour, with none paints
//! nothing. A `pattern` paints nothing, and an unresolvable `url()` paints
//! its fallback or nothing.
//!
//! # Text
//!
//! Not drawn yet: a `text` element and its `tspan`s produce nothing, as an
//! `image` does. Text lands in the next change, shaped on the document
//! thread through the document's own text context.

mod encode;
mod nesting;
mod paint_server;
mod parse;
mod shapes;
mod style;

pub(crate) use encode::{encode, opens_blend};
pub(crate) use parse::parse;

use crate::render::image::AspectRatio;
use crate::vello::kurbo::{Affine, BezPath, Rect, Stroke};
use crate::vello::peniko::{BlendMode, Brush, Fill};

/// A parsed SVG document, ready to encode: its sizes and a flat command
/// list in viewport units.
///
/// `pub` only because [`ImageEvent::ParsedDocument`](crate::ImageEvent)
/// carries it from the parsing thread to the document's; no field or
/// method is public, and no crate outside `dom` reads it.
#[derive(Clone, Debug)]
pub struct VectorDocument {
    /// The size layout is told, in whole CSS px (CSS Images 3 default
    /// sizing over the root's `width`, `height` and `viewBox`).
    pub(crate) natural: (u32, u32),
    /// The rectangle the items are drawn in: the `viewBox` size when the
    /// root has one, else the natural size.
    pub(crate) viewport: (f32, f32),
    /// The root's `preserveAspectRatio`.
    pub(crate) aspect: AspectRatio,
    pub(crate) items: Vec<Item>,
}

/// One drawing command. Transforms are absolute (viewport space); shapes
/// are in the coordinates the transform maps from.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a path carries its brushes inline: a document is a few hundred items, read once to encode"
)]
pub(crate) enum Item {
    Path {
        shape: BezPath,
        /// The shape's bounding box in its own coordinates, computed once:
        /// what bounding-box paint units and enclosing layers read.
        bounds: Rect,
        transform: Affine,
        fill: Option<FillPaint>,
        stroke: Option<StrokePaint>,
        /// `paint-order`: the fill before the stroke.
        fill_first: bool,
    },
    /// A compositing layer, popped by a matching [`Item::Pop`].
    PushLayer {
        blend: BlendMode,
        alpha: f32,
        clip: LayerClip,
        transform: Affine,
    },
    /// A clip layer, popped by a matching [`Item::Pop`]. `isolate` makes
    /// it a full `Normal` layer, per the vello #1198 rule in the module
    /// docs.
    PushClip {
        shape: BezPath,
        rule: Fill,
        transform: Affine,
        isolate: bool,
    },
    Pop,
}

/// A fill: its brush, the brush transform (`None` for a solid colour) and
/// the fill rule.
#[derive(Clone, Debug)]
pub(crate) struct FillPaint {
    pub(crate) brush: Brush,
    pub(crate) brush_transform: Option<Affine>,
    pub(crate) rule: Fill,
}

/// A stroke: its style and brush.
#[derive(Clone, Debug)]
pub(crate) struct StrokePaint {
    pub(crate) style: Stroke,
    pub(crate) brush: Brush,
    pub(crate) brush_transform: Option<Affine>,
}

/// What a compositing layer is clipped to.
#[derive(Clone, Debug)]
pub(crate) enum LayerClip {
    /// The bounding box of the layer's own drawing, in the layer's
    /// coordinates.
    Bounds(Rect),
    /// The element's single-shape clip path, which doubles as the layer's
    /// shape.
    Path(BezPath, Fill),
}

/// Why a document did not parse.
#[derive(Debug)]
pub(crate) enum SvgError {
    /// The bytes are not UTF-8 (gzip-compressed `.svgz` included).
    NotUtf8,
    /// The XML did not parse.
    Xml(roxmltree::Error),
    /// The root element is not `svg`.
    NotSvg,
    /// The replacement text of the document's entities nests markup that
    /// could take the parse past the nesting bound.
    TooDeep,
}

impl std::fmt::Display for SvgError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUtf8 => formatter.write_str("the document is not UTF-8"),
            Self::Xml(error) => write!(formatter, "the document is not well-formed XML: {error}"),
            Self::NotSvg => formatter.write_str("the root element is not <svg>"),
            Self::TooDeep => {
                formatter.write_str("the document's entities could nest markup past the bound")
            }
        }
    }
}

impl std::error::Error for SvgError {}

/// The document crosses from the parsing thread to the document's inside an
/// `ImageEvent`.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<VectorDocument>();
};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
