//! The standard `<svg>` element, as a subset: an inline SVG root whose
//! subtree is drawn as one vector image.
//!
//! # What a root is
//!
//! An element named `svg` is replaced content from its creation
//! ([`Document::create_element`] gives it [`NaturalSize::NONE`]), so layout
//! treats it as a leaf and its children generate no boxes. Its descendants
//! are ordinary DOM nodes: script creates and mutates them as it does any
//! element, and selectors match them, but the engine's cascade does not
//! style the picture.
//!
//! The *root* a node belongs to is found by walking up from it (a text node
//! starts at its parent) for as long as each element's name is one `usvg`
//! reads ([`svg_markup::is_svg_element_name`]); the root is the topmost `svg`
//! met on that walk. A nested `svg` is therefore part of its outer root's
//! document, never a root of its own, and a mutation under an element
//! `usvg` does not know (and drops with its subtree) is not tracked at all.
//! The element set is `usvg`'s, not the Lynx tag set, so it contains `text`
//! and `image`: an `svg` inside a Lynx `<text>` or `<image>` is still a root,
//! because the walk passes through those two names and finds no `svg` above
//! them. Shadow trees are not part of the walk: it stops at a shadow root.
//!
//! # When a root re-renders
//!
//! Every mutation that can change a root's markup marks it: an attribute,
//! class, id or `style` set or removed on the root or a descendant, a text
//! node's data changed, a child inserted, moved or removed. The marks are a
//! plain list, deduplicated only against the last push; the gate in front of
//! them is the count of live `svg` elements, so a document with none pays
//! one integer test per mutation. [`Document::refresh_inline_svgs`], at the
//! start of every [`Document::layout`], resolves each mark to its current
//! root (the tree may have moved since it was made), deduplicates, and
//! renders each root once:
//!
//! 1. serialise the subtree ([`svg_markup::serialize`]);
//! 2. parse it with [`ImageEvent::parse_document`], the parse a host's reported document goes
//!    through, inline on the document thread (browsers parse inline SVG on their main thread as
//!    well);
//! 3. store the result in the [`ImageRegistry`](crate::render::image) under a synthetic source,
//!    `inline-svg:<node>:<generation>`, created already settled, so no paint walk ever queues it
//!    and the host is never asked for it;
//! 4. bind the root to that source as its [`ImageRole::Source`]. Its outcome is ignored: the
//!    standard `<svg>` fires no `load`;
//! 5. forget the previous generation's entry. Synthetic entries are the one kind the registry
//!    removes; a host source still never regresses.
//!
//! From there the existing replaced-image path draws it: the natural size
//! [`crate::VectorImage::parse`] reports goes to layout, and the vector
//! branch of the paint walk appends its cached scene. A document that does
//! not parse settles its source `Failed`, which draws nothing and fails no
//! commit.
//!
//! The root's own `width` and `height` attributes are presentational hints
//! for CSS `width` and `height` (`Document::set_attribute`), so author CSS
//! still overrides them.

use std::sync::Arc;

use crate::layout::NaturalSize;
use crate::render::image::{
    DocumentKind, ImageEvent, ImageRole, SYNTHETIC_SOURCE_PREFIX, is_synthetic_source,
};
use crate::tree::document::{Document, NodeId};
use crate::tree::svg_markup;

const SVG_TAG: &str = "svg";

/// A document's inline SVG bookkeeping.
#[derive(Debug, Default)]
pub(crate) struct InlineSvgs {
    /// Live elements named `svg`: zero skips every walk.
    live: usize,
    /// Roots (or moved `svg` elements) marked since the last refresh.
    dirty: Vec<NodeId>,
    /// The last generation a synthetic source was minted with.
    generation: u64,
}

/// Whether `node` is an element named `svg`.
fn is_svg<T>(node: &crate::Node<T>) -> bool {
    node.is_element() && node.tag_name() == Some(SVG_TAG)
}

impl<T> Document<T> {
    /// Counts a new element and, for an `svg`, makes it replaced content
    /// with no natural size until its first refresh.
    pub(crate) fn note_element_created_for_inline_svg(&mut self, id: NodeId) {
        if !self.get(id).is_some_and(is_svg) {
            return;
        }
        self.inline_svgs.live += 1;
        self.live_node_mut(id).set_natural_size(NaturalSize::NONE);
    }

    /// Uncounts a freed `svg` and forgets its synthetic source.
    pub(crate) fn note_node_freed_for_inline_svg(&mut self, node: &crate::Node<T>) {
        if !is_svg(node) {
            return;
        }
        self.inline_svgs.live -= 1;
        if let Some(source) = node
            .image_source(ImageRole::Source)
            .filter(|source| is_synthetic_source(source))
        {
            self.images.forget_synthetic(source);
        }
    }

    /// The inline SVG root `id` belongs to, if any (the module docs give
    /// the rule).
    pub(crate) fn inline_svg_root(&self, id: NodeId) -> Option<NodeId> {
        let mut node = self.get(id)?;
        if node.is_text_node() {
            node = self.get(node.parent_id()?)?;
        }
        let mut root = None;
        while let Some(name) = node
            .tag_name()
            .filter(|&name| node.is_element() && svg_markup::is_svg_element_name(name))
        {
            if name == SVG_TAG {
                root = Some(node.id());
            }
            let Some(parent) = node.parent_id().and_then(|parent| self.get(parent)) else {
                break;
            };
            node = parent;
        }
        root
    }

    /// Marks the root `id` belongs to for a refresh.
    pub(crate) fn note_inline_svg_mutation(&mut self, id: NodeId) {
        if self.inline_svgs.live == 0 {
            return;
        }
        if let Some(root) = self.inline_svg_root(id) {
            self.mark_inline_svg(root);
        }
    }

    /// Marks an `svg` element that was just inserted, as itself rather than
    /// as its root: the refresh re-resolves it, and when it turned out to be
    /// nested it lets go of the source it held as a root of its own.
    pub(crate) fn note_svg_inserted(&mut self, child: NodeId) {
        if self.inline_svgs.live != 0 && self.get(child).is_some_and(is_svg) {
            self.mark_inline_svg(child);
        }
    }

    fn mark_inline_svg(&mut self, id: NodeId) {
        if self.inline_svgs.dirty.last() != Some(&id) {
            self.inline_svgs.dirty.push(id);
        }
        self.note_visual_mutation();
    }

    /// Re-renders every marked root (the module docs give the steps).
    pub(crate) fn refresh_inline_svgs(&mut self) {
        if self.inline_svgs.dirty.is_empty() {
            return;
        }
        let marks = std::mem::take(&mut self.inline_svgs.dirty);
        let mut roots = Vec::with_capacity(marks.len());
        for id in marks {
            let root = self.inline_svg_root(id);
            if root != Some(id) {
                self.release_nested_svg(id);
            }
            roots.extend(root);
        }
        // `NodeId` is not `Ord`; its bits order it.
        roots.sort_unstable_by_key(|id| id.to_bits());
        roots.dedup();
        for root in roots {
            self.render_inline_svg(root);
        }
    }

    /// Takes the synthetic source off an `svg` that was a root and is now
    /// nested, keeping it replaced content.
    fn release_nested_svg(&mut self, id: NodeId) {
        let Some(node) = self.get(id).filter(|node| is_svg(node)) else {
            return;
        };
        let Some(source) = node
            .image_source(ImageRole::Source)
            .filter(|source| is_synthetic_source(source))
            .map(Box::<str>::from)
        else {
            return;
        };
        let _ = self.set_image_source(id, ImageRole::Source, None);
        self.set_natural_size(id, NaturalSize::NONE);
        self.images.forget_synthetic(&source);
    }

    /// Serialises, parses and rebinds one root.
    fn render_inline_svg(&mut self, root: NodeId) {
        let markup = svg_markup::serialize(self, root);
        self.inline_svgs.generation += 1;
        let source: Arc<str> = Arc::from(format!(
            "{SYNTHETIC_SOURCE_PREFIX}{root}:{}",
            self.inline_svgs.generation
        ));
        let event =
            ImageEvent::parse_document(Arc::clone(&source), markup.as_bytes(), DocumentKind::Svg);
        self.images.insert_synthetic(&event);
        let previous = self
            .get(root)
            .and_then(|node| node.image_source(ImageRole::Source))
            .filter(|source| is_synthetic_source(source))
            .map(Box::<str>::from);
        // No `load`: the standard element fires none, so the outcome is
        // dropped.
        let _ = self.set_image_source(root, ImageRole::Source, Some(&source));
        if let Some(previous) = previous {
            self.images.forget_synthetic(&previous);
        }
    }

    /// Reflects an `svg` element's `width` or `height` attribute as the
    /// presentational hint of the same name. A bare number is px; anything
    /// else is handed to the CSS parser as written, and an invalid value
    /// leaves no hint.
    pub(crate) fn reflect_svg_size_attribute(
        &mut self,
        id: NodeId,
        name: &str,
        value: Option<&str>,
    ) {
        if !matches!(name, "width" | "height") || !self.get(id).is_some_and(is_svg) {
            return;
        }
        // Cleared first, so a value the parser refuses does not leave the
        // previous one standing.
        self.set_presentational_hint(id, name, "");
        let Some(value) = value else {
            return;
        };
        // A bare number is written from the parsed value, not by appending
        // `px` to the text: `5.` parses as a number and is no CSS one, and
        // `inf` or `NaN` must never reach a declaration. `<blur-view>`'s
        // `blur-radius` reflection in `bobcat-core` does the same.
        match value.trim().parse::<f32>() {
            Ok(number) if number.is_finite() => {
                self.set_presentational_hint(id, name, &format!("{number}px"));
            }
            _ => self.set_presentational_hint(id, name, value),
        }
    }
}
