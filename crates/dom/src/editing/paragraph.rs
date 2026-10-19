//! The paragraph an editing host's text is laid out in, and the one
//! conversion between that paragraph's layout bytes and the value's
//! offsets.
//!
//! Editing is not tied to the kind of text container. A host's text is laid
//! out by whichever paragraph its element kind and display give it:
//!
//! - a Lynx text element lays out its own Lynx paragraph (`TextStore::Block`), with native Lynx's
//!   verbatim white space and truncation;
//! - a block container (an engine sheet's `display: block`) lays out its own CSS inline formatting
//!   context (`TextStore::Flow`);
//! - any other element — a flex, grid, linear or relative `view` or `div` — lays its loose text out
//!   in an anonymous block box ([`crate::tree::anonymous`]), whose inline formatting context is the
//!   paragraph, and whose container is the host.
//!
//! The value's offsets are not the layout's: white space a paragraph
//! collapses, the edges of an inline box and a truncation tail all move
//! bytes. The value's UTF-16 units are the paragraph's *source* units — the
//! paragraph is built from the host's text children in content-walk order —
//! so every conversion goes value byte → UTF-16 unit → layout byte and back
//! through the paragraph's own source map ([`OffsetMap`]).

use hughie::text::paragraph::OffsetMap;
use parley::{Cursor, Layout, Selection};

use super::ByteSelection;
use super::offsets::{byte_of_unit_floor, unit_of_byte};
use crate::layout::paragraph::{establishes_paragraph, generated_content};
use crate::layout::{StyleView, TextBrush};
use crate::tree::document::NodeId;
use crate::{Document, Point2D, Vector2D};

/// The committed paragraph a host's text is laid out in, as the editor
/// reads it.
pub(crate) struct EditableParagraph<'a> {
    /// The node whose layout holds the paragraph: the host itself, or the
    /// anonymous block box around its text.
    pub(crate) node: NodeId,
    /// The display layout, in the paragraph's own coordinates (its node's
    /// content-box top-left).
    pub(crate) layout: &'a Layout<TextBrush>,
    /// The bridge between `layout`'s bytes and the paragraph's source units.
    pub(crate) map: OffsetMap<'a>,
    /// The paragraph's origin in the host's border-box space.
    pub(crate) origin: Point2D<f32>,
    /// Whether the paragraph is built from the host's value and nothing
    /// else: every child of the host is a text node, no generated `content`
    /// replaces them, and every run of the paragraph is the host's own. Only
    /// then are its source units the value's UTF-16 units.
    pub(crate) shows_value: bool,
}

impl EditableParagraph<'_> {
    /// The layout byte value byte `byte` sits at. `None` where the display
    /// layout does not show it: at or past a truncation cut, or after the
    /// first line of a `<text>` that shows only that one.
    pub(crate) fn layout_byte(&self, value: &str, byte: usize) -> Option<usize> {
        let unit = u32::try_from(unit_of_byte(value, byte)).ok()?;
        let at = self.map.unit_to_byte_or_end(unit)?;
        (!self.map.truncated() || at < self.map.cut_byte()).then_some(at)
    }

    /// [`Self::layout_byte`], or the end of what the layout shows: where a
    /// painted caret or a reported box goes for an offset the layout lacks.
    pub(crate) fn layout_byte_clamped(&self, value: &str, byte: usize) -> usize {
        self.layout_byte(value, byte)
            .unwrap_or_else(|| self.map.cut_byte())
    }

    /// The value byte layout byte `byte` stands for: the start of the
    /// value's character there. A byte inside a collapsed white-space span
    /// reads as the span's start, one of an inline box's edges as the unit
    /// after it. `None` at or past a truncation cut, whose tail is no value
    /// text.
    pub(crate) fn value_byte(&self, value: &str, byte: usize) -> Option<usize> {
        if self.map.truncated() && byte >= self.map.cut_byte() {
            return None;
        }
        let unit = self.map.byte_to_unit(byte);
        Some(byte_of_unit_floor(value, unit as usize))
    }

    /// The value byte layout byte `byte` stands for as the *end* of a
    /// range: everything before it, a collapsed span sitting at it
    /// included. `None` past a truncation cut; the cut itself ends the last
    /// visible cluster.
    pub(crate) fn value_end_byte(&self, value: &str, byte: usize) -> Option<usize> {
        if self.map.truncated() && byte > self.map.cut_byte() {
            return None;
        }
        let unit = self.map.unit_before(byte);
        Some(byte_of_unit_floor(value, unit as usize))
    }

    /// `selection` (value bytes) placed in the layout; `None` when either
    /// end is not shown there.
    pub(crate) fn place(&self, value: &str, selection: ByteSelection) -> Option<Selection> {
        let anchor = self.layout_byte(value, selection.anchor)?;
        let focus = self.layout_byte(value, selection.focus)?;
        Some(Selection::new(
            Cursor::from_byte_index(self.layout, anchor, selection.affinity),
            Cursor::from_byte_index(self.layout, focus, selection.affinity),
        ))
    }

    /// [`Self::place`], with an end the layout does not show moved to the
    /// end of what it shows: for painting and geometry.
    pub(crate) fn place_clamped(&self, value: &str, selection: ByteSelection) -> Selection {
        Selection::new(
            Cursor::from_byte_index(
                self.layout,
                self.layout_byte_clamped(value, selection.anchor),
                selection.affinity,
            ),
            Cursor::from_byte_index(
                self.layout,
                self.layout_byte_clamped(value, selection.focus),
                selection.affinity,
            ),
        )
    }

    /// A parley selection over the layout, read back as value bytes; `None`
    /// when either end lies at or past a truncation cut.
    pub(crate) fn read(&self, value: &str, selection: &Selection) -> Option<ByteSelection> {
        Some(ByteSelection {
            anchor: self.value_byte(value, selection.anchor().index())?,
            focus: self.value_byte(value, selection.focus().index())?,
            affinity: selection.focus().affinity(),
        })
    }
}

impl<T> Document<T> {
    /// The node whose committed paragraph lays `host`'s text out: `host`
    /// when it establishes a paragraph itself (a Lynx text element, a block
    /// container), else the anonymous block box of its first text child,
    /// whose container is `host`. `None` when no committed paragraph holds
    /// the text: an empty host, one whose text is all white space (which a
    /// flex, grid or other item container does not render, css-flexbox-1
    /// §4), a host no pass has laid out, an inline box or a `display:
    /// contents` host, whose text is laid out by a paragraph around it.
    pub(crate) fn paragraph_node_of(&self, host: NodeId) -> Option<NodeId> {
        let node = self.get(host)?;
        let view = StyleView::try_of(node)?;
        if establishes_paragraph(view.display_mode()) {
            return self.committed_paragraph(host).map(|_| host);
        }
        let first_text = node
            .child_ids()
            .iter()
            .copied()
            .find(|&child| self.get(child).is_some_and(crate::Node::is_text_node))?;
        let anonymous = self.arenas().anonymous().box_of(first_text)?;
        if self.arenas().anonymous_container(anonymous) != Some(host) {
            return None;
        }
        self.committed_paragraph(anonymous).map(|_| anonymous)
    }

    /// The committed paragraph `host`'s text is laid out in
    /// ([`Self::paragraph_node_of`]), as the editor reads it.
    pub(crate) fn editable_paragraph(&self, host: NodeId) -> Option<EditableParagraph<'_>> {
        let node = self.paragraph_node_of(host)?;
        let view = self.committed_paragraph(node)?.editable()?;
        let shows_value = self.host_is_plain_text(host)
            && generated_content(self.get(host)?).is_none()
            && view.sources.iter().all(|&source| source == host);
        Some(EditableParagraph {
            node,
            layout: view.layout,
            map: view.map,
            origin: self.paragraph_origin(host, node),
            shows_value,
        })
    }

    /// Where the content box of `node` — the host, or the anonymous box of
    /// its text — sits in `host`'s border-box space.
    fn paragraph_origin(&self, host: NodeId, node: NodeId) -> Point2D<f32> {
        let inset = self.content_inset(node);
        if node == host {
            return inset.to_point();
        }
        // An anonymous box is laid out inside its container, so the two
        // client rects differ by its place there and by whatever the host
        // scrolls by.
        match (
            self.bounding_client_rect(node),
            self.bounding_client_rect(host),
        ) {
            (Some(paragraph), Some(host)) => (paragraph.origin - host.origin + inset).to_point(),
            _ => self.rounded_layout(node).map_or_else(
                || inset.to_point(),
                |layout| Point2D::new(layout.location.x, layout.location.y) + inset,
            ),
        }
    }

    /// The border and padding before `node`'s content box, from its last
    /// committed layout.
    pub(crate) fn content_inset(&self, node: NodeId) -> Vector2D<f32> {
        self.rounded_layout(node)
            .map_or_else(Vector2D::zero, |layout| {
                Vector2D::new(
                    layout.border.left + layout.padding.left,
                    layout.border.top + layout.padding.top,
                )
            })
    }
}
