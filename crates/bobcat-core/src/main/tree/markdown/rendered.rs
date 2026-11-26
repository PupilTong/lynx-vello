//! The rendered text of a markdown element's shadow root: web-core's
//! `root.textContent`, which its `getParseResult`, `getTextBoundingRect` and
//! `getCharIndexByPoint` count offsets in, and the paragraphs that show it.
//!
//! The text is every text node under `#markdown-root` in tree order, plus the
//! `"\n"` text nodes web-core's tree holds between block tags, which are not
//! nodes here and are counted from [`super::build::Newlines`]. A list item's
//! marker is skipped: it is the browser's `::marker`, no node there. Offsets
//! are UTF-16 code units, JavaScript string indices.
//!
//! A paragraph's own offsets are its source units
//! ([`terminus::link::Document::text_range_boxes`]): the same text nodes, plus one unit
//! for each atomic inline (an image, a slotted inline view) and the generated
//! line break of a `br`, none of which renders text. [`Paragraph`] maps one
//! to the other.

use rustc_hash::FxHashMap;
use terminus::link::NodeId;

use super::build::{AFTER_CLOSE, AFTER_OPEN, BEFORE_OPEN, IMAGE_TAG, MARKER_TAG, Newlines};
use super::parse::utf16_len;
use crate::main::tree::LynxDocument;

pub(super) enum Step<'a> {
    Enter(NodeId),
    Text(&'a str),
    Leave,
}

pub(super) fn walk<'d>(
    document: &'d LynxDocument,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
    visit: &mut impl FnMut(Step<'d>),
) {
    let Some(node) = document.get(root) else {
        return;
    };
    for &child in node.child_ids() {
        walk_node(document, child, newlines, visit);
    }
}

fn walk_node<'d>(
    document: &'d LynxDocument,
    id: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
    visit: &mut impl FnMut(Step<'d>),
) {
    let Some(node) = document.get(id) else {
        return;
    };
    if node.is_text_node() {
        visit(Step::Text(node.text().unwrap_or_default()));
        return;
    }
    if node.tag_name() == Some(MARKER_TAG) {
        return;
    }
    let breaks = newlines.get(&id).copied().unwrap_or(0);
    if breaks & BEFORE_OPEN != 0 {
        visit(Step::Text("\n"));
    }
    visit(Step::Enter(id));
    if breaks & AFTER_OPEN != 0 {
        visit(Step::Text("\n"));
    }
    for &child in node.child_ids() {
        walk_node(document, child, newlines, visit);
    }
    visit(Step::Leave);
    if breaks & AFTER_CLOSE != 0 {
        visit(Step::Text("\n"));
    }
}

pub(super) fn text_units(
    document: &LynxDocument,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
) -> Vec<u16> {
    let mut units = Vec::new();
    walk(document, root, newlines, &mut |step| {
        if let Step::Text(text) = step {
            units.extend(text.encode_utf16());
        }
    });
    units
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Paragraph {
    pub(super) element: NodeId,
    pub(super) start: usize,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment {
    units: usize,
    text: bool,
}

impl Paragraph {
    pub(super) fn len(&self) -> usize {
        self.segments
            .iter()
            .filter(|segment| segment.text)
            .map(|segment| segment.units)
            .sum()
    }

    pub(super) fn units(&self) -> usize {
        self.segments.iter().map(|segment| segment.units).sum()
    }

    pub(super) fn unit_at(&self, offset: usize) -> usize {
        let mut unit = 0;
        let mut position = 0;
        for segment in &self.segments {
            if segment.text {
                if offset <= position + segment.units {
                    return unit + offset.saturating_sub(position);
                }
                position += segment.units;
            }
            unit += segment.units;
        }
        unit
    }

    pub(super) fn offset_of(&self, unit: usize) -> usize {
        let mut first = 0;
        let mut position = 0;
        for segment in &self.segments {
            if unit < first + segment.units {
                return if segment.text {
                    position + (unit - first)
                } else {
                    position
                };
            }
            first += segment.units;
            if segment.text {
                position += segment.units;
            }
        }
        position
    }
}

pub(super) fn paragraphs(
    document: &LynxDocument,
    root: NodeId,
    newlines: &FxHashMap<NodeId, Newlines>,
) -> Vec<Paragraph> {
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut open: Option<(Paragraph, usize)> = None;
    let mut position = 0;
    walk(document, root, newlines, &mut |step| match step {
        Step::Enter(element) => {
            if let Some((paragraph, depth)) = open.as_mut() {
                *depth += 1;
                let atom = atom_units(document, element);
                if atom > 0 {
                    paragraph.segments.push(Segment {
                        units: atom,
                        text: false,
                    });
                }
            } else if document.paragraph_of(element) == Some(element) {
                open = Some((
                    Paragraph {
                        element,
                        start: position,
                        segments: Vec::new(),
                    },
                    0,
                ));
            }
        }
        Step::Text(text) => {
            let units = utf16_len(text);
            position += units;
            if let Some((paragraph, _)) = open.as_mut() {
                paragraph.segments.push(Segment { units, text: true });
            }
        }
        Step::Leave => {
            if let Some((_, depth)) = open.as_mut() {
                if *depth == 0 {
                    let (paragraph, _) = open.take().expect("open above");
                    paragraphs.push(paragraph);
                } else {
                    *depth -= 1;
                }
            }
        }
    });
    paragraphs
}

fn atom_units(document: &LynxDocument, element: NodeId) -> usize {
    let range_len = |node: NodeId| {
        document
            .inline_source_range(node)
            .map_or(0, |range| range.len())
    };
    match document
        .get(element)
        .and_then(terminus::link::Node::tag_name)
    {
        Some(IMAGE_TAG | "br") => range_len(element),
        Some("slot") => document
            .assigned_nodes(element)
            .iter()
            .copied()
            .map(range_len)
            .sum(),
        _ => 0,
    }
}

pub(super) fn rendered_prefix(source: &[u16], rendered: &[u16], source_offset: usize) -> usize {
    let mut rendered_index = 0;
    for &unit in &source[..source_offset.min(source.len())] {
        if rendered_index >= rendered.len() {
            break;
        }
        if unit == rendered[rendered_index] {
            rendered_index += 1;
        }
    }
    rendered_index
}

pub(super) fn source_offset(source: &[u16], rendered: &[u16], offset: usize) -> Option<usize> {
    let mut rendered_index = 0;
    for (index, &unit) in source.iter().enumerate() {
        if rendered_index >= rendered.len() {
            break;
        }
        if unit == rendered[rendered_index] {
            if rendered_index == offset {
                return Some(index);
            }
            rendered_index += 1;
        }
    }
    None
}
