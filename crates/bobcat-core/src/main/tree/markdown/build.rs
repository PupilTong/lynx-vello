//! The shadow-tree elements one [`super::parse::Block`] renders as: the elements
//! markdown-it's renderer writes for it, by their HTML names, as the browser
//! builds them from that HTML under web-core
//! (`XMarkdownAttributes.ts:743`, `root.innerHTML = sanitizeHtml(...)`).
//!
//! Two kinds of element are this engine's own and have no counterpart in
//! web-core's tree:
//!
//! - [`LINE_TAG`] holds inline content markdown-it writes straight into its container with no
//!   element of its own — a tight list item's text, or an HTML block's content outside any `<p>`
//!   ([`super::parse::Block::Inlines`]). A browser gives that text an anonymous block; a paragraph
//!   here is an element, so the line is one.
//! - [`MARKER_TAG`] is a list item's marker, the browser's `::marker` box, which is no node there
//!   either.
//!
//! Neither is reported by any UI method: the rendered text skips the marker
//! and sees through the line ([`super::rendered`]), and neither name matches
//! a tag `getParseResult` is asked about.
//!
//! # The line breaks between block tags
//!
//! markdown-it writes `"\n"` after most block tags (`renderToken`,
//! `markdown-it/lib/renderer.mjs`), and the browser keeps each as a text node,
//! so they are part of the shadow root's `textContent` that `getParseResult`,
//! `getTextBoundingRect` and `getCharIndexByPoint` count offsets in. They
//! render as nothing, so they are not nodes here; each element records which
//! of them markdown-it wrote around it ([`Newlines`]), and the rendered-text
//! walk counts them where the browser's tree has them. `renderToken`'s rule,
//! per token:
//!
//! - A block tag's close tag is followed by one, and so is a block tag with no close tag (`<hr>`).
//!   [`AFTER_CLOSE`].
//! - A block open tag is followed by one unless the next token is inline content (a paragraph's, a
//!   heading's, a cell's), a hidden paragraph (a tight list item's text), or the same tag's close
//!   (an empty item). [`AFTER_OPEN`].
//! - A block open tag right after a hidden paragraph is preceded by one: a tight item's text before
//!   a nested list. [`BEFORE_OPEN`].
//!
//! Fences and code blocks are written by their own rule, `<pre><code>…</code></pre>\n`, so a `pre`
//! is [`AFTER_CLOSE`] only and never [`BEFORE_OPEN`]. A soft break is a `"\n"` text node and a hard
//! break `<br>\n`; both are real text nodes here, inside their paragraph.
//!
//! An HTML block is written as it is: its content, kept and sanitized,
//! ends with its last line's line break, which the browser keeps as a text
//! node after the block's elements. [`super::parse`] keeps that text, and the
//! white space between the block's `<p>`s, as
//! [`super::parse::Block::HtmlSpace`], which is a text node here too.

use dom::NodeId;
use rustc_hash::FxHashMap;

use super::parse::{Alignment, Block, HtmlElement, Inline, ListItem, TableRow};
use crate::main::tree::LynxDocument;

/// The engine's element for inline content that has no element of its own;
/// see the module documentation.
pub(super) const LINE_TAG: &str = "md-line";
/// The engine's element for a list item's marker; see the module
/// documentation.
pub(super) const MARKER_TAG: &str = "md-marker";
/// The engine's element for an image, where web-core's tree has an `<img>`.
pub(super) const IMAGE_TAG: &str = "image";
/// The class web-core gives the `<span>` an inline view's slot sits in
/// (`#injectInlineViews`, `XMarkdownAttributes.ts:1284-1304`).
pub(super) const INLINE_VIEW_CLASS: &str = "md-inline-view";

/// Which of markdown-it's block line breaks are written around one element,
/// as [`AFTER_CLOSE`], [`AFTER_OPEN`] and [`BEFORE_OPEN`] bits.
pub(super) type Newlines = u8;
/// A `"\n"` after the element's close tag, in its parent.
pub(super) const AFTER_CLOSE: Newlines = 1;
/// A `"\n"` right after the element's open tag, its own first text.
pub(super) const AFTER_OPEN: Newlines = 1 << 1;
/// A `"\n"` right before the element's open tag, in its parent.
pub(super) const BEFORE_OPEN: Newlines = 1 << 2;

/// Builds blocks into a shadow tree and records their line breaks.
pub(super) struct Builder<'a> {
    pub(super) document: &'a mut LynxDocument,
    pub(super) newlines: &'a mut FxHashMap<NodeId, Newlines>,
}

impl Builder<'_> {
    /// Builds `block` as the last child of `parent` and answers its element.
    /// `lists` is how many lists enclose it, which picks an unordered list's
    /// marker; `after_hidden` is whether the block before it in the same
    /// list item was that item's hidden paragraph.
    pub(super) fn block(
        &mut self,
        parent: NodeId,
        block: &Block,
        lists: usize,
        after_hidden: bool,
    ) -> NodeId {
        // Every block markdown-it writes through `renderToken` takes the
        // line break before it after a hidden paragraph.
        let before = if after_hidden { BEFORE_OPEN } else { 0 };
        match block {
            Block::Paragraph { children, .. } => {
                let paragraph = self.element(parent, "p", AFTER_CLOSE | before);
                self.inlines(paragraph, children);
                paragraph
            }
            Block::Heading {
                level, children, ..
            } => {
                let tag = match level {
                    1 => "h1",
                    2 => "h2",
                    3 => "h3",
                    4 => "h4",
                    5 => "h5",
                    _ => "h6",
                };
                let heading = self.element(parent, tag, AFTER_CLOSE | before);
                self.inlines(heading, children);
                heading
            }
            Block::Quote { children, .. } => {
                let open = if children.is_empty() { 0 } else { AFTER_OPEN };
                let quote = self.element(parent, "blockquote", AFTER_CLOSE | open | before);
                for child in children {
                    self.block(quote, child, lists, false);
                }
                quote
            }
            Block::List { start, items, .. } => {
                let tag = if start.is_some() { "ol" } else { "ul" };
                let list = self.element(parent, tag, AFTER_CLOSE | AFTER_OPEN | before);
                if let Some(start) = start.filter(|start| *start != 1) {
                    self.document
                        .set_attribute(list, "start", &start.to_string());
                }
                for (index, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(start) => format!(
                            "{}.",
                            start.saturating_add(u64::try_from(index).unwrap_or(u64::MAX))
                        ),
                        None => match lists {
                            0 => "\u{2022}",
                            1 => "\u{25e6}",
                            _ => "\u{25aa}",
                        }
                        .to_owned(),
                    };
                    self.list_item(list, item, &marker, lists + 1);
                }
                list
            }
            Block::Code { language, text, .. } => {
                let pre = self.element(parent, "pre", AFTER_CLOSE);
                let code = self.element(pre, "code", 0);
                if let Some(language) = language {
                    self.document
                        .set_classes(code, &format!("language-{language}"));
                }
                self.text(code, text);
                pre
            }
            Block::HtmlParagraph {
                classes, children, ..
            } => {
                let paragraph = self.element(parent, "p", 0);
                self.classes(paragraph, classes);
                self.inlines(paragraph, children);
                paragraph
            }
            Block::Inlines { children, .. } => {
                let line = self.element(parent, LINE_TAG, 0);
                self.inlines(line, children);
                line
            }
            // A text node of the container, as in the browser's tree: it
            // lays out nothing outside a paragraph, and is counted in the
            // rendered text.
            Block::HtmlSpace { text, .. } => {
                let node = self.document.create_text_node(text.as_str(), ());
                self.document.append_child(parent, node);
                node
            }
            Block::Table {
                alignments,
                head,
                body,
                ..
            } => self.table(parent, alignments, head, body, before),
            Block::ThematicBreak { .. } => self.element(parent, "hr", AFTER_CLOSE | before),
        }
    }

    fn list_item(&mut self, list: NodeId, item: &ListItem, marker: &str, lists: usize) {
        // A tight item's paragraph is hidden, and markdown-it then writes no
        // line break after `<li>`; an empty item is `<li></li>`.
        let open = match item.children.first() {
            Some(Block::Inlines { .. }) | None => 0,
            Some(_) => AFTER_OPEN,
        };
        let element = self.element(list, "li", AFTER_CLOSE | open);
        let marker_element = self.element(element, MARKER_TAG, 0);
        self.text(marker_element, marker);
        let mut after_hidden = false;
        for child in &item.children {
            self.block(element, child, lists, after_hidden);
            after_hidden = matches!(child, Block::Inlines { .. });
        }
    }

    fn table(
        &mut self,
        parent: NodeId,
        alignments: &[Alignment],
        head: &TableRow,
        body: &[TableRow],
        before: Newlines,
    ) -> NodeId {
        let table = self.element(parent, "table", AFTER_CLOSE | AFTER_OPEN | before);
        // hughie has no `repeat()`: one `auto` track per column.
        let columns = head.cells.len().max(1);
        let tracks = vec!["auto"; columns].join(" ");
        self.document
            .set_inline_style(table, &format!("grid-template-columns: {tracks}"));
        let thead = self.element(table, "thead", AFTER_CLOSE | AFTER_OPEN);
        self.row(thead, head, "th", alignments);
        if !body.is_empty() {
            let tbody = self.element(table, "tbody", AFTER_CLOSE | AFTER_OPEN);
            for row in body {
                self.row(tbody, row, "td", alignments);
            }
        }
        table
    }

    fn row(&mut self, parent: NodeId, row: &TableRow, cell_tag: &str, alignments: &[Alignment]) {
        let tr = self.element(parent, "tr", AFTER_CLOSE | AFTER_OPEN);
        for (index, cell) in row.cells.iter().enumerate() {
            let element = self.element(tr, cell_tag, AFTER_CLOSE);
            // markdown-it's `style="text-align:<a>"` on every cell of an
            // aligned column.
            let align = match alignments.get(index) {
                Some(Alignment::Left) => Some("left"),
                Some(Alignment::Center) => Some("center"),
                Some(Alignment::Right) => Some("right"),
                Some(Alignment::None) | None => None,
            };
            if let Some(align) = align {
                self.document
                    .set_inline_style(element, &format!("text-align: {align}"));
            }
            self.inlines(element, &cell.children);
        }
    }

    fn inlines(&mut self, parent: NodeId, inlines: &[Inline]) {
        for inline in inlines {
            self.inline(parent, inline);
        }
    }

    fn inline(&mut self, parent: NodeId, inline: &Inline) {
        match inline {
            Inline::Text { text, .. } => self.text(parent, text),
            Inline::Emphasis { children, .. } => {
                let element = self.element(parent, "em", 0);
                self.inlines(element, children);
            }
            Inline::Strong { children, .. } => {
                let element = self.element(parent, "strong", 0);
                self.inlines(element, children);
            }
            Inline::Strikethrough { children, .. } => {
                let element = self.element(parent, "s", 0);
                self.inlines(element, children);
            }
            Inline::Code { text, .. } => {
                let element = self.element(parent, "code", 0);
                self.text(element, text);
            }
            Inline::Link {
                href,
                title,
                children,
                ..
            } => {
                let element = self.element(parent, "a", 0);
                self.document.set_attribute(element, "href", href);
                if let Some(title) = title {
                    self.document.set_attribute(element, "title", title);
                }
                self.inlines(element, children);
            }
            Inline::Image {
                src, alt, title, ..
            } => {
                let element = self.element(parent, IMAGE_TAG, 0);
                self.document.set_attribute(element, "auto-size", "");
                self.document.set_attribute(element, "alt", alt);
                if let Some(title) = title {
                    self.document.set_attribute(element, "title", title);
                }
                // Last, so the image component binds the source once.
                self.document.set_attribute(element, "src", src);
            }
            Inline::InlineView { id, .. } => {
                let container = self.element(parent, "span", 0);
                self.document.set_classes(container, INLINE_VIEW_CLASS);
                let slot = self.element(container, "slot", 0);
                self.document.set_attribute(slot, "name", id);
            }
            Inline::SoftBreak { .. } => self.text(parent, "\n"),
            Inline::HardBreak { .. } => {
                self.element(parent, "br", 0);
                self.text(parent, "\n");
            }
            Inline::Html {
                element,
                classes,
                children,
                ..
            } => {
                let tag = match element {
                    HtmlElement::Span => "span",
                    HtmlElement::P => "p",
                };
                let element = self.element(parent, tag, 0);
                self.classes(element, classes);
                self.inlines(element, children);
            }
        }
    }

    fn element(&mut self, parent: NodeId, tag: &str, newlines: Newlines) -> NodeId {
        let element = self.document.create_element(tag, ());
        self.document.append_child(parent, element);
        if newlines != 0 {
            self.newlines.insert(element, newlines);
        }
        element
    }

    fn text(&mut self, parent: NodeId, text: &str) {
        let node = self.document.create_text_node(text, ());
        self.document.append_child(parent, node);
    }

    fn classes(&mut self, element: NodeId, classes: &[String]) {
        if !classes.is_empty() {
            self.document.set_classes(element, &classes.join(" "));
        }
    }
}

/// Forgets the line breaks recorded for `element` and everything under it,
/// before the subtree is freed.
pub(super) fn forget(
    document: &LynxDocument,
    newlines: &mut FxHashMap<NodeId, Newlines>,
    element: NodeId,
) {
    let mut stack = vec![element];
    while let Some(node) = stack.pop() {
        newlines.remove(&node);
        if let Some(node) = document.get(node) {
            stack.extend(node.child_ids().iter().copied());
        }
    }
}

/// The tag `getParseResult` matches an element of our tree under: its HTML
/// name, `img` for the engine's `image`, and none for the engine's own
/// [`LINE_TAG`] and [`MARKER_TAG`].
pub(super) fn html_name(tag: &str) -> Option<&str> {
    match tag {
        IMAGE_TAG => Some("img"),
        LINE_TAG | MARKER_TAG => None,
        tag => Some(tag),
    }
}
