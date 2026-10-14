//! `<markdown>`'s source parser: the `content` attribute, read the way
//! web-core reads it, into an owned [`MarkdownTree`] the component turns into
//! shadow-tree elements.
//!
//! # What web-core does
//!
//! web-core, in
//! `lynx-stack/packages/web-platform/web-elements/src/elements/XMarkdown/XMarkdownAttributes.ts`,
//! parses with markdown-it 15 created as `{html: true, linkify: true}` and then
//! `enable(['table', 'strikethrough'])` (`getMarkdownParser`, `:138-151`), rewrites every raw-HTML
//! token through `sanitizeAllowedHtml` (`applyRawHtmlPolicy`, `:118-127`, and `:74-116`), renders,
//! runs `DOMPurify` with its HTML profile over the result (`sanitizeHtml`, `:160-166`), and
//! replaces each image whose source is `inlineview://<id>` with a slot
//! (`preprocessInlineView`, `:379-383`, and `#injectInlineViews`,
//! `:1284-1304`). The whole of that is replicated here as data: the tree
//! below holds what the browser's DOM would hold after those steps, and the
//! component builds the same elements from it.
//!
//! # Parser options
//!
//! [`pulldown_cmark`] is `CommonMark`, as markdown-it is. Of its extensions only
//! [`Options::ENABLE_TABLES`] and [`Options::ENABLE_STRIKETHROUGH`] are on,
//! the two rules web-core enables (markdown-it's `default` preset already has
//! both). Everything else stays off because markdown-it 15 has no such rule,
//! or has it off by default: footnotes, task lists, math, definition lists,
//! super- and subscript, wiki links and GFM blockquote tags (no markdown-it
//! rule); smart punctuation (markdown-it's `typographer`, off); heading
//! attributes and YAML or `+++` metadata blocks (no rule). markdown-it's
//! `breaks: false` is `CommonMark`'s own soft break.
//!
//! pulldown-cmark's strikethrough also accepts a single `~`; markdown-it's
//! (`src/rules_inline/strikethrough.ts`) needs `~~`, so a single-tilde span
//! is put back as text with its tildes.
//!
//! # Links
//!
//! Every link and image destination goes through markdown-it's
//! `normalizeLink` and `validateLink` (`src/index.ts`): percent-encoding by
//! `mdurl.encode` ([`normalize_link`]), and refusal of `javascript:`,
//! `vbscript:`, `file:` and `data:` other than `data:image/(gif|png|jpeg|webp);`
//! ([`validate_link`]). markdown-it then does not parse the construct as a
//! link at all, so a refused link or image is its source text again, with
//! its label still parsed as inline content. The link text of an autolink is
//! `normalizeLinkText`, which percent-decodes ([`normalize_link_text`]).
//! `normalizeLink` also converts a host name to punycode and
//! `normalizeLinkText` back to Unicode; neither is done here, so an `href`
//! with a non-ASCII host keeps it percent-encoded instead.
//!
//! Bare links are markdown-it's `linkify`: its inline rule for
//! `scheme://` (`src/rules_inline/linkify.ts`) and its core rule over text
//! tokens (`src/rules_core/linkify.ts`), both over linkify-it 6 with its
//! defaults. Those defaults are the schemes `http:`, `https:`, `ftp:`, `//`
//! and `mailto:`, `fuzzyEmail: true`, and **`fuzzyLink: false`**: web-core
//! links `https://example.com` and `a@b.com` but not `www.example.com` or
//! `example.com`. The [`linkify`] crate finds the candidates here (see
//! [`Builder::find_links`]); a URL is kept when its scheme is `http`,
//! `https` or `ftp` (any case), an email address always, with a preceding
//! `mailto:` taken into the link. Not matched: linkify-it's scheme-relative
//! `//host` links, which the crate does not find; and the two libraries'
//! own rules for where a link ends, which agree on the common cases
//! (trailing punctuation, balanced parentheses) and are not the same code.
//! markdown-it's inline rule runs before emphasis, so `http://a.com/_b_` is
//! one link there; here emphasis is parsed first and the `_b_` is emphasis.
//! No text inside a link, an image description, a code span or a code block
//! is linked, nor text between a raw `<a ...>` and `</a>` (markdown-it's
//! `isLinkOpen`/`isLinkClose`, counted forward).
//!
//! # Raw HTML
//!
//! An HTML fragment is one inline HTML construct (markdown-it's
//! `html_inline`, pulldown-cmark's [`Event::InlineHtml`]) or the whole of an
//! HTML block (`html_block`, the [`Event::Html`] lines of one
//! [`Tag::HtmlBlock`]). `sanitizeAllowedHtml` (`:74-116`) keeps a fragment
//! only when every tag in it is `<span>` or `<p>` and it holds no `<!`, `<?`
//! or `<%`; otherwise the whole fragment is text, exactly as written. A kept
//! tag keeps only its `class` attribute, reduced to the tokens matching
//! `^[A-Za-z0-9_-]+$` (`normalizeClassList`, `:68-72`). `DOMPurify`, which web-core
//! runs next, has nothing left to remove.
//!
//! pulldown-cmark does not pair tags, so the fragments are tokenized here and
//! paired the way the HTML tree builder pairs them, for these two tags:
//! a `<p>` start tag closes an open `p`; `</p>` closes the open `p` and
//! every `span` inside it, and with no `p` open inserts an empty one; and
//! `</span>` closes the nearest open `span` unless a `p` is open inside it,
//! in which case it is ignored (the tree builder's "any other end tag" step
//! stops at a special element). A tag never closes anything outside the
//! Markdown construct it was written in, and a tag left open is closed where
//! that construct ends — the end of the HTML block, or the end of the
//! paragraph, heading, cell, emphasis or link around an inline tag. (A
//! browser would let an unclosed tag of an HTML block run on into the
//! Markdown after it, and would reopen formatting elements an inline end tag
//! crossed; neither is replicated.) An inline `<p>` stays an inline
//! [`Inline::Html`] where the browser would split the paragraph around it. A
//! stray inline end tag is ignored. Text inside an HTML block has its
//! character references decoded, as the browser does, for the numeric forms
//! and `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` and `&nbsp;` only (see
//! [`decode_entities`]).
//!
//! markdown-it keeps an HTML block's last line break in its content, and the
//! browser keeps it as text: inside a `<p>` left open, at the end of a
//! refused block's text, and otherwise a text node after the block's
//! elements. So is the white space outside every element and run of content
//! between them. Those text nodes are [`Block::HtmlSpace`], which render
//! nothing and are part of `textContent`.
//!
//! # Offsets
//!
//! Every node carries its source range as UTF-16 code-unit offsets into the
//! full `content` string — JavaScript string indices, the unit web-core's
//! `content-range` slices by (`:727`, `:740`) — even when only a
//! `content-range` slice was parsed. [`Inline::append_text_content`] and its
//! block and tree counterparts give the text each node renders, which is
//! the node's `textContent` in web-core's shadow tree, with one difference:
//! web-core's tree also holds the `"\n"` text nodes markdown-it's renderer
//! writes between block tags (`</p>\n<p>`, `<li>\n<p>` in a loose list),
//! which are not nodes here.
//!
//! # Nesting
//!
//! Containers deeper than [`MAX_NESTING`] are dropped with their contents,
//! markdown-it's `maxNesting: 100` (`src/presets/default.ts`), which drops
//! them too, so no tree built here is deeper than that and dropping or
//! comparing one stays within the stack.

use std::ops::Range;

use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{
    Alignment as CmarkAlignment, CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag,
};

use super::style::is_js_space;

/// A node's place in the `content` attribute: UTF-16 code units, start
/// inclusive, end exclusive, counted from the start of the whole attribute.
pub(super) type SourceRange = Range<usize>;

/// How many containers deep the tree may go; deeper ones are dropped.
pub(super) const MAX_NESTING: usize = 100;

/// The image source that names a light-DOM child instead of an image
/// (`preprocessInlineView`, `XMarkdownAttributes.ts:379-383`).
const INLINE_VIEW_SCHEME: &str = "inlineview://";

/// One parse of the `content` attribute: the blocks the component builds
/// its shadow tree from. Not a DOM document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MarkdownTree {
    pub(super) blocks: Vec<Block>,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the component counts rendered text off its shadow tree; this module's tests check the parse's"
    )
)]
impl MarkdownTree {
    /// The text the whole tree renders, in tree order.
    pub(super) fn text_content(&self) -> String {
        let mut text = String::new();
        for block in &self.blocks {
            block.append_text_content(&mut text);
        }
        text
    }
}

/// A block-level node. Each variant names the element markdown-it renders
/// for it, which is the element the component creates.
///
/// Equality compares the whole subtree, ranges included: the component
/// keeps the elements of every leading block equal to the previous parse's,
/// which is how a streaming append leaves the earlier elements in place
/// (web-core's `#appendIncrementally`, `XMarkdownAttributes.ts:759-782`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Block {
    /// `<p>`.
    Paragraph {
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// `<h1>` to `<h6>`; `level` is 1 to 6.
    Heading {
        level: u8,
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// `<blockquote>`.
    Quote {
        children: Vec<Block>,
        range: SourceRange,
    },
    /// `<ol>` when `start` is `Some`, `<ul>` otherwise. markdown-it writes
    /// `start="n"` on an `<ol>` whose first number is not 1.
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
        range: SourceRange,
    },
    /// `<pre><code>`, with `class="language-<language>"` on the `<code>`
    /// when the fence's info string has a first word. `text` ends with the
    /// last line's line break.
    Code {
        language: Option<String>,
        text: String,
        range: SourceRange,
    },
    /// A `<p>` written as raw HTML, with the classes it kept.
    HtmlParagraph {
        classes: Vec<String>,
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// Inline content placed straight in its container with no element of
    /// its own: a tight list item's text (markdown-it hides a tight item's
    /// paragraph), or the content of an HTML block outside any `<p>` — the
    /// whole text of a refused block, or a `<span>`.
    Inlines {
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// White space an HTML block holds in its container outside every
    /// element and every run of content — the line break its last line ends
    /// with, the line breaks between two of its `<p>`s: a text node of the
    /// container, which renders nothing and is part of its `textContent`.
    HtmlSpace { text: String, range: SourceRange },
    /// `<table>` with a `<thead>` holding `head` in one `<tr>` of `<th>`,
    /// and a `<tbody>` of `<tr>`s of `<td>` only when `body` is not empty.
    /// Every cell of column `i` carries `style="text-align:<a>"` when
    /// `alignments[i]` is not [`Alignment::None`].
    Table {
        alignments: Vec<Alignment>,
        head: TableRow,
        body: Vec<TableRow>,
        range: SourceRange,
    },
    /// `<hr>`.
    ThematicBreak { range: SourceRange },
}

/// An `<li>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ListItem {
    pub(super) children: Vec<Block>,
    pub(super) range: SourceRange,
}

/// A `<tr>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TableRow {
    pub(super) cells: Vec<TableCell>,
    pub(super) range: SourceRange,
}

/// A `<th>` in the head row, a `<td>` in a body row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TableCell {
    pub(super) children: Vec<Inline>,
    pub(super) range: SourceRange,
}

/// A table column's alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Alignment {
    None,
    Left,
    Center,
    Right,
}

/// An inline node.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "`InlineView` is the Lynx name of the construct"
)]
pub(super) enum Inline {
    /// A text node.
    Text { text: String, range: SourceRange },
    /// `<em>`.
    Emphasis {
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// `<strong>`.
    Strong {
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// `<s>`, the element markdown-it's strikethrough renders.
    Strikethrough {
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// `<code>`.
    Code { text: String, range: SourceRange },
    /// `<a href title>`; `href` is normalized and validated.
    Link {
        href: String,
        title: Option<String>,
        children: Vec<Inline>,
        range: SourceRange,
    },
    /// `<img src alt title>` in web-core, the engine's `image` here. `alt`
    /// is markdown-it's `renderInlineAsText` of the description.
    Image {
        src: String,
        alt: String,
        title: Option<String>,
        range: SourceRange,
    },
    /// `![...](inlineview://<id>)`: web-core's
    /// `<span class="md-inline-view"><slot name="<id>"></slot></span>`, the
    /// light-DOM child with `slot="<id>"` shown in the text.
    InlineView { id: String, range: SourceRange },
    /// A soft line break: a `"\n"` text node, which white space collapsing
    /// shows as a space.
    SoftBreak { range: SourceRange },
    /// `<br>` followed by a `"\n"` text node.
    HardBreak { range: SourceRange },
    /// A `<span>` or `<p>` written as raw HTML, with the classes it kept.
    Html {
        element: HtmlElement,
        classes: Vec<String>,
        children: Vec<Inline>,
        range: SourceRange,
    },
}

/// The two raw HTML elements content may keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HtmlElement {
    Span,
    P,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the component counts rendered text off its shadow tree; this module's tests check the parse's"
    )
)]
impl Block {
    pub(super) fn range(&self) -> &SourceRange {
        match self {
            Block::Paragraph { range, .. }
            | Block::Heading { range, .. }
            | Block::Quote { range, .. }
            | Block::List { range, .. }
            | Block::Code { range, .. }
            | Block::HtmlParagraph { range, .. }
            | Block::Inlines { range, .. }
            | Block::HtmlSpace { range, .. }
            | Block::Table { range, .. }
            | Block::ThematicBreak { range } => range,
        }
    }

    /// The text this block renders.
    pub(super) fn text_content(&self) -> String {
        let mut text = String::new();
        self.append_text_content(&mut text);
        text
    }

    /// Appends the text this block renders to `text`.
    pub(super) fn append_text_content(&self, text: &mut String) {
        match self {
            Block::Paragraph { children, .. }
            | Block::Heading { children, .. }
            | Block::HtmlParagraph { children, .. }
            | Block::Inlines { children, .. } => append_inlines(children, text),
            Block::Quote { children, .. } => {
                for block in children {
                    block.append_text_content(text);
                }
            }
            Block::List { items, .. } => {
                for item in items {
                    item.append_text_content(text);
                }
            }
            Block::Code { text: code, .. } | Block::HtmlSpace { text: code, .. } => {
                text.push_str(code);
            }
            Block::Table { head, body, .. } => {
                head.append_text_content(text);
                for row in body {
                    row.append_text_content(text);
                }
            }
            Block::ThematicBreak { .. } => {}
        }
    }
}

impl ListItem {
    pub(super) fn append_text_content(&self, text: &mut String) {
        for block in &self.children {
            block.append_text_content(text);
        }
    }
}

impl TableRow {
    pub(super) fn append_text_content(&self, text: &mut String) {
        for cell in &self.cells {
            append_inlines(&cell.children, text);
        }
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the component counts rendered text off its shadow tree; this module's tests check the parse's"
    )
)]
impl Inline {
    pub(super) fn range(&self) -> &SourceRange {
        match self {
            Inline::Text { range, .. }
            | Inline::Emphasis { range, .. }
            | Inline::Strong { range, .. }
            | Inline::Strikethrough { range, .. }
            | Inline::Code { range, .. }
            | Inline::Link { range, .. }
            | Inline::Image { range, .. }
            | Inline::InlineView { range, .. }
            | Inline::SoftBreak { range }
            | Inline::HardBreak { range }
            | Inline::Html { range, .. } => range,
        }
    }

    /// The text this node renders: an image's and an inline view's is
    /// empty, as an `<img>`'s and a slot's `textContent` is.
    pub(super) fn text_content(&self) -> String {
        let mut text = String::new();
        self.append_text_content(&mut text);
        text
    }

    /// Appends the text this node renders to `text`.
    pub(super) fn append_text_content(&self, text: &mut String) {
        match self {
            Inline::Text { text: own, .. } | Inline::Code { text: own, .. } => text.push_str(own),
            Inline::Emphasis { children, .. }
            | Inline::Strong { children, .. }
            | Inline::Strikethrough { children, .. }
            | Inline::Link { children, .. }
            | Inline::Html { children, .. } => append_inlines(children, text),
            Inline::SoftBreak { .. } | Inline::HardBreak { .. } => text.push('\n'),
            Inline::Image { .. } | Inline::InlineView { .. } => {}
        }
    }
}

fn append_inlines(inlines: &[Inline], text: &mut String) {
    for inline in inlines {
        inline.append_text_content(text);
    }
}

/// Parses `content`, or the part of it `range` names in UTF-16 code units
/// (`content-range`, sliced as [`slice_utf16`] slices). Ranges in the result
/// count from the start of the whole `content` either way.
pub(super) fn parse(content: &str, range: Option<(usize, usize)>) -> MarkdownTree {
    let (source, base) = match range {
        Some((start, end)) => slice_utf16(content, start, end),
        None => (content, 0),
    };
    let mut builder = Builder::new(source, base);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        builder.event(event, range);
    }
    builder.finish()
}

/// What a `content-range` attribute value asks of the component
/// (`_handleContentRange`, `XMarkdownAttributes.ts:1255-1282`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentRangeUpdate {
    /// The attribute is absent or empty: render all of `content`.
    Clear,
    /// Render `content[start..end)` in UTF-16 code units.
    Set(usize, usize),
    /// The value is not a range: web-core keeps the range it had.
    Keep,
}

/// Reads a `content-range` value: a JSON array of two numbers, or, when the
/// text is not JSON at all, `"start,end"`. Each bound is JavaScript's
/// `Number()` of the item, the end must not be below the start, and both
/// are clamped to zero. A fractional bound is truncated, as `String.slice`
/// truncates it. JSON that is not a two-item array keeps the old range,
/// without trying the comma form. `Number()` of an array or object item is
/// taken as `NaN`, which refuses the value.
pub(super) fn parse_content_range(value: Option<&str>) -> ContentRangeUpdate {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return ContentRangeUpdate::Clear;
    };
    let (start, end) = match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Array(items)) if items.len() == 2 => {
            (json_to_number(&items[0]), json_to_number(&items[1]))
        }
        Ok(_) => return ContentRangeUpdate::Keep,
        Err(_) => match value.split(',').collect::<Vec<_>>()[..] {
            [start, end] => (string_to_number(start), string_to_number(end)),
            _ => return ContentRangeUpdate::Keep,
        },
    };
    if start.is_nan() || end.is_nan() || end < start {
        return ContentRangeUpdate::Keep;
    }
    ContentRangeUpdate::Set(to_index(start), to_index(end))
}

/// `Math.max(0, n)` truncated toward zero, saturating at `usize::MAX` for
/// `Infinity`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to zero and a float-to-int cast saturates, which is String.slice's clamp"
)]
fn to_index(number: f64) -> usize {
    number.max(0.0).trunc() as usize
}

/// ECMAScript `ToNumber` of a JSON value.
fn json_to_number(value: &serde_json::Value) -> f64 {
    match value {
        serde_json::Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        serde_json::Value::String(text) => string_to_number(text),
        serde_json::Value::Bool(flag) => f64::from(u8::from(*flag)),
        serde_json::Value::Null => 0.0,
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => f64::NAN,
    }
}

/// ECMAScript `StringToNumber`: trimmed, empty is 0, `Infinity` with an
/// optional sign, `0x`/`0o`/`0b` integers without one, otherwise a decimal
/// literal that must cover the whole text.
fn string_to_number(text: &str) -> f64 {
    let text = text.trim_matches(is_js_space);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
                return f64::NAN;
            }
            // Digit by digit, so a literal past `u128` rounds as ToNumber
            // rounds it rather than failing.
            return digits
                .chars()
                .filter_map(|digit| digit.to_digit(radix))
                .fold(0.0, |value, digit| {
                    value * f64::from(radix) + f64::from(digit)
                });
        }
    }
    // Rust's float parser also takes `inf` and `nan`, which `Number()` does not.
    if !text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// The number of UTF-16 code units in `text`, its JavaScript `length`.
pub(super) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `content.slice(start, end)` by UTF-16 code units, with the slice's start
/// as a UTF-16 offset into `content`. Both bounds are clamped to the string
/// and an end before the start is the empty slice at the start, as
/// `String.prototype.slice` clamps. A bound inside a surrogate pair cannot
/// cut a Rust string: the start moves past the pair and the end before it,
/// so a half character is left out where JavaScript would keep a lone
/// surrogate.
pub(super) fn slice_utf16(content: &str, start: usize, end: usize) -> (&str, usize) {
    let (start_byte, start_unit) = utf16_to_byte(content, start, true);
    let (end_byte, _) = utf16_to_byte(content, end, false);
    (&content[start_byte..end_byte.max(start_byte)], start_unit)
}

/// The byte offset of UTF-16 offset `unit` in `text`, rounded to a
/// character boundary up or down, and the UTF-16 offset of that boundary.
fn utf16_to_byte(text: &str, unit: usize, round_up: bool) -> (usize, usize) {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units >= unit {
            return (byte, units);
        }
        let next = units + character.len_utf16();
        if next > unit {
            return if round_up {
                (byte + character.len_utf8(), next)
            } else {
                (byte, units)
            };
        }
        units = next;
    }
    (text.len(), units)
}

/// Byte offsets into the parsed text to UTF-16 offsets into `content`.
struct Utf16Map {
    /// The UTF-16 offset of the parsed text in `content`.
    base: usize,
    /// For each non-ASCII character, the byte offset just past it and how
    /// many more bytes than UTF-16 units the text holds up to there.
    excess: Vec<(usize, usize)>,
}

impl Utf16Map {
    fn new(text: &str, base: usize) -> Self {
        let mut excess = Vec::new();
        let mut total = 0;
        for (byte, character) in text.char_indices() {
            if !character.is_ascii() {
                total += character.len_utf8() - character.len_utf16();
                excess.push((byte + character.len_utf8(), total));
            }
        }
        Self { base, excess }
    }

    fn unit(&self, byte: usize) -> usize {
        let index = self.excess.partition_point(|&(end, _)| end <= byte);
        let excess = index.checked_sub(1).map_or(0, |index| self.excess[index].1);
        self.base + byte - excess
    }

    fn range(&self, range: &Range<usize>) -> SourceRange {
        self.unit(range.start)..self.unit(range.end)
    }
}

/// Offsets into a piece of text assembled from source spans — a run of text
/// events, or an HTML fragment — to byte offsets into the parsed text.
struct SegmentMap {
    segments: Vec<Segment>,
}

struct Segment {
    /// Where this segment starts in the assembled text.
    at: usize,
    /// Its length in the assembled text.
    len: usize,
    /// The source bytes it came from.
    source: Range<usize>,
    /// Whether it is those bytes verbatim; otherwise (an entity, an escape,
    /// a line with a container's prefix removed) an offset inside it maps to
    /// one of its ends.
    verbatim: bool,
}

impl SegmentMap {
    fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    fn push(&mut self, text: &str, source: Range<usize>, parsed: &str) {
        let at = self
            .segments
            .last()
            .map_or(0, |segment| segment.at + segment.len);
        let verbatim = parsed.get(source.clone()) == Some(text);
        self.segments.push(Segment {
            at,
            len: text.len(),
            source,
            verbatim,
        });
    }

    /// The source byte for offset `at`: the start of the segment it opens
    /// when `at` is a boundary and `prefer_end` is false, the end of the one
    /// it closes when true.
    fn byte(&self, at: usize, prefer_end: bool) -> usize {
        let index = if prefer_end {
            self.segments
                .partition_point(|segment| segment.at + segment.len < at)
        } else {
            self.segments
                .partition_point(|segment| segment.at <= at)
                .saturating_sub(1)
        };
        let Some(segment) = self.segments.get(index).or(self.segments.last()) else {
            return 0;
        };
        let within = at.saturating_sub(segment.at).min(segment.len);
        if segment.verbatim {
            segment.source.start + within
        } else if within == 0 || (within < segment.len && !prefer_end) {
            segment.source.start
        } else {
            segment.source.end
        }
    }

    fn range(&self, range: &Range<usize>) -> Range<usize> {
        self.byte(range.start, false)..self.byte(range.end, true)
    }
}

/// The tree under construction: one frame per open container.
struct Builder<'s> {
    /// The parsed text: `content`, or its `content-range` slice.
    source: &'s str,
    units: Utf16Map,
    /// The open containers, the root first; never empty.
    stack: Vec<Frame>,
    /// Consecutive text events not yet placed, so autolinking sees a run of
    /// text as markdown-it's text token does.
    pending: Vec<(String, Range<usize>)>,
    /// While positive, the events of a container dropped for depth, counted
    /// by nesting.
    skip: usize,
    /// Open [`InlineKind::Link`] frames: no autolinking inside one.
    link_depth: usize,
    /// Raw `<a ...>` tags opened and not closed in this paragraph, heading
    /// or cell: no autolinking inside one.
    html_link_level: usize,
    finder: LinkFinder,
}

enum Frame {
    /// The root, a block quote or a list item: holds blocks, and inline
    /// content that has no paragraph of its own.
    Container {
        kind: ContainerKind,
        range: Range<usize>,
        blocks: Vec<Block>,
        loose: Vec<Inline>,
    },
    List {
        start: Option<u64>,
        range: Range<usize>,
        items: Vec<ListItem>,
    },
    Table {
        alignments: Vec<Alignment>,
        range: Range<usize>,
        head: Option<TableRow>,
        body: Vec<TableRow>,
    },
    Row {
        head: bool,
        range: Range<usize>,
        cells: Vec<TableCell>,
    },
    CodeBlock {
        language: Option<String>,
        range: Range<usize>,
        text: String,
    },
    HtmlBlock {
        range: Range<usize>,
        fragment: String,
        segments: SegmentMap,
    },
    Inline {
        kind: InlineKind,
        range: Range<usize>,
        children: Vec<Inline>,
    },
    /// An image: its description only contributes to `alt`, so the events
    /// inside it build no frames, and `depth` counts their nesting.
    Image {
        target: ImageTarget,
        range: Range<usize>,
        alt: String,
        depth: usize,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Root,
    BlockQuote,
    Item,
}

enum InlineKind {
    Paragraph,
    Heading(u8),
    Cell,
    Emphasis,
    Strong,
    Strikethrough,
    /// `text` replaces the children of an autolink, whose shown text is
    /// `normalizeLinkText` of its destination.
    Link {
        href: String,
        title: Option<String>,
        text: Option<String>,
    },
    /// A construct markdown-it would not have parsed — a link or image
    /// whose destination `validateLink` refuses, a single-tilde
    /// strikethrough: its own source text around its children. `children`
    /// is the source span the children cover.
    Refused {
        children: Option<Range<usize>>,
    },
    /// A raw HTML `<span>` or `<p>` written inside inline content.
    Html {
        element: HtmlElement,
        classes: Vec<String>,
    },
}

enum ImageTarget {
    Image { src: String, title: Option<String> },
    InlineView { id: String },
}

impl Frame {
    fn range(&self) -> &Range<usize> {
        match self {
            Frame::Container { range, .. }
            | Frame::List { range, .. }
            | Frame::Table { range, .. }
            | Frame::Row { range, .. }
            | Frame::CodeBlock { range, .. }
            | Frame::HtmlBlock { range, .. }
            | Frame::Inline { range, .. }
            | Frame::Image { range, .. } => range,
        }
    }

    fn is_html(&self) -> bool {
        matches!(
            self,
            Frame::Inline {
                kind: InlineKind::Html { .. },
                ..
            }
        )
    }

    fn inline(kind: InlineKind, range: Range<usize>) -> Self {
        Frame::Inline {
            kind,
            range,
            children: Vec::new(),
        }
    }
}

impl<'s> Builder<'s> {
    fn new(source: &'s str, base: usize) -> Self {
        let mut finder = LinkFinder::new();
        finder.url_must_have_scheme(true);
        Self {
            source,
            units: Utf16Map::new(source, base),
            stack: vec![Frame::Container {
                kind: ContainerKind::Root,
                range: 0..source.len(),
                blocks: Vec::new(),
                loose: Vec::new(),
            }],
            pending: Vec::new(),
            skip: 0,
            link_depth: 0,
            html_link_level: 0,
            finder,
        }
    }

    fn finish(mut self) -> MarkdownTree {
        self.flush_text();
        while self.stack.len() > 1 {
            self.close_top();
        }
        let Some(Frame::Container {
            blocks,
            loose,
            range,
            ..
        }) = self.stack.pop()
        else {
            return MarkdownTree::default();
        };
        let mut blocks = blocks;
        push_loose(&mut blocks, loose, &range, &self.units);
        MarkdownTree { blocks }
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        if self.skip > 0 {
            match event {
                Event::Start(_) => self.skip += 1,
                Event::End(_) => self.skip -= 1,
                _ => {}
            }
            return;
        }
        if matches!(self.stack.last(), Some(Frame::Image { .. })) {
            self.image_event(event);
            return;
        }
        match event {
            Event::Text(text) => self.text(text, range),
            Event::Html(html) => {
                if let Some(Frame::HtmlBlock {
                    fragment, segments, ..
                }) = self.stack.last_mut()
                {
                    segments.push(&html, range, self.source);
                    fragment.push_str(&html);
                } else {
                    self.flush_text();
                    self.note_child(&range);
                    self.inline_html(&html, range);
                }
            }
            Event::End(_) => {
                self.flush_text();
                self.close_html_frames();
                self.close_top();
            }
            event => {
                self.flush_text();
                self.note_child(&range);
                match event {
                    Event::Start(tag) => self.start(tag, range),
                    Event::Code(code) => self.push_inline(Inline::Code {
                        text: code.into_string(),
                        range: self.units.range(&range),
                    }),
                    Event::InlineHtml(html) => self.inline_html(&html, range),
                    Event::SoftBreak => self.push_inline(Inline::SoftBreak {
                        range: self.units.range(&range),
                    }),
                    Event::HardBreak => self.push_inline(Inline::HardBreak {
                        range: self.units.range(&range),
                    }),
                    Event::Rule => self.push_block(Block::ThematicBreak {
                        range: self.units.range(&range),
                    }),
                    // Footnotes, task lists and math are not enabled.
                    _ => {}
                }
            }
        }
    }

    fn text(&mut self, text: CowStr<'_>, range: Range<usize>) {
        match self.stack.last_mut() {
            Some(Frame::CodeBlock { text: code, .. }) => code.push_str(&text),
            Some(Frame::HtmlBlock {
                fragment, segments, ..
            }) => {
                segments.push(&text, range, self.source);
                fragment.push_str(&text);
            }
            _ => {
                self.note_child(&range);
                self.pending.push((text.into_string(), range));
            }
        }
    }

    /// Records that an event of the source span `range` is a child of the
    /// refused construct on top, if one is.
    fn note_child(&mut self, range: &Range<usize>) {
        if let Some(Frame::Inline {
            kind: InlineKind::Refused { children },
            ..
        }) = self.stack.last_mut()
        {
            let span = children.get_or_insert(range.clone());
            span.start = span.start.min(range.start);
            span.end = span.end.max(range.end);
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        if self.stack.len() > MAX_NESTING {
            self.skip = 1;
            return;
        }
        let frame = match tag {
            Tag::Paragraph => {
                self.html_link_level = 0;
                Frame::inline(InlineKind::Paragraph, range)
            }
            Tag::Heading { level, .. } => {
                self.html_link_level = 0;
                Frame::inline(InlineKind::Heading(level as u8), range)
            }
            Tag::BlockQuote(_) => Frame::Container {
                kind: ContainerKind::BlockQuote,
                range,
                blocks: Vec::new(),
                loose: Vec::new(),
            },
            Tag::Item => Frame::Container {
                kind: ContainerKind::Item,
                range,
                blocks: Vec::new(),
                loose: Vec::new(),
            },
            Tag::List(start) => Frame::List {
                start,
                range,
                items: Vec::new(),
            },
            Tag::CodeBlock(kind) => Frame::CodeBlock {
                language: match kind {
                    CodeBlockKind::Fenced(info) => fence_language(&info),
                    CodeBlockKind::Indented => None,
                },
                range,
                text: String::new(),
            },
            Tag::HtmlBlock => Frame::HtmlBlock {
                range,
                fragment: String::new(),
                segments: SegmentMap::new(),
            },
            Tag::Table(alignments) => Frame::Table {
                alignments: alignments.into_iter().map(Alignment::from).collect(),
                range,
                head: None,
                body: Vec::new(),
            },
            Tag::TableHead => Frame::Row {
                head: true,
                range,
                cells: Vec::new(),
            },
            Tag::TableRow => Frame::Row {
                head: false,
                range,
                cells: Vec::new(),
            },
            Tag::TableCell => {
                self.html_link_level = 0;
                Frame::inline(InlineKind::Cell, range)
            }
            Tag::Emphasis => Frame::inline(InlineKind::Emphasis, range),
            Tag::Strong => Frame::inline(InlineKind::Strong, range),
            Tag::Strikethrough => {
                let kind = if self.source[range.start..].starts_with("~~") {
                    InlineKind::Strikethrough
                } else {
                    InlineKind::Refused { children: None }
                };
                Frame::inline(kind, range)
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => link_frame(link_type, &dest_url, &title, range),
            Tag::Image {
                dest_url, title, ..
            } => image_frame(&dest_url, &title, range),
            // Not enabled.
            _ => {
                self.skip = 1;
                return;
            }
        };
        if matches!(
            frame,
            Frame::Inline {
                kind: InlineKind::Link { .. },
                ..
            }
        ) {
            self.link_depth += 1;
        }
        self.stack.push(frame);
    }

    /// An event inside an image's description, which only adds to `alt`:
    /// markdown-it's `renderInlineAsText` (`src/renderer.ts`) keeps text,
    /// code span text, raw HTML as its sanitized markup and a line break as
    /// `"\n"`, and leaves out everything else.
    fn image_event(&mut self, event: Event<'_>) {
        let Some(Frame::Image { alt, depth, .. }) = self.stack.last_mut() else {
            return;
        };
        match event {
            Event::Start(_) => *depth += 1,
            Event::End(_) if *depth > 0 => *depth -= 1,
            Event::End(_) => self.close_top(),
            Event::Text(text) | Event::Code(text) => alt.push_str(&text),
            Event::InlineHtml(html) | Event::Html(html) => alt.push_str(&sanitized_markup(&html)),
            Event::SoftBreak | Event::HardBreak => alt.push('\n'),
            _ => {}
        }
    }

    /// Pops the top frame and places what it built in its parent.
    fn close_top(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match frame {
            Frame::Container {
                kind,
                range,
                blocks,
                loose,
            } => self.close_container(kind, &range, blocks, loose),
            Frame::List {
                start,
                range,
                items,
            } => {
                let range = self.units.range(&range);
                self.push_block(Block::List {
                    start,
                    items,
                    range,
                });
            }
            Frame::Table {
                alignments,
                range,
                head,
                body,
            } => {
                let head = head.unwrap_or_else(|| TableRow {
                    cells: Vec::new(),
                    range: self.units.range(&(range.start..range.start)),
                });
                let range = self.units.range(&range);
                self.push_block(Block::Table {
                    alignments,
                    head,
                    body,
                    range,
                });
            }
            Frame::Row { head, range, cells } => self.close_row(head, &range, cells),
            Frame::CodeBlock {
                language,
                range,
                text,
            } => {
                let range = self.units.range(&range);
                self.push_block(Block::Code {
                    language,
                    text,
                    range,
                });
            }
            Frame::HtmlBlock {
                range,
                fragment,
                segments,
            } => self.html_block(&fragment, &segments, &range),
            Frame::Inline {
                kind,
                range,
                children,
            } => self.close_inline(kind, &range, children),
            Frame::Image {
                target, range, alt, ..
            } => {
                let range = self.units.range(&range);
                self.push_inline(match target {
                    ImageTarget::Image { src, title } => Inline::Image {
                        src,
                        alt,
                        title,
                        range,
                    },
                    ImageTarget::InlineView { id } => Inline::InlineView { id, range },
                });
            }
        }
    }

    fn close_container(
        &mut self,
        kind: ContainerKind,
        range: &Range<usize>,
        mut blocks: Vec<Block>,
        loose: Vec<Inline>,
    ) {
        push_loose(&mut blocks, loose, range, &self.units);
        let range = self.units.range(range);
        match kind {
            ContainerKind::BlockQuote => self.push_block(Block::Quote {
                children: blocks,
                range,
            }),
            ContainerKind::Item => {
                if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                    items.push(ListItem {
                        children: blocks,
                        range,
                    });
                }
            }
            ContainerKind::Root => {}
        }
    }

    fn close_row(&mut self, head: bool, range: &Range<usize>, cells: Vec<TableCell>) {
        let row = TableRow {
            cells,
            range: self.units.range(range),
        };
        if let Some(Frame::Table {
            head: table_head,
            body,
            ..
        }) = self.stack.last_mut()
        {
            if head {
                *table_head = Some(row);
            } else {
                body.push(row);
            }
        }
    }

    fn close_inline(&mut self, kind: InlineKind, source: &Range<usize>, children: Vec<Inline>) {
        let range = self.units.range(source);
        match kind {
            InlineKind::Paragraph => self.push_block(Block::Paragraph { children, range }),
            InlineKind::Heading(level) => self.push_block(Block::Heading {
                level,
                children,
                range,
            }),
            InlineKind::Cell => {
                if let Some(Frame::Row { cells, .. }) = self.stack.last_mut() {
                    cells.push(TableCell { children, range });
                }
            }
            InlineKind::Emphasis => self.push_inline(Inline::Emphasis { children, range }),
            InlineKind::Strong => self.push_inline(Inline::Strong { children, range }),
            InlineKind::Strikethrough => {
                self.push_inline(Inline::Strikethrough { children, range });
            }
            InlineKind::Link { href, title, text } => {
                self.link_depth = self.link_depth.saturating_sub(1);
                let children = match text {
                    Some(text) => {
                        let inner = match (children.first(), children.last()) {
                            (Some(first), Some(last)) => first.range().start..last.range().end,
                            _ => range.clone(),
                        };
                        vec![Inline::Text { text, range: inner }]
                    }
                    None => children,
                };
                self.push_inline(Inline::Link {
                    href,
                    title,
                    children,
                    range,
                });
            }
            InlineKind::Refused { children: span } => match span {
                Some(span) => {
                    self.push_source_text(source.start..span.start);
                    for child in children {
                        self.push_inline(child);
                    }
                    self.push_source_text(span.end..source.end);
                }
                None => self.push_source_text(source.clone()),
            },
            InlineKind::Html { element, classes } => self.push_inline(Inline::Html {
                element,
                classes,
                children,
                range,
            }),
        }
    }

    fn push_source_text(&mut self, range: Range<usize>) {
        if let Some(text) = self
            .source
            .get(range.clone())
            .filter(|text| !text.is_empty())
        {
            let text = text.to_owned();
            self.push_inline(Inline::Text {
                text,
                range: self.units.range(&range),
            });
        }
    }

    fn push_block(&mut self, block: Block) {
        if let Some(Frame::Container {
            blocks,
            loose,
            range,
            ..
        }) = self.stack.last_mut()
        {
            push_loose(blocks, std::mem::take(loose), range, &self.units);
            blocks.push(block);
        }
    }

    fn push_inline(&mut self, inline: Inline) {
        match self.stack.last_mut() {
            Some(Frame::Inline { children, .. }) => push_merged(children, inline),
            Some(Frame::Container { loose, .. }) => push_merged(loose, inline),
            _ => {}
        }
    }

    /// Places the pending text run, autolinked unless inside a link.
    fn flush_text(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let mut text = String::new();
        let mut map = SegmentMap::new();
        for (piece, range) in std::mem::take(&mut self.pending) {
            map.push(&piece, range, self.source);
            text.push_str(&piece);
        }
        let links = if self.link_depth == 0 && self.html_link_level == 0 {
            self.find_links(&text)
        } else {
            Vec::new()
        };
        let mut at = 0;
        for link in links {
            self.push_text_piece(&text, at..link.start, &map);
            let range = self.units.range(&map.range(&(link.start..link.end)));
            self.push_inline(Inline::Link {
                href: link.href,
                title: None,
                children: vec![Inline::Text {
                    text: link.text,
                    range: range.clone(),
                }],
                range,
            });
            at = link.end;
        }
        self.push_text_piece(&text, at..text.len(), &map);
    }

    fn push_text_piece(&mut self, text: &str, piece: Range<usize>, map: &SegmentMap) {
        if piece.is_empty() {
            return;
        }
        let range = self.units.range(&map.range(&piece));
        self.push_inline(Inline::Text {
            text: text[piece].to_owned(),
            range,
        });
    }

    /// markdown-it's linkify over one text run (see the module comment): the
    /// URLs the [`linkify`] crate finds whose scheme is `http`, `https` or
    /// `ftp`, and its email addresses, each normalized, validated and given
    /// the text `normalizeLinkText` shows (`src/rules_core/linkify.ts`).
    fn find_links(&self, text: &str) -> Vec<FoundLink> {
        let mut links: Vec<FoundLink> = Vec::new();
        for link in self.finder.links(text) {
            let (start, href, shown) = match link.kind() {
                LinkKind::Url => {
                    let url = link.as_str();
                    let scheme = url.split_once("://").map_or("", |(scheme, _)| scheme);
                    if !["http", "https", "ftp"]
                        .iter()
                        .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
                    {
                        continue;
                    }
                    (link.start(), normalize_link(url), normalize_link_text(url))
                }
                LinkKind::Email => {
                    let start = link.start();
                    let prefixed = start >= 7
                        && text.is_char_boundary(start - 7)
                        && text[start - 7..start].eq_ignore_ascii_case("mailto:");
                    if prefixed {
                        let url = &text[start - 7..link.end()];
                        (start - 7, normalize_link(url), normalize_link_text(url))
                    } else {
                        let url = format!("mailto:{}", link.as_str());
                        let shown = normalize_link_text(&url);
                        let shown = shown
                            .strip_prefix("mailto:")
                            .map_or(shown.clone(), str::to_owned);
                        (start, normalize_link(&url), shown)
                    }
                }
                _ => continue,
            };
            if !validate_link(&href) || links.last().is_some_and(|last| last.end > start) {
                continue;
            }
            links.push(FoundLink {
                start,
                end: link.end(),
                href,
                text: shown,
            });
        }
        links
    }

    /// One inline raw HTML construct.
    fn inline_html(&mut self, fragment: &str, range: Range<usize>) {
        if is_link_open(fragment) {
            self.html_link_level += 1;
        } else if is_link_close(fragment) {
            self.html_link_level = self.html_link_level.saturating_sub(1);
        }
        let mut map = SegmentMap::new();
        map.push(fragment, range.clone(), self.source);
        let Some(tokens) = sanitize(fragment) else {
            self.push_inline(Inline::Text {
                text: fragment.to_owned(),
                range: self.units.range(&range),
            });
            return;
        };
        for token in tokens {
            match token {
                HtmlToken::Open {
                    element,
                    classes,
                    self_closing,
                    range: tag,
                } => {
                    let tag = map.range(&tag);
                    if self_closing {
                        let range = self.units.range(&tag);
                        self.push_inline(Inline::Html {
                            element,
                            classes,
                            children: Vec::new(),
                            range,
                        });
                    } else if self.stack.len() <= MAX_NESTING {
                        self.stack
                            .push(Frame::inline(InlineKind::Html { element, classes }, tag));
                    }
                }
                HtmlToken::Close {
                    element,
                    range: tag,
                } => self.close_html(element, map.byte(tag.end, true)),
                HtmlToken::Text { range: text } => {
                    let decoded = decode_entities(&fragment[text.clone()]);
                    if !decoded.is_empty() {
                        let range = self.units.range(&map.range(&text));
                        self.push_inline(Inline::Text {
                            text: decoded,
                            range,
                        });
                    }
                }
            }
        }
    }

    /// An inline end tag: closes the nearest open element of its name among
    /// the raw HTML frames on top, unless a `p` stands between (see the
    /// module comment); a tag naming nothing open there is ignored.
    fn close_html(&mut self, element: HtmlElement, end: usize) {
        let mut index = self.stack.len();
        loop {
            index -= 1;
            match self.stack.get(index) {
                Some(Frame::Inline {
                    kind: InlineKind::Html { element: open, .. },
                    ..
                }) if index > 0 => {
                    if *open == element {
                        break;
                    }
                    if *open == HtmlElement::P {
                        return;
                    }
                }
                _ => return,
            }
        }
        while self.stack.len() > index {
            if let Some(Frame::Inline { range, .. }) = self.stack.last_mut() {
                range.end = end;
            }
            self.close_top();
        }
    }

    /// Closes the raw HTML frames on top where the Markdown construct under
    /// them ends.
    fn close_html_frames(&mut self) {
        let end = self
            .stack
            .iter()
            .rev()
            .find(|frame| !frame.is_html())
            .map_or(0, |frame| frame.range().end);
        while self.stack.last().is_some_and(Frame::is_html) {
            if let Some(Frame::Inline { range, .. }) = self.stack.last_mut() {
                range.end = end;
            }
            self.close_top();
        }
    }

    /// One HTML block: its blocks, built by [`HtmlTree`].
    fn html_block(&mut self, fragment: &str, segments: &SegmentMap, range: &Range<usize>) {
        // markdown-it's block keeps its last line break (`getLines(..., true)`,
        // with line endings normalized to `\n`), and the browser keeps it as
        // text after the block's elements, or inside a `<p>` left open: it
        // renders as nothing, but it is part of `textContent`.
        let whole = fragment.len();
        let fragment = fragment.trim_end_matches(['\n', '\r']);
        let line_break = (fragment.len() < whole).then_some(fragment.len()..whole);
        let Some(tokens) = sanitize(fragment) else {
            if !fragment.is_empty() {
                let end = line_break
                    .as_ref()
                    .map_or(fragment.len(), |range| range.end);
                let text_range = self.units.range(&segments.range(&(0..end)));
                let mut text = fragment.to_owned();
                if line_break.is_some() {
                    text.push('\n');
                }
                self.push_block(Block::Inlines {
                    children: vec![Inline::Text {
                        text,
                        range: text_range,
                    }],
                    range: self.units.range(range),
                });
            }
            return;
        };
        let mut tree = HtmlTree {
            fragment,
            map: segments,
            units: &self.units,
            open: Vec::new(),
            loose: Vec::new(),
            blocks: Vec::new(),
        };
        for token in tokens {
            match token {
                HtmlToken::Open {
                    element,
                    classes,
                    self_closing,
                    range,
                } => tree.open(element, classes, range, self_closing),
                HtmlToken::Close { element, range } => tree.close(element, range),
                HtmlToken::Text { range } => tree.text(range),
            }
        }
        if let Some(line_break) = line_break {
            tree.line_break(&line_break);
        }
        for block in tree.finish() {
            self.push_block(block);
        }
    }
}

struct FoundLink {
    start: usize,
    end: usize,
    href: String,
    text: String,
}

fn link_frame(link_type: LinkType, dest: &str, title: &str, range: Range<usize>) -> Frame {
    // markdown-it's `autolink` rule (`src/rules_inline/autolink.ts`): an
    // email autolink's `href` gets `mailto:`, and both kinds show
    // `normalizeLinkText` of what was written.
    let (url, text) = match link_type {
        LinkType::Email => (format!("mailto:{dest}"), Some(normalize_link_text(dest))),
        LinkType::Autolink => (dest.to_owned(), Some(normalize_link_text(dest))),
        _ => (dest.to_owned(), None),
    };
    let href = normalize_link(&url);
    if !validate_link(&href) {
        return Frame::inline(InlineKind::Refused { children: None }, range);
    }
    let title = (!title.is_empty()).then(|| title.to_owned());
    Frame::inline(InlineKind::Link { href, title, text }, range)
}

fn image_frame(dest: &str, title: &str, range: Range<usize>) -> Frame {
    let src = normalize_link(dest);
    if !validate_link(&src) {
        return Frame::inline(InlineKind::Refused { children: None }, range);
    }
    // `preprocessInlineView` matches `src="inlineview://([^"]+)"` in the
    // rendered HTML: a non-empty rest of the normalized source.
    let target = match src.strip_prefix(INLINE_VIEW_SCHEME) {
        Some(id) if !id.is_empty() => ImageTarget::InlineView { id: id.to_owned() },
        _ => ImageTarget::Image {
            src,
            title: (!title.is_empty()).then(|| title.to_owned()),
        },
    };
    Frame::Image {
        target,
        range,
        alt: String::new(),
        depth: 0,
    }
}

/// The fence info string's first word (markdown-it's `fence` renderer:
/// `info.split(/(\s+)/g)[0]` of the trimmed info).
fn fence_language(info: &str) -> Option<String> {
    info.trim_matches(is_js_space)
        .split(is_js_space)
        .next()
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
}

impl From<CmarkAlignment> for Alignment {
    fn from(alignment: CmarkAlignment) -> Self {
        match alignment {
            CmarkAlignment::None => Alignment::None,
            CmarkAlignment::Left => Alignment::Left,
            CmarkAlignment::Center => Alignment::Center,
            CmarkAlignment::Right => Alignment::Right,
        }
    }
}

/// Appends `inline`, joining it to a text node it directly follows in the
/// source, as the HTML parser joins adjacent character data.
fn push_merged(inlines: &mut Vec<Inline>, inline: Inline) {
    if let (
        Some(Inline::Text { text, range }),
        Inline::Text {
            text: next,
            range: next_range,
        },
    ) = (inlines.last_mut(), &inline)
        && range.end == next_range.start
    {
        text.push_str(next);
        range.end = next_range.end;
        return;
    }
    inlines.push(inline);
}

/// Moves a container's loose inline content into an [`Block::Inlines`].
fn push_loose(
    blocks: &mut Vec<Block>,
    loose: Vec<Inline>,
    container: &Range<usize>,
    units: &Utf16Map,
) {
    let (Some(first), Some(last)) = (loose.first(), loose.last()) else {
        return;
    };
    let range = first.range().start..last.range().end;
    let range = if range.is_empty() {
        units.range(container)
    } else {
        range
    };
    blocks.push(Block::Inlines {
        children: loose,
        range,
    });
}

/// An HTML block's tags paired by the tree builder's rules for `p` and
/// `span` (see the module comment).
struct HtmlTree<'a> {
    fragment: &'a str,
    map: &'a SegmentMap,
    units: &'a Utf16Map,
    open: Vec<OpenElement>,
    /// Inline content outside every `p`, not yet placed in a block.
    loose: Vec<Inline>,
    blocks: Vec<Block>,
}

struct OpenElement {
    element: HtmlElement,
    classes: Vec<String>,
    /// Where its start tag begins in the fragment.
    start: usize,
    children: Vec<Inline>,
}

impl HtmlTree<'_> {
    fn range(&self, range: &Range<usize>) -> SourceRange {
        self.units.range(&self.map.range(range))
    }

    fn text(&mut self, range: Range<usize>) {
        let text = decode_entities(&self.fragment[range.clone()]);
        if text.is_empty() {
            return;
        }
        self.push_text(text, &range);
    }

    /// The block's trailing line break, at `range` past the end of the
    /// trimmed fragment: `"\n"` whatever the source's line ending.
    fn line_break(&mut self, range: &Range<usize>) {
        self.push_text("\n".to_owned(), range);
    }

    fn push_text(&mut self, text: String, range: &Range<usize>) {
        let inline = Inline::Text {
            text,
            range: self.range(range),
        };
        match self.open.last_mut() {
            Some(element) => push_merged(&mut element.children, inline),
            None => push_merged(&mut self.loose, inline),
        }
    }

    fn open(
        &mut self,
        element: HtmlElement,
        classes: Vec<String>,
        tag: Range<usize>,
        self_closing: bool,
    ) {
        if element == HtmlElement::P
            && let Some(index) = self
                .open
                .iter()
                .rposition(|open| open.element == HtmlElement::P)
        {
            self.close_to(index, tag.start);
        }
        if !self_closing && self.open.len() >= MAX_NESTING {
            return;
        }
        self.open.push(OpenElement {
            element,
            classes,
            start: tag.start,
            children: Vec::new(),
        });
        if self_closing {
            self.close_to(self.open.len() - 1, tag.end);
        }
    }

    fn close(&mut self, element: HtmlElement, tag: Range<usize>) {
        for index in (0..self.open.len()).rev() {
            match self.open[index].element {
                open if open == element => {
                    self.close_to(index, tag.end);
                    return;
                }
                // `</span>` stops at a `p` (a special element): ignored.
                HtmlElement::P => return,
                HtmlElement::Span => {}
            }
        }
        // `</p>` with no `p` open: the tree builder inserts an empty `p`.
        if element == HtmlElement::P {
            self.open(HtmlElement::P, Vec::new(), tag, true);
        }
    }

    /// Closes `open[index..]`, ending them at fragment offset `end`.
    fn close_to(&mut self, index: usize, end: usize) {
        while self.open.len() > index {
            let Some(element) = self.open.pop() else {
                return;
            };
            let range = self.range(&(element.start..end));
            let OpenElement {
                element,
                classes,
                children,
                ..
            } = element;
            if let Some(parent) = self.open.last_mut() {
                push_merged(
                    &mut parent.children,
                    Inline::Html {
                        element,
                        classes,
                        children,
                        range,
                    },
                );
            } else if element == HtmlElement::P {
                self.flush_loose();
                self.blocks.push(Block::HtmlParagraph {
                    classes,
                    children,
                    range,
                });
            } else {
                push_merged(
                    &mut self.loose,
                    Inline::Html {
                        element,
                        classes,
                        children,
                        range,
                    },
                );
            }
        }
    }

    /// Places the loose run as a block. The white space text at its ends,
    /// which a browser renders as nothing beside a block, is
    /// [`Block::HtmlSpace`] on either side of it rather than content.
    fn flush_loose(&mut self) {
        let mut loose = std::mem::take(&mut self.loose);
        let is_blank = |inline: &Inline| matches!(inline, Inline::Text { text, .. } if text.chars().all(is_html_space));
        let mut trailing = Vec::new();
        while loose.last().is_some_and(is_blank) {
            trailing.extend(loose.pop());
        }
        let leading = loose.iter().take_while(|inline| is_blank(inline)).count();
        let spaces: Vec<Inline> = loose.drain(..leading).collect();
        for inline in spaces {
            self.push_space(inline);
        }
        if let (Some(first), Some(last)) = (loose.first(), loose.last()) {
            let range = first.range().start..last.range().end;
            self.blocks.push(Block::Inlines {
                children: loose,
                range,
            });
        }
        for inline in trailing.into_iter().rev() {
            self.push_space(inline);
        }
    }

    /// One white-space text of the loose run as a [`Block::HtmlSpace`].
    fn push_space(&mut self, inline: Inline) {
        if let Inline::Text { text, range } = inline {
            self.blocks.push(Block::HtmlSpace { text, range });
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.close_to(0, self.fragment.len());
        self.flush_loose();
        self.blocks
    }
}

/// HTML's ASCII white space.
fn is_html_space(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}

/// A token of a kept HTML fragment, by fragment byte offsets.
#[derive(Debug, PartialEq, Eq)]
enum HtmlToken {
    Open {
        element: HtmlElement,
        classes: Vec<String>,
        self_closing: bool,
        range: Range<usize>,
    },
    Close {
        element: HtmlElement,
        range: Range<usize>,
    },
    Text {
        range: Range<usize>,
    },
}

/// `sanitizeAllowedHtml` (`XMarkdownAttributes.ts:74-116`) decides; `None`
/// is a fragment it escapes whole. A kept fragment is then tokenized the
/// way the browser parses web-core's rewritten markup (see [`tokenize`]).
fn sanitize(fragment: &str) -> Option<Vec<HtmlToken>> {
    if refuses(fragment) {
        None
    } else {
        Some(tokenize(fragment))
    }
}

/// Whether `sanitizeAllowedHtml` escapes `fragment` whole: it holds `<!`
/// (which covers `<!--`), `<?` or `<%`; or a tag that `tagNameRe`
/// (`/<\s*\/?\s*([A-Za-z][\w-]*)\b[^>]*>/g`) names is neither `span` nor
/// `p`; or it has no such tag but holds a `<`.
fn refuses(fragment: &str) -> bool {
    if fragment.is_empty() {
        return false;
    }
    if ["<!", "<?", "<%"]
        .iter()
        .any(|marker| fragment.contains(marker))
    {
        return true;
    }
    let mut has_tag = false;
    let mut at = 0;
    while let Some(offset) = fragment[at..].find('<') {
        let lt = at + offset;
        match tag_name_at(fragment, lt) {
            Some((name, end)) => {
                has_tag = true;
                if !name.eq_ignore_ascii_case("span") && !name.eq_ignore_ascii_case("p") {
                    return true;
                }
                at = end;
            }
            None => at = lt + 1,
        }
    }
    !has_tag && fragment.contains('<')
}

/// `tagNameRe` matched at the `<` at byte `lt`: the tag name and the byte
/// after the match's `>`.
fn tag_name_at(text: &str, lt: usize) -> Option<(&str, usize)> {
    let mut rest = text[lt + 1..].trim_start_matches(is_js_space);
    rest = rest.strip_prefix('/').unwrap_or(rest);
    rest = rest.trim_start_matches(is_js_space);
    if !rest.starts_with(|character: char| character.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = text.len() - rest.len();
    let name_len = rest
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'))
        .unwrap_or(rest.len());
    // `\b` after a greedy `[\w-]*` backtracks over trailing dashes.
    let name = text[name_start..name_start + name_len].trim_end_matches('-');
    let gt = text[name_start + name.len()..].find('>')?;
    Some((name, name_start + name.len() + gt + 1))
}

/// `openTagRe` (`/<(span|p)\b([^>]*?)(\/?)>/gi`) matched at the `<` at byte
/// `lt`.
struct OpenTag<'t> {
    element: HtmlElement,
    /// The lowercase tag name.
    name: &'static str,
    attributes: &'t str,
    self_closing: bool,
    /// The byte after the `>`.
    end: usize,
}

fn open_tag_at(text: &str, lt: usize) -> Option<OpenTag<'_>> {
    let rest = &text[lt + 1..];
    let (element, name, len) = if rest
        .get(..4)
        .is_some_and(|name| name.eq_ignore_ascii_case("span"))
    {
        (HtmlElement::Span, "span", 4)
    } else if rest
        .get(..1)
        .is_some_and(|name| name.eq_ignore_ascii_case("p"))
    {
        (HtmlElement::P, "p", 1)
    } else {
        return None;
    };
    // `\b`: the name must not run on into a word character.
    if rest[len..]
        .starts_with(|character: char| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    let gt = rest[len..].find('>')?;
    let inside = &rest[len..len + gt];
    let (attributes, self_closing) = match inside.strip_suffix('/') {
        Some(attributes) => (attributes, true),
        None => (inside, false),
    };
    Some(OpenTag {
        element,
        name,
        attributes,
        self_closing,
        end: lt + 1 + len + gt + 1,
    })
}

/// The tokens the browser's tokenizer makes of a kept fragment after web-core
/// rewrote its start tags: a `<span ...>`/`<p ...>` start tag (the classes
/// [`tag_classes`] keeps), a `</span>`/`</p>` end tag, and text. Everything
/// else that looks like markup in a kept fragment is removed, as the
/// browser removes it: another end tag, `</>`, a `</` that starts a bogus
/// comment, a start tag cut off by the end of the fragment. A start tag of
/// another name can only survive web-core's check when `tagNameRe` read it
/// differently from the browser (`< span <b>`); the browser would make
/// that element and `DOMPurify` keep it, and here its tag is removed and its
/// content kept.
fn tokenize(fragment: &str) -> Vec<HtmlToken> {
    fn flush(tokens: &mut Vec<HtmlToken>, end: usize, text_start: usize) {
        if end > text_start {
            tokens.push(HtmlToken::Text {
                range: text_start..end,
            });
        }
    }
    let mut tokens = Vec::new();
    let mut text_start = 0;
    let mut at = 0;
    while let Some(offset) = fragment[at..].find('<') {
        let lt = at + offset;
        if let Some(tag) = open_tag_at(fragment, lt) {
            flush(&mut tokens, lt, text_start);
            tokens.push(HtmlToken::Open {
                element: tag.element,
                classes: tag_classes(tag.attributes),
                self_closing: tag.self_closing,
                range: lt..tag.end,
            });
            at = tag.end;
            text_start = at;
            continue;
        }
        let rest = &fragment[lt + 1..];
        let skip_to = if let Some(after) = rest.strip_prefix('/') {
            match after.chars().next() {
                Some(character) if character.is_ascii_alphabetic() => {
                    flush(&mut tokens, lt, text_start);
                    // An end tag cut off by the end of the fragment is
                    // dropped with the rest of it.
                    match fragment[lt..].find('>') {
                        Some(gt) => {
                            let name_len = after
                                .find(['\t', '\n', '\u{c}', ' ', '/', '>'])
                                .unwrap_or(after.len());
                            let name = &after[..name_len];
                            let element = if name.eq_ignore_ascii_case("span") {
                                Some(HtmlElement::Span)
                            } else if name.eq_ignore_ascii_case("p") {
                                Some(HtmlElement::P)
                            } else {
                                None
                            };
                            if let Some(element) = element {
                                tokens.push(HtmlToken::Close {
                                    element,
                                    range: lt..lt + gt + 1,
                                });
                            }
                            lt + gt + 1
                        }
                        None => fragment.len(),
                    }
                }
                // `</` at the end is text.
                None => {
                    at = lt + 1;
                    continue;
                }
                // `</>` and a bogus comment up to the next `>`.
                Some(_) => {
                    flush(&mut tokens, lt, text_start);
                    fragment[lt..]
                        .find('>')
                        .map_or(fragment.len(), |gt| lt + gt + 1)
                }
            }
        } else if rest.starts_with(|character: char| character.is_ascii_alphabetic()) {
            flush(&mut tokens, lt, text_start);
            fragment[lt..]
                .find('>')
                .map_or(fragment.len(), |gt| lt + gt + 1)
        } else {
            at = lt + 1;
            continue;
        };
        at = skip_to;
        text_start = skip_to;
    }
    flush(&mut tokens, fragment.len(), text_start);
    tokens
}

/// The classes web-core keeps from a start tag's attribute text: the
/// `class` value `/\bclass\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i` finds
/// in the text after `decodeHtmlEntities` (`XMarkdownAttributes.ts:58-66`,
/// `:103-108`), reduced by `normalizeClassList` (`:68-72`). The regex is
/// replicated as written, so it also finds `class=` inside another
/// attribute's name or value (`data-class="x"`).
fn tag_classes(attributes: &str) -> Vec<String> {
    let attributes = decode_attribute_entities(attributes);
    let value = class_value(&attributes).unwrap_or_default();
    value
        .split(is_js_space)
        .filter(|token| !token.is_empty())
        .filter(|token| {
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
        .map(str::to_owned)
        .collect()
}

fn class_value(attributes: &str) -> Option<&str> {
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let bytes = attributes.as_bytes();
    for start in 0..bytes.len() {
        if !attributes.is_char_boundary(start)
            || !attributes[start..]
                .get(..5)
                .is_some_and(|word| word.eq_ignore_ascii_case("class"))
            || (start > 0 && is_word(bytes[start - 1]))
        {
            continue;
        }
        let rest = attributes[start + 5..].trim_start_matches(is_js_space);
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start_matches(is_js_space);
        if let Some(quoted) = rest.strip_prefix('"')
            && let Some(close) = quoted.find('"')
        {
            return Some(&quoted[..close]);
        }
        if let Some(quoted) = rest.strip_prefix('\'')
            && let Some(close) = quoted.find('\'')
        {
            return Some(&quoted[..close]);
        }
        let len = rest
            .find(|character: char| is_js_space(character) || character == '>')
            .unwrap_or(rest.len());
        if len > 0 {
            return Some(&rest[..len]);
        }
    }
    None
}

/// `decodeHtmlEntities` (`XMarkdownAttributes.ts:58-66`): seven fixed
/// replacements, in order, each over the result of the one before.
fn decode_attribute_entities(value: &str) -> String {
    [
        ("&quot;", "\""),
        ("&#34;", "\""),
        ("&apos;", "'"),
        ("&#39;", "'"),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
    ]
    .iter()
    .fold(value.to_owned(), |text, (entity, character)| {
        text.replace(entity, character)
    })
}

/// The markup `sanitizeAllowedHtml` returns for `fragment`
/// (`XMarkdownAttributes.ts:74-116`): `escapeHtml` of a refused fragment
/// (`:44-48`); a kept one with each start tag rewritten to its lowercase
/// name and kept classes, and a self-closing one written as a start and an
/// end tag. Only an image's `alt` holds this text, as markdown-it's
/// `renderInlineAsText` copies raw HTML into it.
fn sanitized_markup(fragment: &str) -> String {
    if refuses(fragment) {
        return fragment
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
    }
    let mut out = String::with_capacity(fragment.len());
    let mut at = 0;
    let mut copied = 0;
    while let Some(offset) = fragment[at..].find('<') {
        let lt = at + offset;
        let Some(tag) = open_tag_at(fragment, lt) else {
            at = lt + 1;
            continue;
        };
        out.push_str(&fragment[copied..lt]);
        let classes = tag_classes(tag.attributes).join(" ");
        out.push('<');
        out.push_str(tag.name);
        if !classes.is_empty() {
            out.push_str(" class=\"");
            out.push_str(&classes);
            out.push('"');
        }
        out.push('>');
        if tag.self_closing {
            out.push_str("</");
            out.push_str(tag.name);
            out.push('>');
        }
        at = tag.end;
        copied = tag.end;
    }
    out.push_str(&fragment[copied..]);
    out
}

/// Character references in HTML text, as far as this module decodes them:
/// `&#NNN;` and `&#xHH;` (the `;` optional, an invalid code point or NUL
/// being U+FFFD) and the named references `&amp;`, `&lt;`, `&gt;`, `&quot;`,
/// `&apos;` and `&nbsp;`. Any other `&` is text. The browser knows the
/// whole HTML named-reference table and the legacy references without `;`.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let (decoded, len) = decode_reference(rest).unwrap_or(('&', 1));
        out.push(decoded);
        rest = &rest[len..];
    }
    out.push_str(rest);
    out
}

/// The character `text` (starting with `&`) refers to, and the reference's
/// length.
fn decode_reference(text: &str) -> Option<(char, usize)> {
    for (name, character) in [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&nbsp;", '\u{a0}'),
    ] {
        if text.starts_with(name) {
            return Some((character, name.len()));
        }
    }
    let numeric = text.strip_prefix("&#")?;
    let (digits, radix, prefix) = match numeric.strip_prefix(['x', 'X']) {
        Some(hex) => (hex, 16, 3),
        None => (numeric, 10, 2),
    };
    let len = digits
        .find(|character: char| !character.is_digit(radix))
        .unwrap_or(digits.len());
    if len == 0 {
        return None;
    }
    let value = u32::from_str_radix(&digits[..len], radix).unwrap_or(u32::MAX);
    let character = char::from_u32(value)
        .filter(|&character| character != '\0')
        .unwrap_or('\u{fffd}');
    let semicolon = usize::from(digits[len..].starts_with(';'));
    Some((character, prefix + len + semicolon))
}

/// markdown-it's `isLinkOpen` (`src/rules_inline/html_inline.ts`).
fn is_link_open(fragment: &str) -> bool {
    let bytes = fragment.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'<'
        && bytes[1].eq_ignore_ascii_case(&b'a')
        && (bytes[2] == b'>' || fragment[2..].starts_with(is_js_space))
}

/// markdown-it's `isLinkClose`: `/^<\/a\s*>/i`.
fn is_link_close(fragment: &str) -> bool {
    fragment
        .strip_prefix("</")
        .and_then(|rest| rest.strip_prefix(['a', 'A']))
        .is_some_and(|rest| rest.trim_start_matches(is_js_space).starts_with('>'))
}

/// markdown-it's `normalizeLink` without the punycode step: the trimmed URL
/// through `mdurl.encode` with its default set, which keeps ASCII letters,
/// digits, `;/?:@&=+$,-_.!~*'()#` and an existing `%XX`, and percent-encodes
/// every other character's UTF-8 bytes.
pub(super) fn normalize_link(url: &str) -> String {
    const KEPT: &[u8] = b";/?:@&=+$,-_.!~*'()#";
    let url = url.trim_matches(is_js_space);
    let bytes = url.as_bytes();
    let mut out = String::with_capacity(url.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && bytes
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        {
            out.push_str(&url[index..index + 3]);
            index += 3;
            continue;
        }
        if byte.is_ascii_alphanumeric() || KEPT.contains(&byte) {
            out.push(char::from(byte));
        } else {
            push_percent(&mut out, byte);
        }
        index += 1;
    }
    out
}

/// markdown-it's `normalizeLinkText` without the punycode step: the
/// trimmed URL through `mdurl.decode`, which leaves `%XX` of
/// `;/?:@&=+$,#` and `%` encoded (upper-cased) and replaces a malformed
/// UTF-8 sequence with U+FFFD per byte it covers.
pub(super) fn normalize_link_text(url: &str) -> String {
    const KEPT: &[u8] = b";/?:@&=+$,#%";
    let url = url.trim_matches(is_js_space);
    let bytes = url.as_bytes();
    let mut out = String::with_capacity(url.len());
    let mut index = 0;
    let mut copied = 0;
    while index < bytes.len() {
        // A run of `%XX`.
        let mut run = Vec::new();
        let mut end = index;
        while bytes.get(end) == Some(&b'%')
            && let Some(hex) = url.get(end + 1..end + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
            && hex.bytes().all(|digit| digit.is_ascii_hexdigit())
        {
            run.push(byte);
            end += 3;
        }
        if run.is_empty() {
            index += 1;
            continue;
        }
        out.push_str(&url[copied..index]);
        decode_percent_run(&run, KEPT, &mut out);
        index = end;
        copied = end;
    }
    out.push_str(&url[copied..]);
    out
}

/// `%XX` with upper-case hex digits.
fn push_percent(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(char::from(HEX[usize::from(byte >> 4)]));
    out.push(char::from(HEX[usize::from(byte & 0xF)]));
}

/// `mdurl.decode`'s replacement of one run of percent-encoded bytes.
fn decode_percent_run(run: &[u8], kept: &[u8], out: &mut String) {
    let continuation = |byte: Option<&u8>| byte.is_some_and(|byte| byte & 0xC0 == 0x80);
    let mut index = 0;
    while index < run.len() {
        let first = run[index];
        if first < 0x80 {
            if kept.contains(&first) {
                push_percent(out, first);
            } else {
                out.push(char::from(first));
            }
            index += 1;
            continue;
        }
        let (len, min, mask) = match first {
            _ if first & 0xE0 == 0xC0 => (2, 0x80, 0x1F),
            _ if first & 0xF0 == 0xE0 => (3, 0x800, 0x0F),
            _ if first & 0xF8 == 0xF0 => (4, 0x1_0000, 0x07),
            _ => (1, 0, 0),
        };
        if len > 1
            && index + len <= run.len()
            && (1..len).all(|offset| continuation(run.get(index + offset)))
        {
            let mut value = u32::from(first & mask);
            for offset in 1..len {
                value = (value << 6) | u32::from(run[index + offset] & 0x3F);
            }
            match char::from_u32(value).filter(|_| value >= min) {
                Some(character) => out.push(character),
                None => (0..len).for_each(|_| out.push('\u{fffd}')),
            }
            index += len;
            continue;
        }
        out.push('\u{fffd}');
        index += 1;
    }
}

/// markdown-it's `validateLink`: `javascript:`, `vbscript:`, `file:` and
/// `data:` are refused, except `data:image/(gif|png|jpeg|webp);`.
pub(super) fn validate_link(url: &str) -> bool {
    let url = url.trim_matches(is_js_space).to_lowercase();
    let bad = ["vbscript:", "javascript:", "file:", "data:"]
        .iter()
        .any(|scheme| url.starts_with(scheme));
    !bad || ["gif", "png", "jpeg", "webp"]
        .iter()
        .any(|kind| url.starts_with(&format!("data:image/{kind};")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_node(text: &str, range: SourceRange) -> Inline {
        Inline::Text {
            text: text.to_owned(),
            range,
        }
    }

    /// The UTF-16 slice of `content` a range names.
    fn at(content: &str, range: &SourceRange) -> String {
        let units: Vec<u16> = content.encode_utf16().collect();
        String::from_utf16_lossy(&units[range.clone()])
    }

    fn paragraph(tree: &MarkdownTree) -> &[Inline] {
        match &tree.blocks[..] {
            [Block::Paragraph { children, .. }] => children,
            blocks => panic!("not one paragraph: {blocks:#?}"),
        }
    }

    /// `basic.html` (`web-elements/tests/fixtures/x-markdown/basic.html`),
    /// whose spec reads the `h1`, `strong`, `em`, two `li` and the `code`.
    #[test]
    fn basic_fixture() {
        let content = "# Title\n\nthis is **bold** and *italic*.\n\n- item 1\n- item 2\n\ninline code: `code`.\n\n```\nblock code\n```";
        let tree = parse(content, None);
        let [heading, emphasis, list, code, block] = &tree.blocks[..] else {
            panic!("{tree:#?}");
        };
        let Block::Heading {
            level: 1,
            children,
            range,
        } = heading
        else {
            panic!("{heading:?}")
        };
        assert_eq!(children[..], [text_node("Title", 2..7)]);
        assert_eq!(at(content, range).trim_end(), "# Title");
        let Block::Paragraph { children, .. } = emphasis else {
            panic!("{emphasis:?}")
        };
        let [
            Inline::Text { .. },
            Inline::Strong {
                children: bold,
                range: strong,
            },
            Inline::Text { .. },
            Inline::Emphasis { range: em, .. },
            Inline::Text { .. },
        ] = &children[..]
        else {
            panic!("{children:?}");
        };
        assert_eq!(bold[0].text_content(), "bold");
        assert_eq!(at(content, strong), "**bold**");
        assert_eq!(at(content, em), "*italic*");
        assert_eq!(emphasis.text_content(), "this is bold and italic.");
        let Block::List {
            start: None, items, ..
        } = list
        else {
            panic!("{list:?}")
        };
        assert_eq!(items.len(), 2);
        // A tight item's text is inline content of the item itself.
        assert!(
            matches!(&items[0].children[..], [Block::Inlines { children, .. }] if children[..] == [text_node("item 1", 43..49)])
        );
        let Block::Paragraph { children, .. } = code else {
            panic!("{code:?}")
        };
        assert!(
            matches!(&children[1], Inline::Code { text, range } if text == "code" && at(content, range) == "`code`")
        );
        assert_eq!(
            block,
            &Block::Code {
                language: None,
                text: "block code\n".to_owned(),
                range: 82..100
            }
        );
        assert_eq!(
            tree.text_content(),
            "Titlethis is bold and italic.item 1item 2inline code: code.block code\n"
        );
        // pulldown-cmark's list range runs over the blank line after it.
        assert_eq!(
            at(content, tree.blocks[2].range()),
            "- item 1\n- item 2\n\n"
        );
    }

    /// `table.html`.
    #[test]
    fn table_fixture() {
        let content = "\n| A | B |\n| - | - |\n| 1 | 2 |\n";
        let tree = parse(content, None);
        let [
            Block::Table {
                alignments,
                head,
                body,
                ..
            },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(alignments[..], [Alignment::None, Alignment::None]);
        let cells = |row: &TableRow| {
            row.cells
                .iter()
                .map(|cell| append(&cell.children))
                .collect::<Vec<_>>()
        };
        assert_eq!(cells(head), ["A", "B"]);
        assert_eq!(body.len(), 1);
        assert_eq!(cells(&body[0]), ["1", "2"]);
        let aligned = parse("| A | B | C |\n| :- | :-: | -: |\n", None);
        let [
            Block::Table {
                alignments, body, ..
            },
        ] = &aligned.blocks[..]
        else {
            panic!()
        };
        assert_eq!(
            alignments[..],
            [Alignment::Left, Alignment::Center, Alignment::Right]
        );
        // markdown-it writes no `<tbody>` for a table without body rows.
        assert!(body.is_empty());
    }

    fn append(inlines: &[Inline]) -> String {
        let mut text = String::new();
        append_inlines(inlines, &mut text);
        text
    }

    /// `html-tags.html`: the span keeps only its class, the `<p>` block
    /// becomes a paragraph with its class followed by the block's last line
    /// break, and the `<div>` block is its own text, with no line break at
    /// the end of the content (`tests/x-markdown.spec.ts`, "should only allow
    /// span and p html tags in markdown content").
    #[test]
    fn html_tags_fixture() {
        let content = "### Hello <span class=\"mark-red other\" style=\"color: red\" onclick=\"noop()\">World</span>\n\n<p class=\"mark-help\" style=\"font-size: 12px\">Paragraph</p>\n\n<div class=\"not-allowed\">Not allowed</div>";
        let tree = parse(content, None);
        let [
            Block::Heading {
                level: 3, children, ..
            },
            Block::HtmlParagraph {
                classes,
                children: paragraph,
                range,
            },
            Block::HtmlSpace {
                text: line_break,
                range: break_range,
            },
            Block::Inlines {
                children: refused, ..
            },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        let [
            Inline::Text { text: hello, .. },
            Inline::Html {
                element: HtmlElement::Span,
                classes: span,
                children: world,
                range: span_range,
            },
        ] = &children[..]
        else {
            panic!("{children:?}");
        };
        assert_eq!(hello, "Hello ");
        assert_eq!(span[..], ["mark-red", "other"]);
        assert_eq!(append(world), "World");
        assert!(
            at(content, span_range).starts_with("<span class")
                && at(content, span_range).ends_with("World</span>")
        );
        assert_eq!(classes[..], ["mark-help"]);
        assert_eq!(append(paragraph), "Paragraph");
        assert_eq!(
            at(content, range),
            "<p class=\"mark-help\" style=\"font-size: 12px\">Paragraph</p>"
        );
        assert_eq!(line_break, "\n");
        assert_eq!(at(content, break_range), "\n");
        assert_eq!(break_range.start, range.end);
        assert_eq!(
            append(refused),
            "<div class=\"not-allowed\">Not allowed</div>"
        );
        // A refused block keeps its line break in its text, and a `<p>` left
        // open holds it.
        let tree = parse("<div>x</div>\n\n<p>open\r\n", None);
        let [
            Block::Inlines {
                children: refused, ..
            },
            Block::HtmlParagraph { children, .. },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(append(refused), "<div>x</div>\n");
        assert_eq!(append(children), "open\n");
    }

    /// The spec's "should return full text ranges for nested markdown tags"
    /// content: both paragraphs, the `strong` and the class-less `span`.
    #[test]
    fn nested_tags() {
        let tree = parse(
            "Hello <span>nested only</span>\n\nHello **bold** tail",
            None,
        );
        let [first, second] = &tree.blocks[..] else {
            panic!("{tree:#?}")
        };
        assert_eq!(first.text_content(), "Hello nested only");
        assert_eq!(second.text_content(), "Hello bold tail");
        let Block::Paragraph { children, .. } = first else {
            panic!()
        };
        assert!(
            matches!(&children[1], Inline::Html { element: HtmlElement::Span, classes, children, .. } if classes.is_empty() && append(children) == "nested only")
        );
    }

    #[test]
    fn html_pairing() {
        // An inline tag left open closes with its construct; a later end tag
        // naming nothing open is ignored.
        let tree = parse("*a <span>b* c</span>", None);
        let [Inline::Emphasis { children, .. }, Inline::Text { text, .. }] = paragraph(&tree)
        else {
            panic!("{tree:#?}");
        };
        assert!(matches!(&children[1], Inline::Html { children, .. } if append(children) == "b"));
        assert_eq!(text, " c");
        // `</span>` inside a `p` inside a `span` is ignored; `<p>` closes
        // an open `p`; a stray `</p>` is an empty `p`.
        let tree = parse("<p class=a>one <span>two<p>three</span> four</p></p>", None);
        let [
            Block::HtmlParagraph {
                classes,
                children: one,
                ..
            },
            Block::HtmlParagraph {
                children: third, ..
            },
            Block::HtmlParagraph {
                children: empty, ..
            },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(classes[..], ["a"]);
        assert_eq!(append(one), "one two");
        assert_eq!(append(third), "three four");
        assert!(empty.is_empty());
        // A `span` alone on its line is an HTML block: inline content with
        // no paragraph, its text raw HTML (no emphasis), references decoded.
        let tree = parse(
            "<span class=\"x  y!\">\nfoo *bar* &amp; &#x41;\n</span>\n\nnext",
            None,
        );
        let [
            Block::Inlines { children, .. },
            Block::HtmlSpace {
                text: line_break, ..
            },
            Block::Paragraph { .. },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(line_break, "\n");
        let [
            Inline::Html {
                classes, children, ..
            },
        ] = &children[..]
        else {
            panic!("{children:?}")
        };
        assert_eq!(classes[..], ["x"]);
        assert_eq!(append(children), "\nfoo *bar* & A\n");
        // A comment refuses the fragment.
        let tree = parse("a <!-- x --> b", None);
        assert_eq!(paragraph(&tree)[..], [text_node("a <!-- x --> b", 0..14)]);
    }

    #[test]
    fn class_attribute() {
        assert_eq!(
            tag_classes(" class=\"a b-c d_e f.g\" id=x"),
            ["a", "b-c", "d_e"]
        );
        assert_eq!(tag_classes(" CLASS='q'"), ["q"]);
        assert_eq!(tag_classes(" data-class=z"), ["z"]);
        assert_eq!(tag_classes(" class=&quot;w&quot;"), ["w"]);
        assert!(tag_classes(" title=\"x\"").is_empty());
        assert_eq!(
            sanitized_markup("<SPAN class=\"a\" style=x/>"),
            "<span class=\"a\"></span>"
        );
        assert_eq!(sanitized_markup("<div>"), "&lt;div&gt;");
    }

    /// `inlineview.html`: the image becomes a slot for the light-DOM child.
    #[test]
    fn inline_view_fixture() {
        let content = "inline view here ![](inlineview://badge) next";
        let tree = parse(content, None);
        let [
            Inline::Text { .. },
            Inline::InlineView { id, range },
            Inline::Text { .. },
        ] = paragraph(&tree)
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(id, "badge");
        assert_eq!(at(content, range), "![](inlineview://badge)");
        assert_eq!(tree.text_content(), "inline view here  next");
        // `inlineview-class.html`.
        let tree = parse("![](inlineview://content-view)", None);
        assert!(
            matches!(paragraph(&tree), [Inline::InlineView { id, .. }] if id == "content-view")
        );
        // An empty id is an image.
        let tree = parse("![](inlineview://)", None);
        assert!(matches!(paragraph(&tree), [Inline::Image { src, .. }] if src == "inlineview://"));
    }

    #[test]
    fn image() {
        let tree = parse("![a `c` <span>b</span> *e*](x.png \"t\")", None);
        let [
            Inline::Image {
                src, alt, title, ..
            },
        ] = paragraph(&tree)
        else {
            panic!("{tree:#?}")
        };
        assert_eq!(src, "x.png");
        assert_eq!(alt, "a c <span>b</span> e");
        assert_eq!(title.as_deref(), Some("t"));
    }

    /// Bare links as web-core's markdown-it 15 links them: schemes and email
    /// addresses, not `www.` (linkify-it 6 has `fuzzyLink: false`).
    #[test]
    fn autolinks() {
        let content = "see https://example.com and www.example.com, example.com, a@b.com, mailto:c@d.com ftp://f.org/x HTTP://X.COM/a";
        let tree = parse(content, None);
        let links: Vec<(&str, String, String)> = paragraph(&tree)
            .iter()
            .filter_map(|inline| match inline {
                Inline::Link {
                    href,
                    children,
                    range,
                    ..
                } => Some((href.as_str(), append(children), at(content, range))),
                _ => None,
            })
            .collect();
        let expected = [
            ("https://example.com", "https://example.com"),
            ("mailto:a@b.com", "a@b.com"),
            ("mailto:c@d.com", "mailto:c@d.com"),
            ("ftp://f.org/x", "ftp://f.org/x"),
            ("HTTP://X.COM/a", "HTTP://X.COM/a"),
        ];
        assert_eq!(links.len(), expected.len(), "{links:?}");
        for ((href, shown, source), (expected_href, expected_text)) in links.iter().zip(expected) {
            assert_eq!(*href, expected_href);
            assert_eq!(shown, expected_text);
            assert_eq!(source, expected_text);
        }
        // Trailing punctuation and balanced parentheses.
        let content = "(https://en.wikipedia.org/wiki/Link_(Zelda)) and https://a.com/x.";
        let tree = parse(content, None);
        let hrefs: Vec<&str> = paragraph(&tree)
            .iter()
            .filter_map(|inline| match inline {
                Inline::Link { href, .. } => Some(href.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            hrefs,
            [
                "https://en.wikipedia.org/wiki/Link_(Zelda)",
                "https://a.com/x"
            ]
        );
        // Not inside a link, a code span, or a raw `<a>`.
        let tree = parse(
            "[http://a.com](http://b.com) `http://c.com` <a href=x>http://d.com</a>",
            None,
        );
        let links = paragraph(&tree)
            .iter()
            .filter(|inline| matches!(inline, Inline::Link { .. }))
            .count();
        assert_eq!(links, 1);
        // A percent-encoded link shows decoded, except for reserved bytes.
        let tree = parse("http://bar.com/%E4%BD%A0%2F <https://q.com/%20x>", None);
        let shown: Vec<String> = paragraph(&tree)
            .iter()
            .filter_map(|inline| match inline {
                Inline::Link { children, .. } => Some(append(children)),
                _ => None,
            })
            .collect();
        assert_eq!(shown, ["http://bar.com/你%2F", "https://q.com/ x"]);
    }

    #[test]
    fn link_destinations() {
        let tree = parse(
            "[ok](<a b> \"t\") [x](javascript:alert(1)) [*y*](vbscript:z) ![i](data:image/png;base64,AA)",
            None,
        );
        let inlines = paragraph(&tree);
        assert!(
            matches!(&inlines[0], Inline::Link { href, title, .. } if href == "a%20b" && title.as_deref() == Some("t"))
        );
        // A refused link is its source text, its label still parsed.
        let Inline::Text { text: refused, .. } = &inlines[1] else {
            panic!("{inlines:?}")
        };
        assert_eq!(refused, " [x](javascript:alert(1)) [");
        assert!(matches!(&inlines[2], Inline::Emphasis { .. }));
        assert!(matches!(&inlines[3], Inline::Text { text, .. } if text == "](vbscript:z) "));
        assert!(
            matches!(&inlines[4], Inline::Image { src, .. } if src == "data:image/png;base64,AA")
        );
        assert_eq!(
            normalize_link("http://例子.com/ä"),
            "http://%E4%BE%8B%E5%AD%90.com/%C3%A4"
        );
        assert_eq!(normalize_link_text("%C3%A4%2f%zz%E4"), "ä%2F%zz\u{fffd}");
        let tree = parse("<javascript:q>", None);
        assert_eq!(paragraph(&tree)[..], [text_node("<javascript:q>", 0..14)]);
    }

    #[test]
    fn strikethrough() {
        let content = "~~gone~~ and ~kept~";
        let tree = parse(content, None);
        let [
            Inline::Strikethrough { children, range },
            Inline::Text { text, range: rest },
        ] = paragraph(&tree)
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(append(children), "gone");
        assert_eq!(at(content, range), "~~gone~~");
        assert_eq!(text, " and ~kept~");
        assert_eq!(*rest, 8..19);
    }

    /// Offsets are UTF-16 code units: `😀` is two.
    #[test]
    fn utf16_ranges() {
        let content = "😀 é **b**\n\n中文 *c*";
        let tree = parse(content, None);
        let [
            Block::Paragraph { children, .. },
            Block::Paragraph {
                children: second,
                range,
            },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(children[0], text_node("😀 é ", 0..5));
        assert_eq!(*children[1].range(), 5..10);
        assert_eq!(*range, 12..18);
        assert_eq!(*second[1].range(), 15..18);
        assert_eq!(at(content, second[1].range()), "*c*");
    }

    #[test]
    fn content_range() {
        let content = "# A\n\nhello **world**";
        let tree = parse(content, Some((5, 20)));
        let [
            Inline::Text { range: hello, .. },
            Inline::Strong { range, .. },
        ] = paragraph(&tree)
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(*hello, 5..11);
        assert_eq!(at(content, range), "**world**");
        // Clamped to the string; an end before the start is empty.
        assert_eq!(slice_utf16("abc", 1, 100), ("bc", 1));
        assert_eq!(slice_utf16("abc", 5, 9), ("", 3));
        assert_eq!(slice_utf16("abc", 2, 1), ("", 2));
        // A bound inside a surrogate pair leaves the half character out.
        assert_eq!(slice_utf16("😀x😀", 1, 4), ("x", 2));
        assert_eq!(slice_utf16("😀x😀", 0, 3), ("😀x", 0));
        assert_eq!(utf16_len("😀x"), 3);
        assert_eq!(parse("😀x", Some((1, 3))).text_content(), "x");
        assert_eq!(parse_content_range(None), ContentRangeUpdate::Clear);
        assert_eq!(parse_content_range(Some("")), ContentRangeUpdate::Clear);
        assert_eq!(
            parse_content_range(Some("[1, 5]")),
            ContentRangeUpdate::Set(1, 5)
        );
        assert_eq!(
            parse_content_range(Some(" 2 ,7")),
            ContentRangeUpdate::Set(2, 7)
        );
        assert_eq!(
            parse_content_range(Some("[\"1\", 2.9]")),
            ContentRangeUpdate::Set(1, 2)
        );
        assert_eq!(
            parse_content_range(Some("[-3, 2]")),
            ContentRangeUpdate::Set(0, 2)
        );
        assert_eq!(
            parse_content_range(Some("0x10,Infinity")),
            ContentRangeUpdate::Set(16, usize::MAX)
        );
        for kept in ["[5, 1]", "abc", "5", "[1]", "1,2,3", "inf,2", "[{}, 1]"] {
            assert_eq!(
                parse_content_range(Some(kept)),
                ContentRangeUpdate::Keep,
                "{kept}"
            );
        }
    }

    /// A streaming append leaves the leading blocks equal.
    #[test]
    fn incremental_prefix() {
        let before = parse("# Title\n\nthis is", None);
        let after = parse("# Title\n\nthis is more\n\n## More", None);
        assert_eq!(before.blocks[0], after.blocks[0]);
        assert_ne!(before.blocks[1], after.blocks[1]);
        assert_eq!(after.blocks.len(), 3);
    }

    #[test]
    fn breaks_and_code() {
        let tree = parse("a  \nb\\\nc\nd", None);
        assert_eq!(tree.text_content(), "a\nb\nc\nd");
        assert!(matches!(paragraph(&tree)[1], Inline::HardBreak { .. }));
        assert!(matches!(paragraph(&tree)[5], Inline::SoftBreak { .. }));
        let tree = parse("```js extra\ncode\n```\n\n    indented\n\n---", None);
        let [
            Block::Code {
                language,
                text: fenced,
                ..
            },
            Block::Code {
                language: none,
                text: indented,
                ..
            },
            Block::ThematicBreak { .. },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        assert_eq!(language.as_deref(), Some("js"));
        assert_eq!(fenced, "code\n");
        assert_eq!(*none, None);
        assert_eq!(indented, "indented\n");
        let tree = parse("3. a\n4. b\n\n5. c\n\n> q\n> - r", None);
        let [
            Block::List {
                start: Some(3),
                items,
                ..
            },
            Block::Quote { children, .. },
        ] = &tree.blocks[..]
        else {
            panic!("{tree:#?}");
        };
        // A loose list's items hold paragraphs.
        assert!(matches!(&items[0].children[..], [Block::Paragraph { .. }]));
        assert!(matches!(
            &children[..],
            [Block::Paragraph { .. }, Block::List { start: None, .. }]
        ));
    }

    /// Nesting past [`MAX_NESTING`] is dropped, so no input builds a tree
    /// deep enough to exhaust the stack.
    #[test]
    fn nesting_limit() {
        fn depth(blocks: &[Block]) -> usize {
            blocks
                .iter()
                .map(|block| match block {
                    Block::Quote { children, .. } => 1 + depth(children),
                    _ => 1,
                })
                .max()
                .unwrap_or(0)
        }
        let quotes = format!("{} x", ">".repeat(5000));
        let tree = parse(&quotes, None);
        assert!(depth(&tree.blocks) <= MAX_NESTING);
        let spans = "<span>".repeat(5000);
        let _ = parse(&spans, None);
        let _ = parse(&format!("a {}b", "<span>".repeat(5000)), None);
        let _ = parse(&format!("{}a{}", "*".repeat(5000), "*".repeat(5000)), None);
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("a &amp; &lt;&gt; &#65;&#x42 &#0; &bogus; &"),
            "a & <> AB \u{fffd} &bogus; &"
        );
    }
}
