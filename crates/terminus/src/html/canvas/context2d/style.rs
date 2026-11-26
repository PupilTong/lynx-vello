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

macro_rules! keywords {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal,)+ }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }

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
    LineCap {
        Butt = "butt",
        Round = "round",
        Square = "square",
    }
);

keywords!(
    LineJoin {
        Round = "round",
        Bevel = "bevel",
        Miter = "miter",
    }
);

keywords!(
    FillRule {
        NonZero = "nonzero",
        EvenOdd = "evenodd",
    }
);

keywords!(
    TextAlign {
        Start = "start",
        End = "end",
        Left = "left",
        Right = "right",
        Center = "center",
    }
);

keywords!(
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
    Direction {
        Ltr = "ltr",
        Rtl = "rtl",
        Inherit = "inherit",
    }
);

keywords!(
    SmoothingQuality {
        Low = "low",
        Medium = "medium",
        High = "high",
    }
);

keywords!(
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
    #[must_use]
    pub fn from_pattern_name(name: &str) -> Option<Self> {
        if name.is_empty() {
            Some(Self::Repeat)
        } else {
            Self::from_name(name)
        }
    }
}

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

#[derive(Clone, Debug)]
pub struct Gradient(Arc<Mutex<GradientData>>);

#[derive(Debug)]
struct GradientData {
    gradient: crate::peniko::Gradient,
    paints_nothing: bool,
}

impl Gradient {
    #[expect(clippy::float_cmp, reason = "the standard's equality is exact")]
    pub fn linear(x0: f64, y0: f64, x1: f64, y1: f64) -> Result<Self, CanvasError> {
        finite(&[x0, y0, x1, y1])?;
        let gradient = crate::peniko::Gradient::new_linear((x0, y0), (x1, y1));
        Ok(Self::new(gradient, x0 == x1 && y0 == y1))
    }

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

    pub fn conic(start_angle: f64, x: f64, y: f64) -> Result<Self, CanvasError> {
        finite(&[start_angle, x, y])?;
        let start = start_angle.rem_euclid(TAU);
        #[expect(clippy::cast_possible_truncation, reason = "peniko angles are f32")]
        let mut gradient =
            crate::peniko::Gradient::new_sweep((x, y), start as f32, (start + TAU) as f32);
        gradient.extend = Extend::Repeat;
        Ok(Self::new(gradient, false))
    }

    fn new(mut gradient: crate::peniko::Gradient, paints_nothing: bool) -> Self {
        gradient.interpolation_alpha_space = InterpolationAlphaSpace::Unpremultiplied;
        Self(Arc::new(Mutex::new(GradientData {
            gradient,
            paints_nothing,
        })))
    }

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

    pub fn add_color_stop_css(&self, offset: f64, css: &str) -> Result<(), CanvasError> {
        stop_offset(offset)?;
        let color = parse_color(css).ok_or(CanvasError::Syntax)?;
        self.add_color_stop(offset, color)
    }

    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn paint(&self, alpha: f32) -> Option<display_list::Paint> {
        let data = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if data.paints_nothing || data.gradient.stops.is_empty() {
            return None;
        }
        let gradient = data.gradient.clone().multiply_alpha(alpha);
        Some(display_list::Paint::from(BrushRef::Gradient(&gradient)))
    }
}

#[derive(Clone, Debug)]
pub struct Pattern(Arc<Mutex<PatternData>>);

#[derive(Debug)]
struct PatternData {
    image: Image,
    repetition: Repetition,
    transform: Affine,
}

impl Pattern {
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

#[derive(Debug)]
pub(crate) struct Paint {
    pub(crate) brush: display_list::Paint,
    pub(crate) transform: Affine,
    pub(crate) bounds: Option<BezPath>,
    pub(crate) keep: Option<SurfaceImage>,
    pub(crate) live: Option<Texture>,
}

impl Paint {
    fn plain(brush: display_list::Paint, transform: Affine) -> Self {
        Self {
            brush,
            transform,
            bounds: None,
            keep: None,
            live: None,
        }
    }

    pub(crate) fn brush_transform(&self, geometry: Affine) -> Option<Affine> {
        let transform = geometry.inverse() * self.transform;
        (transform != Affine::IDENTITY).then_some(transform)
    }
}

pub(crate) fn image_quality(smoothing: bool, quality: SmoothingQuality) -> ImageQuality {
    match (smoothing, quality) {
        (false, _) => ImageQuality::Low,
        (true, SmoothingQuality::High) => ImageQuality::High,
        (true, _) => ImageQuality::Medium,
    }
}

impl Style {
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

fn pattern_bounds(
    repetition: Repetition,
    tile: Rect,
    transform: Affine,
    canvas: Rect,
) -> Option<BezPath> {
    if transform.determinant() == 0.0 {
        return None;
    }
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

pub(crate) fn canvas_rect(width: u32, height: u32) -> Rect {
    Rect::from_origin_size(Point::ZERO, (f64::from(width), f64::from(height)))
}
