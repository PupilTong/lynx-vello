//! HTML's `OffscreenCanvasRenderingContext2D` (canvas.html, "The 2D
//! rendering context") over one `OffscreenCanvas`'s bitmap, replayed on the
//! GPU.
//!
//! A [`Context2d`] is the 2D context of one
//! [`OffscreenCanvas`](super::OffscreenCanvas), borrowed from it: the
//! context's drawing state, state stack and default path, which the canvas
//! keeps, and the canvas's bitmap it draws on. The canvas owns the size,
//! the bitmap and its commit. Each context method and attribute is one call taking numbers,
//! `&str`, small enums or the module's own value types ([`Path`],
//! [`Gradient`], [`Pattern`], [`Image`]); errors name the exception the
//! standard throws ([`CanvasError`]). There is no JavaScript here.
//!
//! The attributes that take a CSS string have HTML's setter steps here, in
//! two forms. A string form — [`Context2d::set_fill_style_css`],
//! `set_stroke_style_css`, `set_shadow_color_css`, `set_font_css` and
//! [`Gradient::add_color_stop_css`] — parses the string with stylo
//! (`css.rs`): one that does not parse leaves the attribute as it was and
//! answers `None`, one that does sets it and answers the serialization the
//! attribute's getter gives. Colors are a CSS `<color>` (`currentcolor` as
//! opaque black) serialized as `#rrggbb` or `rgba(r, g, b, a)`; fonts are
//! the `font` shorthand, computed with no document and no device, relative
//! sizes against the default font's 10px, serialized with the size in px
//! and no line height. A typed form takes the parsed value: a
//! [`peniko::Color`](crate::peniko::Color), a [`Style`] (for a gradient or
//! a pattern) or a [`ResolvedFont`]. The caller owns the
//! [`TextContext`](seldon::text::TextContext) text is shaped in, through
//! the one-line shaper [`shape_line`](seldon::text::shape_line) that SVG
//! `<text>` shapes with too. A canvas needs no
//! [`Document`](crate::tree::document::Document): a Worker's
//! `OffscreenCanvas` has none.
//!
//! Draw calls are recorded into a [`DisplayList`](crate::paint::display_list::DisplayList)
//! ([`Raster`]) and replayed onto the canvas's GPU texture
//! ([`Surface`](crate::render::canvas::Surface)) at a read, at the owner's
//! flush, or once the recording grows past a bound
//! (`docs/terminus-architecture.md`, "Canvas 2D context").

#![allow(
    clippy::many_single_char_names,
    reason = "the IDL methods' argument names: x, y, w, h, a..f"
)]

mod context;
mod css;
mod image;
mod path;
mod pixels;
mod raster;
mod style;
mod text;

pub use context::Context2d;
pub(super) use context::Context2dData;
pub use image::Image;
pub(super) use image::Source;
pub use path::Path;
pub use pixels::ImageDataRead;
pub(super) use raster::Raster;
pub use seldon::text::{GenericFamily, ResolvedFamily, ResolvedFont, ResolvedFontStyle};
pub use style::{
    CompositeOp, Direction, FillRule, Gradient, LineCap, LineJoin, Pattern, Repetition,
    SmoothingQuality, Style, TextAlign, TextBaseline,
};
pub use text::TextMetrics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasError {
    IndexSize,
    InvalidState,
    Syntax,
    Type,
    Range,
}

impl CanvasError {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::IndexSize => "IndexSizeError",
            Self::InvalidState => "InvalidStateError",
            Self::Syntax => "SyntaxError",
            Self::Type => "TypeError",
            Self::Range => "RangeError",
        }
    }

    #[must_use]
    pub fn is_dom_exception(self) -> bool {
        matches!(self, Self::IndexSize | Self::InvalidState | Self::Syntax)
    }
}

impl std::fmt::Display for CanvasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for CanvasError {}
