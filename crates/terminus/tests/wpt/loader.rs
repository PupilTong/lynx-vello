//! A WPT page → `terminus` document.
//!
//! The page is parsed by html5ever as a whole document. Every element is
//! imported with all of its attributes, so the test's own selectors
//! (`#id`, `.class`, `[attr]`, `:nth-child`, sibling combinators) match as
//! they do in a browser. `<script>`, `<style>`, `<link>`, `<meta>` and
//! `<title>` are imported too, without their text, so sibling selectors see
//! the same tree; the harness sheet hides them.
//!
//! The page's sheets are its `<style>` text and its local
//! `<link rel="stylesheet">` files, mounted as author sheets in document
//! order. `/fonts/ahem.css` is not read: the harness registers the vendored
//! Ahem face itself, which is the face that file names. The page's default
//! family is the vendored Roboto, so no result depends on the host's fonts.
//!
//! # The harness sheet
//!
//! A browser's HTML user-agent sheet makes `div` a block container, and the
//! tests rely on block flow around the flex and grid containers they test.
//! The engine's `lynx` grammar has no author `display: block`, and its
//! `(block, flow)` box lays block-level children out as atomic inlines, not
//! one under the other. The harness sheet (chrome rules enabled, user-agent
//! origin, so any author `display` wins) therefore lays an HTML block
//! element out as Lynx's `display: linear`, whose initial
//! `linear-direction` is `column` and whose `normal` cross alignment
//! stretches an auto-width child: children stack at the start edge and fill
//! the width, as block flow does, but margins do not collapse and there are
//! no floats. An element whose content is only text and inline elements
//! keeps `display: block` instead, the engine's inline formatting context,
//! so its text wraps as a paragraph. The root gets `width: 100%`: CSS
//! sizes an auto-width root element to the initial containing block, while
//! the engine shrinks it to its content, and every block under it would
//! shrink with it. The importer marks those elements with
//! [`IFC_ATTRIBUTE`]. An author `display: block`, `flow-root` or
//! `list-item` (in a sheet, a `style` attribute, or a script's style
//! change) is rewritten to `display: linear` for the same reason
//! ([`emulate_block`]).
//!
//! Whitespace-only text is kept inside inline content (an inline element,
//! or a block element marked as holding only inline content) and dropped
//! elsewhere, where a browser renders none. `<br>` holds a newline that its
//! `white-space: pre-line` preserves, which is the forced break.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, parse_document};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use terminus::link::{Document, FontBlob, NodeId, StylesheetOrigin};

use crate::common::device;

pub(crate) const VIEWPORT_WIDTH: f32 = 800.0;
pub(crate) const VIEWPORT_HEIGHT: f32 = 600.0;

/// Marks an HTML block element whose children are all inline-level.
pub(crate) const IFC_ATTRIBUTE: &str = "data-wpt-harness-ifc";

const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

/// The default face: vendored, so a page's text does not depend on the
/// host's fonts and the expectations hold on every machine.
const ROBOTO: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Roboto-Regular.ttf");

/// HTML elements a browser makes block-level.
const BLOCK_TAGS: &[&str] = &[
    "html",
    "body",
    "div",
    "p",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "form",
    "figure",
    "figcaption",
    "blockquote",
    "pre",
    "center",
    "address",
    "hr",
    "fieldset",
    "legend",
    "details",
    "summary",
    "menu",
];

/// HTML elements a browser makes inline-level, whose whitespace-only text
/// is kept.
const INLINE_TAGS: &[&str] = &[
    "span", "a", "b", "i", "em", "strong", "code", "small", "big", "sub", "sup", "label", "font",
    "tt", "u", "s", "abbr", "cite", "q", "var", "kbd", "samp", "mark", "bdi", "bdo",
];

/// Rewrites an author `display: block`, `flow-root` or `list-item` — values
/// the `lynx` grammar does not accept, which a browser lays out as block
/// flow — to `display: linear`, the harness's block emulation. Two-value
/// forms (`block flex`) are left alone.
pub(crate) fn emulate_block(css: &str) -> String {
    let lower = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = lower[search..].find("display") {
        let start = search + found;
        search = start + "display".len();
        let preceded = lower[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '-');
        if preceded {
            continue;
        }
        let rest = &lower[search..];
        let after_ws = rest.len() - rest.trim_start().len();
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let value_ws = rest.len() - rest.trim_start().len();
        let value = rest.trim_start();
        let Some(keyword) = ["flow-root", "list-item", "block"]
            .into_iter()
            .find(|keyword| value.starts_with(keyword))
        else {
            continue;
        };
        let tail = value[keyword.len()..].trim_start();
        if !(tail.is_empty() || tail.starts_with([';', '}', '!', '"', '\''])) {
            continue;
        }
        let value_start = search + after_ws + 1 + value_ws;
        out.push_str(&css[copied..value_start]);
        out.push_str("linear");
        copied = value_start + keyword.len();
        search = copied;
    }
    out.push_str(&css[copied..]);
    out
}

/// Elements that are never rendered.
const HIDDEN_TAGS: &[&str] = &[
    "head", "script", "style", "link", "meta", "title", "template", "base", "noscript",
];

const HARNESS_SHEET: &str = r"
html, body, div, p, section, article, header, footer, main, nav, aside, ul, ol,
li, dl, dt, dd, h1, h2, h3, h4, h5, h6, form, figure, figcaption, blockquote,
pre, center, address, hr, fieldset, legend, details, summary, menu {
    display: linear;
}
[data-wpt-harness-ifc] { display: block; }
head, script, style, link, meta, title, template, base, noscript { display: none; }
span, a, b, i, em, strong, code, small, big, sub, sup, label, font, tt, u, s,
abbr, cite, q, var, kbd, samp, mark, bdi, bdo, br, wbr, img { display: inline; }
br { white-space: pre-line; }
html { font-family: Roboto; width: 100%; }
body { margin: 8px; }
p, dl, blockquote, figure, pre { margin-top: 1em; margin-bottom: 1em; }
h1 { font-size: 2em; margin-top: 0.67em; margin-bottom: 0.67em; font-weight: bold; }
h2 { font-size: 1.5em; margin-top: 0.83em; margin-bottom: 0.83em; font-weight: bold; }
h3 { font-size: 1.17em; margin-top: 1em; margin-bottom: 1em; font-weight: bold; }
ul, ol, menu { margin-top: 1em; margin-bottom: 1em; padding-left: 40px; }
dd { margin-left: 40px; }
b, strong { font-weight: bold; }
i, em, cite, var { font-style: italic; }
pre, code, tt, kbd, samp { font-family: monospace; }
pre { white-space: pre; }
center { text-align: center; }
hr { border: 1px inset gray; margin-top: 0.5em; margin-bottom: 0.5em; }
fieldset { margin-left: 2px; margin-right: 2px; padding: 0.35em 0.75em 0.625em;
           border: 2px groove ThreeDFace; }
";

/// A loaded page.
pub(crate) struct Page {
    pub(crate) dom: Document<()>,
    pub(crate) body: Option<NodeId>,
    /// The `onload` attribute of `<body>`, if any.
    pub(crate) body_onload: Option<String>,
    /// Inline script text, in document order.
    pub(crate) scripts: Vec<String>,
    /// `src` of each external script.
    pub(crate) script_srcs: Vec<String>,
    /// Whether the root has `class="reftest-wait"` or similar.
    pub(crate) html_classes: Vec<String>,
    /// `(rel, href)` of every `<link>`.
    pub(crate) links: Vec<(String, String)>,
    /// `(name, content)` of every `<meta>`.
    pub(crate) metas: Vec<(String, String)>,
    /// Whether any element other than `<body>` has an `on*` attribute.
    pub(crate) has_event_handlers: bool,
}

/// What the importer found while walking the source tree.
#[derive(Default)]
struct Collected {
    sheets: Vec<String>,
    body: Option<NodeId>,
    body_onload: Option<String>,
    scripts: Vec<String>,
    script_srcs: Vec<String>,
    html_classes: Vec<String>,
    links: Vec<(String, String)>,
    metas: Vec<(String, String)>,
    has_event_handlers: bool,
}

/// Where the suite's files live, for root-relative URLs.
pub(crate) struct Paths {
    pub(crate) root: PathBuf,
}

impl Paths {
    /// Resolves `href` against the file at `base`.
    pub(crate) fn resolve(&self, base: &Path, href: &str) -> Option<PathBuf> {
        let href = href.split(['?', '#']).next()?;
        if href.is_empty() || href.contains("://") || href.starts_with("data:") {
            return None;
        }
        let path = if let Some(rooted) = href.strip_prefix('/') {
            self.root.join(rooted)
        } else {
            base.parent()?.join(href)
        };
        Some(path)
    }
}

/// Reads and parses one page.
pub(crate) fn load(paths: &Paths, file: &Path) -> Result<Page, String> {
    let bytes = std::fs::read(file).map_err(|error| format!("read {}: {error}", file.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let parsed = parse_document(RcDom::default(), ParseOpts::default()).one(text.as_ref());

    let html = parsed
        .document
        .children
        .borrow()
        .iter()
        .find(|node| matches!(&node.data, NodeData::Element { .. }))
        .cloned()
        .ok_or("no document element")?;

    let mut dom = Document::new(device(VIEWPORT_WIDTH, VIEWPORT_HEIGHT), "html", ());
    dom.add_stylesheet(HARNESS_SHEET, StylesheetOrigin::UserAgent);
    assert_eq!(dom.register_fonts(FontBlob::from_static(AHEM)), 1);
    assert_eq!(dom.register_fonts(FontBlob::from_static(ROBOTO)), 1);

    let root = dom.document_element().id();
    let mut collected = Collected::default();
    let collected_cell = RefCell::new(&mut collected);
    import_onto(&mut dom, root, &html, paths, file, &collected_cell);

    for sheet in &collected.sheets {
        dom.add_stylesheet(sheet, StylesheetOrigin::Author);
    }

    Ok(Page {
        dom,
        body: collected.body,
        body_onload: collected.body_onload,
        scripts: collected.scripts,
        script_srcs: collected.script_srcs,
        html_classes: collected.html_classes,
        links: collected.links,
        metas: collected.metas,
        has_event_handlers: collected.has_event_handlers,
    })
}

fn local_name(handle: &Handle) -> Option<String> {
    match &handle.data {
        NodeData::Element { name, .. } => Some(name.local.as_ref().to_ascii_lowercase()),
        _ => None,
    }
}

fn attr(handle: &Handle, wanted: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &handle.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref().eq_ignore_ascii_case(wanted))
        .map(|attr| attr.value.to_string())
}

fn text_of(handle: &Handle) -> String {
    let mut out = String::new();
    for child in handle.children.borrow().iter() {
        if let NodeData::Text { contents } = &child.data {
            out.push_str(&contents.borrow());
        }
    }
    out
}

/// Whether every child of `handle` is text or an inline-level element, and
/// at least one carries non-blank text or is an element.
fn has_only_inline_content(handle: &Handle) -> bool {
    let mut any = false;
    for child in handle.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } if !contents.borrow().trim().is_empty() => {
                any = true;
            }
            NodeData::Element { .. } => {
                let tag = local_name(child).unwrap_or_default();
                if HIDDEN_TAGS.contains(&tag.as_str()) {
                    continue;
                }
                if BLOCK_TAGS.contains(&tag.as_str()) {
                    return false;
                }
                // An unknown or inline tag counts as inline unless its own
                // inline style gives it a box display.
                if attr(child, "style").is_some_and(|style| {
                    let style = style.to_ascii_lowercase();
                    style.contains("display")
                        && !style.contains("display: inline")
                        && !style.contains("display:inline")
                }) {
                    return false;
                }
                any = true;
            }
            _ => {}
        }
    }
    any
}

#[allow(
    clippy::too_many_lines,
    reason = "one match over the element kinds the importer treats specially"
)]
fn import_onto(
    dom: &mut Document<()>,
    id: NodeId,
    source: &Handle,
    paths: &Paths,
    file: &Path,
    collected: &RefCell<&mut Collected>,
) {
    let tag = local_name(source).unwrap_or_default();
    if let NodeData::Element { attrs, .. } = &source.data {
        for attribute in attrs.borrow().iter() {
            let name = attribute.name.local.as_ref().to_ascii_lowercase();
            let value = attribute.value.as_ref();
            match name.as_str() {
                "style" => dom.set_inline_style(id, &emulate_block(value)),
                "class" => dom.set_classes(id, value),
                "id" => dom.set_id_attribute(id, Some(value)),
                _ => dom.set_attribute(id, &name, value),
            }
            if name.starts_with("on") {
                if tag == "body" && name == "onload" {
                    collected.borrow_mut().body_onload = Some(value.to_owned());
                } else {
                    collected.borrow_mut().has_event_handlers = true;
                }
            }
        }
    }
    let inline_content =
        BLOCK_TAGS.contains(&tag.as_str()) && tag != "html" && has_only_inline_content(source);
    if inline_content {
        dom.set_attribute(id, IFC_ATTRIBUTE, "");
    }
    let keeps_whitespace = inline_content || INLINE_TAGS.contains(&tag.as_str());
    match tag.as_str() {
        "html" => {
            if let Some(classes) = attr(source, "class") {
                collected.borrow_mut().html_classes =
                    classes.split_whitespace().map(str::to_owned).collect();
            }
        }
        "body" => collected.borrow_mut().body = Some(id),
        "style" => {
            // An XHTML file wraps its sheet in a CDATA section, which the
            // HTML parser leaves in the text.
            let css = text_of(source).replace("<![CDATA[", "").replace("]]>", "");
            collected.borrow_mut().sheets.push(emulate_block(&css));
            return;
        }
        "script" => {
            if let Some(src) = attr(source, "src") {
                collected.borrow_mut().script_srcs.push(src);
            } else {
                collected.borrow_mut().scripts.push(text_of(source));
            }
            return;
        }
        "link" => {
            let rel = attr(source, "rel").unwrap_or_default().to_ascii_lowercase();
            let href = attr(source, "href").unwrap_or_default();
            if rel.split_whitespace().any(|rel| rel == "stylesheet")
                && !href.ends_with("/fonts/ahem.css")
                && let Some(path) = paths.resolve(file, &href)
                && let Ok(css) = std::fs::read_to_string(&path)
            {
                collected.borrow_mut().sheets.push(emulate_block(&css));
            }
            collected.borrow_mut().links.push((rel, href));
            return;
        }
        "meta" => {
            let name = attr(source, "name")
                .unwrap_or_default()
                .to_ascii_lowercase();
            let content = attr(source, "content").unwrap_or_default();
            collected.borrow_mut().metas.push((name, content));
            return;
        }
        "title" | "template" | "noscript" => return,
        // A forced line break: a newline the harness sheet preserves.
        // Between two anonymous blocks the break is already there.
        "br" => {
            let parent = dom.get(id).and_then(terminus::link::Node::parent_id);
            let in_inline_content = parent
                .and_then(|parent| dom.get(parent))
                .is_some_and(|node| {
                    node.attribute(IFC_ATTRIBUTE).is_some()
                        || node
                            .tag_name()
                            .is_some_and(|tag| INLINE_TAGS.contains(&tag))
                });
            if in_inline_content {
                let text = dom.create_text_node("\n", ());
                dom.append_child(id, text);
            }
            return;
        }
        _ => {}
    }

    for child in source.children.borrow().iter() {
        match &child.data {
            NodeData::Element { .. } => {
                let child_tag = local_name(child).unwrap_or_default();
                let child_id = dom.create_element(&child_tag, ());
                dom.append_child(id, child_id);
                import_onto(dom, child_id, child, paths, file, collected);
            }
            NodeData::Text { contents } => {
                let text = contents.borrow();
                if text.trim().is_empty() && !keeps_whitespace {
                    continue;
                }
                let child_id = dom.create_text_node(text.as_ref(), ());
                dom.append_child(id, child_id);
            }
            _ => {}
        }
    }
}
