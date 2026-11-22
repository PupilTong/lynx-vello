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
//! standard throws ([`CanvasError`]). There is no JavaScript, document or CSS
//! parsing here: colors arrive as [`peniko::Color`](crate::peniko::Color),
//! fonts as a [`ResolvedFont`], and the caller owns the
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

/// The exception a call throws, named as the standard names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasError {
    /// `DOMException` `IndexSizeError`.
    IndexSize,
    /// `DOMException` `InvalidStateError`.
    InvalidState,
    /// `DOMException` `SyntaxError`.
    Syntax,
    /// ECMAScript `TypeError`.
    Type,
    /// ECMAScript `RangeError`.
    Range,
}

impl CanvasError {
    /// The exception's name: the `DOMException` name, or the ECMAScript
    /// error constructor's.
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

    /// Whether the JavaScript layer throws a `DOMException` (rather than an
    /// ECMAScript error object).
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
