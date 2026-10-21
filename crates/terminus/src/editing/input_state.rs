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

/// The caret's width, in CSS px.
pub(crate) const CARET_WIDTH: f32 = 1.0;

/// What kind of field the focused host is, which picks the platform
/// keyboard and its behaviors (Chromium's `TextInputType`, HTML's `<input
/// type>`). Read from the host's `type` attribute with HTML's `<input type>`
/// keywords (ASCII case-insensitive), [`Self::Text`] for anything else and
/// when it is absent: a plain `contenteditable` host carries none, and the
/// element layer over this kernel (`x-input`'s `type`) writes it on the
/// editing host in its shadow tree.
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
    /// HTML's `<input type>` keywords this enumeration names, ASCII
    /// case-insensitive; every other value, and none, is [`Self::Text`].
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

/// HTML's `inputmode` attribute: the virtual keyboard a host asks for.
/// [`Self::Default`] when the attribute is absent or invalid, which leaves
/// the choice to the input type.
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
    /// HTML's enumerated-attribute parse, ASCII case-insensitive.
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

/// HTML's `enterkeyhint` attribute: the label of the virtual keyboard's
/// enter key. [`Self::Default`] when the attribute is absent or invalid.
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

/// The focused host's editing state, as a platform input method sees it.
/// The default value is "nothing editable is focused".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextInputState {
    /// The focused editing host; `None` when nothing editable is focused,
    /// and then every other field is its default.
    pub host: Option<NodeId>,
    pub input_type: InputType,
    /// From the host's `inputmode` attribute.
    pub input_mode: InputMode,
    /// From the host's `enterkeyhint` attribute.
    pub enter_key_hint: EnterKeyHint,
    /// The whole value: the host's direct text-node children, concatenated.
    pub value: String,
    /// The selection, start ≤ end, in UTF-16 units into `value`.
    pub selection: Range<u32>,
    /// The composition, in UTF-16 units into `value`.
    pub composition: Option<Range<u32>>,
    /// The caret at the selection's focus, `CARET_WIDTH` wide and as tall
    /// as its line, in viewport CSS px.
    pub caret: Rect<f32>,
    /// The host's border box, in viewport CSS px.
    pub editable_bounds: Rect<f32>,
    /// One box per UTF-16 unit of the composition, in viewport CSS px: the
    /// box of the grapheme cluster the unit belongs to, so both units of a
    /// surrogate pair and every code point of one cluster share a box.
    pub composition_bounds: Vec<Rect<f32>>,
}

/// The used line height of `style` for a host with no paragraph to measure
/// a line in — a flex `view` with no text, or with only white space it does
/// not render: its caret is still one line tall. A host that establishes a
/// paragraph of its own has a line even when empty (one empty line of its
/// strut), and its caret is that line's.
pub(crate) fn empty_line_height(style: &ComputedValues) -> f32 {
    use stylo::values::computed::font::LineHeight;
    let font_size = style.get_font().font_size.computed_size().px();
    match style.get_font().line_height {
        LineHeight::Normal => font_size * 1.2,
        LineHeight::Number(factor) => font_size * factor.0,
        LineHeight::Length(length) => length.0.px(),
    }
}

/// The caret box at `cursor` in `layout`'s coordinates, `width` wide, as
/// tall as its line. An empty host's own paragraph has one empty line, which
/// answers here like any other; a layout with no line at all puts the caret
/// at the origin, `fallback` tall.
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

/// The box of the grapheme cluster holding `byte`, in `layout`'s
/// coordinates: its advance by its line's height.
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
    /// The focused host's [`TextInputState`], from the last committed
    /// layout. Runs no pass: geometry of a change not laid out yet is the
    /// previous layout's, while the value and the offsets are current.
    #[must_use]
    pub fn text_input_state(&self) -> TextInputState {
        // A selectable element selected without focus is nothing a platform
        // input method edits.
        let Some(session) = self.focused_session() else {
            return TextInputState::default();
        };
        let host = session.host;
        let Some(node) = self.get(host) else {
            return TextInputState::default();
        };
        let value = self.host_value(host);
        let paragraph = self.editable_paragraph(host);
        // A parley selection the paragraph cannot read back as value offsets
        // is reported collapsed at the value's end rather than as offsets
        // into some other text.
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
                // No paragraph — a flex host with no text, or with white
                // space it does not render: the caret at the content box's
                // origin, one line tall. A host with a paragraph of its own
                // has a line even when empty, and is answered above.
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
            input_type: InputType::parse(node.attribute("type")),
            input_mode: InputMode::parse(node.attribute("inputmode")),
            enter_key_hint: EnterKeyHint::parse(node.attribute("enterkeyhint")),
            value,
            selection,
            composition,
            caret,
            editable_bounds,
            composition_bounds,
        }
    }
}
