//! [`TextInputState`]: everything a platform input method asks about the
//! focused host, in one comparable value.
//!
//! Chromium's `TextInputState` is the model: the field type, mode and
//! action, the whole value, the selection and the composition in UTF-16
//! units, and the caret's and each composed character's bounds. An embedder
//! publishes it whenever it differs from the last one it published
//! (`PartialEq`), and answers every synchronous platform query from it.
//!
//! Offsets are value offsets, read back from the host's paragraph through
//! its source map. While that paragraph is not built from the value alone
//! (generated `content`, an element child's text) and the selection is one
//! placed in it, or an end of it lies past a truncation cut, the selection
//! is reported collapsed at the value's end ([`crate::editing`]).
//!
//! Geometry is viewport CSS px from the last committed layout, walked the way
//! [`Document::bounding_client_rect`] walks it: sticky offsets and scroll
//! offsets are applied, transforms are not.

use core::ops::Range;

use euclid::default::{Point2D, Rect, Size2D};
use parley::{Cluster, Cursor, Layout};
use stylo::properties::ComputedValues;

use super::offsets::{unit_of_byte, utf16_len};
use crate::layout::TextBrush;
use crate::tree::document::{Document, NodeId};

pub(crate) const CARET_WIDTH: f32 = 1.0;

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputType {
    #[default]
    Text,
    Password,
    Search,
    Email,
    Number,
    Tel,
    Url,
}

impl InputType {
    fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::Text;
        };
        [
            ("password", Self::Password),
            ("search", Self::Search),
            ("email", Self::Email),
            ("number", Self::Number),
            ("tel", Self::Tel),
            ("url", Self::Url),
        ]
        .into_iter()
        .find(|(keyword, _)| value.eq_ignore_ascii_case(keyword))
        .map_or(Self::Text, |(_, kind)| kind)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Default,
    None,
    Text,
    Decimal,
    Numeric,
    Tel,
    Search,
    Email,
    Url,
}

impl InputMode {
    fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::Default;
        };
        [
            ("none", Self::None),
            ("text", Self::Text),
            ("decimal", Self::Decimal),
            ("numeric", Self::Numeric),
            ("tel", Self::Tel),
            ("search", Self::Search),
            ("email", Self::Email),
            ("url", Self::Url),
        ]
        .into_iter()
        .find(|(keyword, _)| value.eq_ignore_ascii_case(keyword))
        .map_or(Self::Default, |(_, mode)| mode)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnterKeyHint {
    #[default]
    Default,
    Enter,
    Done,
    Go,
    Next,
    Previous,
    Search,
    Send,
}

impl EnterKeyHint {
    fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::Default;
        };
        [
            ("enter", Self::Enter),
            ("done", Self::Done),
            ("go", Self::Go),
            ("next", Self::Next),
            ("previous", Self::Previous),
            ("search", Self::Search),
            ("send", Self::Send),
        ]
        .into_iter()
        .find(|(keyword, _)| value.eq_ignore_ascii_case(keyword))
        .map_or(Self::Default, |(_, hint)| hint)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextInputState {
    pub host: Option<NodeId>,
    pub input_type: InputType,
    pub input_mode: InputMode,
    pub enter_key_hint: EnterKeyHint,
    pub value: String,
    pub selection: Range<u32>,
    pub composition: Option<Range<u32>>,
    pub caret: Rect<f32>,
    pub editable_bounds: Rect<f32>,
    pub composition_bounds: Vec<Rect<f32>>,
}

pub(crate) fn empty_line_height(style: &ComputedValues) -> f32 {
    use stylo::values::computed::font::LineHeight;
    let font_size = style.get_font().font_size.computed_size().px();
    match style.get_font().line_height {
        LineHeight::Normal => font_size * 1.2,
        LineHeight::Number(factor) => font_size * factor.0,
        LineHeight::Length(length) => length.0.px(),
    }
}

pub(crate) fn caret_box(
    layout: &Layout<TextBrush>,
    cursor: Cursor,
    width: f32,
    fallback: f32,
) -> Rect<f32> {
    let geometry = cursor.geometry(layout, width);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "parley's f64 boxes hold f32 layout values"
    )]
    let rect = Rect::new(
        Point2D::new(geometry.x0 as f32, geometry.y0 as f32),
        Size2D::new(width, (geometry.y1 - geometry.y0) as f32),
    );
    if rect.size.height > 0.0 {
        rect
    } else {
        Rect::new(Point2D::zero(), Size2D::new(width, fallback))
    }
}

pub(crate) fn cluster_box(layout: &Layout<TextBrush>, byte: usize) -> Option<Rect<f32>> {
    let cluster = Cluster::from_byte_index(layout, byte)?;
    let x = cluster.visual_offset()?;
    let metrics = *cluster.line().metrics();
    Some(Rect::new(
        Point2D::new(x, metrics.block_min_coord),
        Size2D::new(
            cluster.advance(),
            metrics.block_max_coord - metrics.block_min_coord,
        ),
    ))
}

impl<T> Document<T> {
    #[must_use]
    pub fn text_input_state(&self) -> TextInputState {
        let Some(session) = self.focused_session() else {
            return TextInputState::default();
        };
        let host = session.host;
        let Some(node) = self.get(host) else {
            return TextInputState::default();
        };
        let value = self.host_value(host);
        let paragraph = self.editable_paragraph(host);
        let range = session
            .bytes(paragraph.as_ref(), &value)
            .map_or(value.len()..value.len(), super::ByteSelection::range);
        let to_units = |byte: usize| u32::try_from(unit_of_byte(&value, byte)).unwrap_or(u32::MAX);
        let selection = to_units(range.start)..to_units(range.end);
        let composition = session
            .composition
            .as_ref()
            .map(|range| to_units(range.start)..to_units(range.end));

        let host_rect = self.bounding_client_rect(host);
        let editable_bounds = host_rect.unwrap_or_else(Rect::zero);
        let mut caret = Rect::zero();
        let mut composition_bounds = Vec::new();
        let fallback = self.paint_style(host).map_or(0.0, empty_line_height);
        if host_rect.is_some() {
            if let Some(paragraph) = &paragraph {
                let origin = editable_bounds.origin + paragraph.origin.to_vector();
                let focus = session.selection_in(paragraph, &value).focus();
                caret = caret_box(paragraph.layout, focus, CARET_WIDTH, fallback)
                    .translate(origin.to_vector());
                if let Some(range) = &session.composition
                    && let Some(composed) = value.get(range.clone())
                {
                    for (offset, character) in composed.char_indices() {
                        let rect = paragraph
                            .layout_byte(&value, range.start + offset)
                            .and_then(|byte| cluster_box(paragraph.layout, byte))
                            .map_or_else(Rect::zero, |rect| rect.translate(origin.to_vector()));
                        composition_bounds.extend(std::iter::repeat_n(rect, character.len_utf16()));
                    }
                }
            } else {
                let origin = editable_bounds.origin + self.content_inset(host);
                caret = Rect::new(origin, Size2D::new(CARET_WIDTH, fallback));
                if let Some(range) = &session.composition {
                    let units = value.get(range.clone()).map_or(0, utf16_len);
                    composition_bounds = vec![Rect::new(origin, Size2D::zero()); units];
                }
            }
        }

        TextInputState {
            host: Some(host),
            input_type: InputType::parse(node.attribute("type").as_deref()),
            input_mode: InputMode::parse(node.attribute("inputmode").as_deref()),
            enter_key_hint: EnterKeyHint::parse(node.attribute("enterkeyhint").as_deref()),
            value,
            selection,
            composition,
            caret,
            editable_bounds,
            composition_bounds,
        }
    }
}
