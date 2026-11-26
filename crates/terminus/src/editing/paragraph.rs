//! The paragraph a selection host's text is laid out in, and the one
//! conversion between that paragraph's layout bytes and the offsets of the
//! selection text.
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
//!
//! A selectable element selected without focus has no value: its selection
//! text is its paragraph's source text, every run of the flattened content
//! and one U+FFFC per atomic inline box ([`Document::session_text`]), whose
//! UTF-16 units are the source units by construction. The same conversions
//! serve it, and its paragraph always counts as showing that text
//! ([`Document::session_paragraph`]).

use euclid::default::{Point2D, Vector2D};
use parley::{Cursor, Layout, Selection};
use seldon::text::paragraph::OffsetMap;

use super::ByteSelection;
use super::offsets::{byte_of_unit_floor, unit_of_byte};
use crate::layout::paragraph::{establishes_paragraph, generated_content};
use crate::layout::{StyleView, TextBrush};
use crate::tree::document::{Document, NodeId};

pub(crate) struct EditableParagraph<'a> {
    pub(crate) node: NodeId,
    pub(crate) layout: &'a Layout<TextBrush>,
    pub(crate) map: OffsetMap<'a>,
    pub(crate) origin: Point2D<f32>,
    pub(crate) shows_value: bool,
}

impl EditableParagraph<'_> {
    pub(crate) fn layout_byte(&self, value: &str, byte: usize) -> Option<usize> {
        let unit = u32::try_from(unit_of_byte(value, byte)).ok()?;
        let at = self.map.unit_to_byte_or_end(unit)?;
        (!self.map.truncated() || at < self.map.cut_byte()).then_some(at)
    }

    pub(crate) fn layout_byte_clamped(&self, value: &str, byte: usize) -> usize {
        self.layout_byte(value, byte)
            .unwrap_or_else(|| self.map.cut_byte())
    }

    pub(crate) fn value_byte(&self, value: &str, byte: usize) -> Option<usize> {
        if self.map.truncated() && byte >= self.map.cut_byte() {
            return None;
        }
        let unit = self.map.byte_to_unit(byte);
        Some(byte_of_unit_floor(value, unit as usize))
    }

    pub(crate) fn value_end_byte(&self, value: &str, byte: usize) -> Option<usize> {
        if self.map.truncated() && byte > self.map.cut_byte() {
            return None;
        }
        let unit = self.map.unit_before(byte);
        Some(byte_of_unit_floor(value, unit as usize))
    }

    pub(crate) fn place(&self, value: &str, selection: ByteSelection) -> Option<Selection> {
        let anchor = self.layout_byte(value, selection.anchor)?;
        let focus = self.layout_byte(value, selection.focus)?;
        Some(Selection::new(
            Cursor::from_byte_index(self.layout, anchor, selection.affinity),
            Cursor::from_byte_index(self.layout, focus, selection.affinity),
        ))
    }

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

    pub(crate) fn read(&self, value: &str, selection: &Selection) -> Option<ByteSelection> {
        Some(ByteSelection {
            anchor: self.value_byte(value, selection.anchor().index())?,
            focus: self.value_byte(value, selection.focus().index())?,
            affinity: selection.focus().affinity(),
        })
    }
}

impl<T> Document<T> {
    pub(crate) fn paragraph_node_of(&self, host: NodeId) -> Option<NodeId> {
        let node = self.get(host)?;
        let view = StyleView::try_of(node)?;
        if establishes_paragraph(view.display_mode()) {
            return self.committed_paragraph(host).map(|_| host);
        }
        let first_text = node.child_ids().iter().copied().find(|&child| {
            self.get(child)
                .is_some_and(crate::tree::node::Node::is_text_node)
        })?;
        let anonymous = self.arenas().anonymous().box_of(first_text)?;
        if self.arenas().anonymous_container(anonymous) != Some(host) {
            return None;
        }
        self.committed_paragraph(anonymous).map(|_| anonymous)
    }

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

    pub(crate) fn session_paragraph(
        &self,
        session: &super::Session,
    ) -> Option<EditableParagraph<'_>> {
        let mut paragraph = self.editable_paragraph(session.host)?;
        if !session.focused {
            paragraph.shows_value = true;
        }
        Some(paragraph)
    }

    fn paragraph_origin(&self, host: NodeId, node: NodeId) -> Point2D<f32> {
        let inset = self.content_inset(node);
        if node == host {
            return inset.to_point();
        }
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
