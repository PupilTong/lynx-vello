//! Fill and stroke styles and the keyword-valued attributes.

use std::f64::consts::TAU;
use std::sync::{Arc, Mutex, PoisonError};

use super::css::parse_color;
use super::{CanvasError, Image};
use crate::kurbo::{Affine, BezPath, Point, Rect, Shape};
use crate::paint::display_list::{self, Texture};
use crate::peniko::color::DynamicColor;
use crate::peniko::{
    BlendMode, BrushRef, Color, ColorStop, Compose, Extend, Fill, ImageQuality, ImageSampler,
    InterpolationAlphaSpace, Mix,
};
use crate::render::canvas::SurfaceImage;

/// Declares a keyword enum with its IDL names: `from_name` answers `None`
/// for a string the attribute ignores.
macro_rules! keywords {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal,)+ }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            /// The value the IDL string names.
            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// The IDL string the getter returns.
            #[must_use]
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }
    };
}

keywords!(
    /// `globalCompositeOperation`.
    CompositeOp {
        SourceOver = "source-over",
        SourceIn = "source-in",
        SourceOut = "source-out",
        SourceAtop = "source-atop",
        DestinationOver = "destination-over",
        DestinationIn = "destination-in",
        DestinationOut = "destination-out",
        DestinationAtop = "destination-atop",
        Lighter = "lighter",
        Copy = "copy",
        Xor = "xor",
        Multiply = "multiply",
        Screen = "screen",
        Overlay = "overlay",
        Darken = "darken",
        Lighten = "lighten",
        ColorDodge = "color-dodge",
        ColorBurn = "color-burn",
        HardLight = "hard-light",
        SoftLight = "soft-light",
        Difference = "difference",
        Exclusion = "exclusion",
        Hue = "hue",
        Saturation = "saturation",
        Color = "color",
        Luminosity = "luminosity",
    }
);

keywords!(
    /// `lineCap`.
    LineCap {
        Butt = "butt",
        Round = "round",
        Square = "square",
    }
);

keywords!(
    /// `lineJoin`.
    LineJoin {
        Round = "round",
        Bevel = "bevel",
        Miter = "miter",
    }
);

keywords!(
    /// `CanvasFillRule`.
    FillRule {
        NonZero = "nonzero",
        EvenOdd = "evenodd",
    }
);

keywords!(
    /// `textAlign`.
    TextAlign {
        Start = "start",
        End = "end",
        Left = "left",
        Right = "right",
        Center = "center",
    }
);

keywords!(
    /// `textBaseline`.
    TextBaseline {
        Top = "top",
        Hanging = "hanging",
        Middle = "middle",
        Alphabetic = "alphabetic",
        Ideographic = "ideographic",
        Bottom = "bottom",
    }
);

keywords!(
    /// `direction`. A canvas with no element to inherit from treats
    /// `inherit` as `ltr`.
    Direction {
        Ltr = "ltr",
        Rtl = "rtl",
        Inherit = "inherit",
    }
);

keywords!(
    /// `imageSmoothingQuality`.
    SmoothingQuality {
        Low = "low",
        Medium = "medium",
        High = "high",
    }
);

keywords!(
    /// A pattern's repetition. `createPattern` also takes the empty string
    /// (and `null`) for `repeat`.
    Repetition {
        Repeat = "repeat",
        RepeatX = "repeat-x",
        RepeatY = "repeat-y",
        NoRepeat = "no-repeat",
    }
);

impl CompositeOp {
    pub(crate) fn blend(self) -> BlendMode {
        let mix = |mix| BlendMode::new(mix, Compose::SrcOver);
        let compose = |compose| BlendMode::new(Mix::Normal, compose);
        match self {
            Self::SourceOver => compose(Compose::SrcOver),
            Self::SourceIn => compose(Compose::SrcIn),
            Self::SourceOut => compose(Compose::SrcOut),
            Self::SourceAtop => compose(Compose::SrcAtop),
            Self::DestinationOver => compose(Compose::DestOver),
            Self::DestinationIn => compose(Compose::DestIn),
            Self::DestinationOut => compose(Compose::DestOut),
            Self::DestinationAtop => compose(Compose::DestAtop),
            Self::Lighter => compose(Compose::Plus),
            Self::Copy => compose(Compose::Copy),
            Self::Xor => compose(Compose::Xor),
            Self::Multiply => mix(Mix::Multiply),
            Self::Screen => mix(Mix::Screen),
            Self::Overlay => mix(Mix::Overlay),
            Self::Darken => mix(Mix::Darken),
            Self::Lighten => mix(Mix::Lighten),
            Self::ColorDodge => mix(Mix::ColorDodge),
            Self::ColorBurn => mix(Mix::ColorBurn),
            Self::HardLight => mix(Mix::HardLight),
            Self::SoftLight => mix(Mix::SoftLight),
            Self::Difference => mix(Mix::Difference),
            Self::Exclusion => mix(Mix::Exclusion),
            Self::Hue => mix(Mix::Hue),
            Self::Saturation => mix(Mix::Saturation),
            Self::Color => mix(Mix::Color),
            Self::Luminosity => mix(Mix::Luminosity),
        }
    }
}

impl From<FillRule> for Fill {
    fn from(rule: FillRule) -> Self {
        match rule {
            FillRule::NonZero => Self::NonZero,
            FillRule::EvenOdd => Self::EvenOdd,
        }
    }
}

impl Repetition {
    /// The repetition `createPattern`'s string names; the empty string is
    /// `repeat`.
    #[must_use]
    pub fn from_pattern_name(name: &str) -> Option<Self> {
        if name.is_empty() {
            Some(Self::Repeat)
        } else {
            Self::from_name(name)
        }
    }
}

/// `fillStyle` or `strokeStyle`.
#[derive(Clone, Debug)]
pub enum Style {
    Color(Color),
    Gradient(Gradient),
    Pattern(Pattern),
}

impl Default for Style {
    fn default() -> Self {
        Self::Color(Color::BLACK)
    }
}

/// A `CanvasGradient`. Clones share one gradient: a stop added through any
/// of them shows in every draw that starts after it.
#[derive(Clone, Debug)]
pub struct Gradient(Arc<Mutex<GradientData>>);

#[derive(Debug)]
struct GradientData {
    gradient: crate::peniko::Gradient,
    /// The geometry is degenerate: the gradient paints nothing whatever its
    /// stops.
    paints_nothing: bool,
}

impl Gradient {
    /// `createLinearGradient`. `Type` for a non-finite argument.
    #[expect(clippy::float_cmp, reason = "the standard's equality is exact")]
    pub fn linear(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<Self, CanvasError> {
        finite(&[x0, y0, x1, y1])?;
        let gradient = crate::peniko::Gradient::new_linear((x0, y0), (x1, y1));
        Ok(Self::new(gradient, x0 == x1 && y0 == y1))
    }

    /// `createRadialGradient`: the cone between two circles. `Type` for a
    /// non-finite argument, `IndexSize` for a negative radius.
    #[expect(clippy::float_cmp, reason = "the standard's equality is exact")]
    pub fn radial(
        x0: f64,
        y0: f64,
        r0: f64,
        x1: f64,
        y1: f64,
        r1: f64,
    ) -> Result<Self, CanvasError> {
        finite(&[x0, y0, r0, x1, y1, r1])?;
        if r0 < 0.0 || r1 < 0.0 {
            return Err(CanvasError::IndexSize);
        }
        #[expect(clippy::cast_possible_truncation, reason = "peniko radii are f32")]
        let gradient =
            crate::peniko::Gradient::new_two_point_radial((x0, y0), r0 as f32, (x1, y1), r1 as f32);
        Ok(Self::new(gradient, x0 == x1 && y0 == y1 && r0 == r1))
    }

    /// `createConicGradient`: stops around `(x, y)` clockwise from
    /// `start_angle` radians off the positive x axis. `Type` for a non-finite
    /// argument.
    pub fn conic(start_angle: f64, x: f64, y: f64) -> Result<Self, CanvasError> {
        finite(&[start_angle, x, y])?;
        // vello measures the angle in [0, 2π); repeating the ramp makes a
        // start angle anywhere wrap the same way.
        let start = start_angle.rem_euclid(TAU);
        #[expect(clippy::cast_possible_truncation, reason = "peniko angles are f32")]
        let mut gradient =
            crate::peniko::Gradient::new_sweep((x, y), start as f32, (start + TAU) as f32);
        gradient.extend = Extend::Repeat;
        Ok(Self::new(gradient, false))
    }

    fn new(mut gradient: crate::peniko::Gradient, paints_nothing: bool) -> Self {
        // HTML "fill and stroke styles": stops interpolate in unpremultiplied
        // RGBA.
        gradient.interpolation_alpha_space = InterpolationAlphaSpace::Unpremultiplied;
        Self(Arc::new(Mutex::new(GradientData {
            gradient,
            paints_nothing,
        })))
    }

    /// `addColorStop`. `Type` for a non-finite offset, `IndexSize` for one
    /// outside 0..=1. Stops at one offset keep the order they were added in.
    pub fn add_color_stop(&self, offset: f64, color: Color) -> Result<(), CanvasError> {
        stop_offset(offset)?;
        #[expect(clippy::cast_possible_truncation, reason = "peniko offsets are f32")]
        let offset = offset as f32;
        let mut data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let stops = &mut data.gradient.stops.0;
        let at = stops.partition_point(|stop| stop.offset <= offset);
        stops.insert(
            at,
            ColorStop {
                offset,
                color: DynamicColor::from_alpha_color(color),
            },
        );
        Ok(())
    }

    /// `addColorStop` given a string, as HTML defines it: `Type` for a
    /// non-finite offset and `IndexSize` for one outside 0..=1, both
    /// checked before the color, then `Syntax` when `css` is not a CSS
    /// `<color>`.
    pub fn add_color_stop_css(&self, offset: f64, css: &str) -> Result<(), CanvasError> {
        stop_offset(offset)?;
        let color = parse_color(css).ok_or(CanvasError::Syntax)?;
        self.add_color_stop(offset, color)
    }

    /// Whether `other` is a clone of this gradient.
    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The paint as of now, its alpha scaled by `alpha`; `None` when it
    /// paints transparent black everywhere.
    fn paint(&self, alpha: f32) -> Option<display_list::Paint> {
        let data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if data.paints_nothing || data.gradient.stops.is_empty() {
            return None;
        }
        let gradient = data.gradient.clone().multiply_alpha(alpha);
        Some(display_list::Paint::from(BrushRef::Gradient(&gradient)))
    }
}

/// A `CanvasPattern`. Clones share one pattern: `set_transform` through any
/// of them shows in every draw that starts after it.
#[derive(Clone, Debug)]
pub struct Pattern(Arc<Mutex<PatternData>>);

#[derive(Debug)]
struct PatternData {
    image: Image,
    repetition: Repetition,
    transform: Affine,
}

impl Pattern {
    /// `createPattern`. `InvalidState` for an image with no pixels. A
    /// canvas's current pixels
    /// ([`OffscreenCanvas::image`](crate::html::canvas::OffscreenCanvas::image)) are copied as they
    /// are now.
    pub fn new(image: Image, repetition: Repetition) -> Result<Self, CanvasError> {
        if image.is_empty() {
            return Err(CanvasError::InvalidState);
        }
        Ok(Self(Arc::new(Mutex::new(PatternData {
            image: image.stable(),
            repetition,
            transform: Affine::IDENTITY,
        }))))
    }

    /// `setTransform(matrix)`, the matrix as `[a, b, c, d, e, f]`. `Type`
    /// for a non-finite entry.
    pub fn set_transform(&self, matrix: [f64; 6]) -> Result<(), CanvasError> {
        finite(&matrix)?;
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .transform = Affine::new(matrix);
        Ok(())
    }

    #[must_use]
    pub fn repetition(&self) -> Repetition {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .repetition
    }
}

/// What one draw paints with: the brush, the transform from paint space to
/// canvas space, and the region outside which the paint is transparent
/// (a pattern that does not repeat on an axis), in canvas space; with the
/// texture the brush samples, kept until the draw is replayed, and the
/// canvas texture it reads as it is at replay.
#[derive(Debug)]
pub(crate) struct Paint {
    pub(crate) brush: display_list::Paint,
    pub(crate) transform: Affine,
    pub(crate) bounds: Option<BezPath>,
    pub(crate) keep: Option<SurfaceImage>,
    pub(crate) live: Option<Texture>,
}

impl Paint {
    /// A paint of `brush` in canvas space.
    fn plain(brush: display_list::Paint, transform: Affine) -> Self {
        Self {
            brush,
            transform,
            bounds: None,
            keep: None,
            live: None,
        }
    }

    /// The brush's transform relative to a shape under `geometry`, which
    /// is invertible; `None` for the identity.
    pub(crate) fn brush_transform(&self, geometry: Affine) -> Option<Affine> {
        let transform = geometry.inverse() * self.transform;
        (transform != Affine::IDENTITY).then_some(transform)
    }
}

/// The sampler options `imageSmoothingEnabled` and `imageSmoothingQuality`
/// select.
pub(crate) fn image_quality(smoothing: bool, quality: SmoothingQuality) -> ImageQuality {
    match (smoothing, quality) {
        (false, _) => ImageQuality::Low,
        (true, SmoothingQuality::High) => ImageQuality::High,
        (true, _) => ImageQuality::Medium,
    }
}

impl Style {
    /// The paint for one draw under the current transform `ctm`, or `None`
    /// when the style paints transparent black everywhere.
    pub(crate) fn paint(
        &self,
        ctm: Affine,
        alpha: f32,
        quality: ImageQuality,
        canvas: Rect,
    ) -> Option<Paint> {
        match self {
            Self::Color(color) => {
                let color = color.multiply_alpha(alpha);
                (color.components[3] > 0.0)
                    .then(|| Paint::plain(display_list::Paint::Solid(color), Affine::IDENTITY))
            }
            Self::Gradient(gradient) => gradient.paint(alpha).map(|brush| Paint::plain(brush, ctm)),
            Self::Pattern(pattern) => {
                let data = pattern.0.lock().unwrap_or_else(PoisonError::into_inner);
                let (x_extend, y_extend) = match data.repetition {
                    Repetition::Repeat => (Extend::Repeat, Extend::Repeat),
                    Repetition::RepeatX => (Extend::Repeat, Extend::Pad),
                    Repetition::RepeatY => (Extend::Pad, Extend::Repeat),
                    Repetition::NoRepeat => (Extend::Pad, Extend::Pad),
                };
                let transform = ctm * data.transform;
                let image = &data.image;
                let tile = Rect::new(
                    0.0,
                    0.0,
                    f64::from(image.width()),
                    f64::from(image.height()),
                );
                let bounds = match data.repetition {
                    Repetition::Repeat => None,
                    repetition => Some(pattern_bounds(repetition, tile, transform, canvas)?),
                };
                let sampler = ImageSampler {
                    x_extend,
                    y_extend,
                    quality,
                    alpha,
                };
                let (brush, keep, live) = image.paint(sampler);
                Some(Paint {
                    brush,
                    transform,
                    bounds,
                    keep,
                    live,
                })
            }
        }
    }
}

/// The region in canvas space a pattern repeating only on `repetition`'s
/// axes covers: one tile row, column or the tile itself. `None` when the
/// transform is singular, which paints nothing.
fn pattern_bounds(
    repetition: Repetition,
    tile: Rect,
    transform: Affine,
    canvas: Rect,
) -> Option<BezPath> {
    if transform.determinant() == 0.0 {
        return None;
    }
    // The canvas in pattern space bounds how far a repeating axis needs to
    // reach.
    let reach = transform
        .inverse()
        .transform_rect_bbox(canvas)
        .inflate(1.0, 1.0);
    let band = match repetition {
        Repetition::Repeat => reach,
        Repetition::RepeatX => Rect::new(reach.x0, tile.y0, reach.x1, tile.y1),
        Repetition::RepeatY => Rect::new(tile.x0, reach.y0, tile.x1, reach.y1),
        Repetition::NoRepeat => tile,
    };
    let mut path = band.to_path(0.1);
    path.apply_affine(transform);
    Some(path)
}

/// `addColorStop`'s offset check: `Type` for a non-finite offset,
/// `IndexSize` for one outside 0..=1.
fn stop_offset(offset: f64) -> Result<(), CanvasError> {
    finite(&[offset])?;
    if (0.0..=1.0).contains(&offset) {
        Ok(())
    } else {
        Err(CanvasError::IndexSize)
    }
}

pub(crate) fn finite(values: &[f64]) -> Result<(), CanvasError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(CanvasError::Type)
    }
}

/// The canvas-space rectangle `[0, w] × [0, h]`.
pub(crate) fn canvas_rect(width: u32, height: u32) -> Rect {
    Rect::from_origin_size(Point::ZERO, (f64::from(width), f64::from(height)))
}
