//! The `<markdown>` / `<x-markdown>` tags, web-core's `x-markdown`
//! (`lynx-stack/packages/web-platform/web-elements/src/elements/XMarkdown/`):
//! an element that renders its `content` attribute as Markdown in its UA
//! shadow tree, with `link`, `imageTap` and `parseEnd` events and UI methods
//! that read the rendered text.
//!
//! A Lynx-only component with no W3C equivalent, so `AGENTS.md`'s standards
//! policy puts it in the second bucket: this module implements what web-core
//! does with it, and native where web-core has nothing.
//!
//! # Two tag names, one component
//!
//! Native registers `markdown` (`lynx_xelement/markdown/LynxUIMarkdown.m`,
//! the typings' `<markdown>`, `types/common/element/markdown.d.ts`) and
//! web-core `x-markdown` (`XMarkdown.ts`, `@Component('x-markdown', ...)`).
//! A compiled card writes the JSX tag verbatim here, so both are defined
//! ([`define`]), as [`super::overlay`] defines its two.
//!
//! # What is built
//!
//! [`parse`] turns `content` (and `content-range`) into an owned tree of
//! blocks and inlines, replicating markdown-it, web-core's raw-HTML policy
//! and `DOMPurify` as data; [`style`] turns `markdown-style` into the text of a
//! stylesheet. This module builds the shadow tree from them ([`build`]),
//! counts its rendered text ([`rendered`]), and answers the UI methods
//! ([`methods`]).
//!
//! ## The shadow tree
//!
//! web-core's template (`htmlTemplates.ts:280-364`): two style elements and
//! `<div id="markdown-root" part="root" class="markdown-body">`, which the
//! rendered HTML replaces the children of. Here [`Markdown::constructed`]
//! attaches an open shadow root, adds two shadow stylesheets — [`SHADOW_RULES`],
//! the template's `<style>`, first, and the `markdown-style` sheet second, so
//! an author's `markdown-style` rule wins on equal specificity as it does in
//! web-core — and creates the root `div`.
//!
//! The rendered blocks are the root's children, by the HTML names
//! markdown-it writes (`h1`-`h6`, `p`, `blockquote`, `ul`/`ol`/`li`,
//! `pre > code`, `hr`, `table > thead/tbody > tr > th/td`, and inline
//! `strong`, `em`, `s`, `code`, `a[href][title]`, `span[class]`, `br`), so
//! [`style`]'s selectors match them; text is real DOM text nodes. An image is
//! the engine's `image` element with `src` and `auto-size`, where web-core's
//! tree has an `<img>`; an inline view is web-core's
//! `span.md-inline-view > slot[name=<id>]`, and the host's light child with
//! that `slot` renders in the line. [`build`] lists the two element kinds of
//! the engine's own (a line holder and a list marker).
//!
//! ## The shadow sheet
//!
//! [`SHADOW_RULES`] is web-core's template `<style>` and `x-markdown.css`,
//! translated to this engine's boxes, plus the browser UA defaults web-core
//! gets for free (heading sizes, bold, italic, the list indent), each rule
//! with its source. In outline:
//!
//! - **Paragraphs.** There is no block or inline flow here. A block-level text holder (`p`,
//!   `h1`-`h6`, `th`, `td`, `pre > code`, the line holder, the list marker) is `display:
//!   -lynx-text`, one paragraph each; an inline element (`a`, `strong`, `em`, `s`, `code`, `span`)
//!   is `display: -lynx-text` too, and inside a paragraph that makes it a nested run. One rule
//!   covers both: whether an element establishes a paragraph is decided by its parent, so `code` is
//!   a paragraph in `pre` and a run in `p`. Plain declarations, unlike the `text` tag's
//!   `!important` (`docs/style-assumptions.md` §D.15), so a `markdown-style` `display` still
//!   applies.
//! - **Blocks.** `#markdown-root`, `blockquote`, `ul`, `ol`, `li` and `pre` are column flex boxes,
//!   the block containers a browser makes them. Flex items' margins do not collapse, so the gap
//!   between a paragraph (`margin-bottom: 0.75em`) and a heading after it (`margin-top: 0.8em`) is
//!   their sum here where a browser takes the larger.
//! - **Colour.** The paragraphs are not `text`, so `text`'s `color: initial` does not reach them:
//!   they inherit the host's colour, as web-core's `.markdown-body { color: inherit }` does.
//! - **Lists** have no `display: list-item` or `::marker` here. Each `li` holds a marker element,
//!   `•`, `◦` or `▪` by how many lists enclose its list (the browser's `disc`, `circle` and
//!   `square`) or `N.` from the list's `start`, absolutely positioned at the item's start edge in
//!   the list's 40px padding, as an outside marker sits. An approximation of the browser's markers:
//!   a glyph rather than a drawn disc, its gap a padding.
//! - **Tables** have no `display: table` in hughie. A `table` is a grid of `N` `auto` columns
//!   (written out, hughie having no `repeat()`), shrink-wrapped, with the browser's 2px
//!   `border-spacing` as gap and padding; `thead`, `tbody` and `tr` are `display: contents`, so the
//!   cells are its grid items, placed row by row; `th` is bold and centred, a column's alignment an
//!   inline `text-align` on its cells, as markdown-it's `style` attribute. An approximation: rows
//!   do not size as table rows do across a cell that spans more than its column's share.
//! - **`pre`** is web-core's `overflow: auto`, written `overflow-x: scroll; overflow-y: hidden`
//!   (this engine has no `auto`), and its code keeps line breaks (`white-space-collapse:
//!   preserve-breaks`, the one preserving mode hughie has) without wrapping; runs of spaces still
//!   collapse, so an indented line loses its indent.
//! - **Images** are atomic boxes in the line, sized from their bitmap by `auto-size` (whose UA rule
//!   already caps the width at 100%) where web-core makes an `<img>` `display: block`: an image
//!   with text beside it in one paragraph shares that text's line.
//! - **`br`** generates its line break (`content: "\A"` with breaks preserved), as `raw-text`
//!   generates its run ([`super::raw_text`]); the `"\n"` text node markdown-it writes after it then
//!   collapses at the start of the next line.
//!
//! The host's own box is `x-markdown.css`: [`UA_RULES`] make both tags a
//! column flex box that stretches the root and inherits `color`; they keep
//! the box block every container tag gets ([`super::ua_sheet`]), a border box
//! that clips, where web-core's host is in no such block.
//!
//! ## Rendering
//!
//! Synchronously, inside the attribute callback that changed what is
//! rendered (`content` or `content-range`): web-core renders from a microtask
//! only because its parser loads lazily. Every render parses the content
//! again and reconciles against the previous parse: the shadow children of
//! the longest equal prefix of top-level blocks stay, the rest are removed
//! and the blocks from the first that differs are built anew. Content
//! streamed by appending therefore keeps the elements of every block it did
//! not change, which is what web-core's incremental append buys
//! (`#appendIncrementally`, `XMarkdownAttributes.ts:759-782`) without its
//! cut at the last line break and its flush timer: here the block a partial
//! line belongs to is rebuilt on each append, and markup an append completes
//! (a closing `**`) restyles what came before it, as a full render does.
//! Empty `content` empties the root.
//!
//! Each render of non-empty content queues `parseEnd` at the host with
//! `{id}`, the `content-id` attribute (`#dispatchParseEnd`,
//! `XMarkdownAttributes.ts:993-1001`), bubbling and composed. web-core skips
//! it for an append that ends with a line break and for empty content.
//!
//! ## Events
//!
//! A `tap` at the shadow content ([`dom::event::ElementEventKind::Tap`], at
//! the hit node, heard in the bubble pass) becomes web-core's click handling
//! (`#handleClick`, `XMarkdownAttributes.ts:620-651`), both events bubbling
//! and composed:
//!
//! - **`link`** `{url, content, contentId}`, the anchor's `href`, its text and the `content-id`:
//!   when the hit node is inside an `a` (web-core's `closest('a')`), or when the tap lands on a
//!   character of a paragraph that an `a` holds. A paragraph is one box here, its runs none, so the
//!   second is how a tap on link text finds its anchor: the tap point in the paragraph's content
//!   box, [`dom::Document::text_index_at_point`], and the innermost `a` whose
//!   [`dom::Document::inline_source_range`] holds that unit.
//! - **`imageTap`** `{url, contentId}`, the image's `src`: when the hit node is an image not inside
//!   a link.
//!
//! A tap on anything else queues nothing.
//!
//! ## UI methods
//!
//! [`Markdown::invoke`], each in [`methods`]: `getContent`, `getImages`,
//! `getParseResult`, `getTextBoundingRect` and `getCharIndexByPoint`.
//! `getImages` and `getParseResult` answer the typings' shapes (`{images}`,
//! `{id, result}`) where web-core answers the bare value; `getContent`'s
//! `end` is web-core's inclusive one, not native's exclusive one; the
//! offsets are web-core's rendered-text offsets, which [`rendered`] counts
//! with markdown-it's line breaks between block tags (`docs/tracking/deviations.md`).
//!
//! Not built, answering code 3 as the `text` component's unbuilt methods do
//! (the kind has no method of the name):
//!
//! - `pauseAnimation` and `resumeAnimation`: the typewriter animation is deferred; it needs the
//!   component timer infrastructure of PR #408.
//! - `getSelectedText` and `setTextSelection`: there is no text selection model.
//!
//! ## State
//!
//! Per element, the component keeps the parse its shadow tree was built
//! from, the elements each top-level block built, the `content-range` in
//! force (an invalid value keeps the previous one, `_handleContentRange`,
//! `XMarkdownAttributes.ts:1255-1282`) and the line breaks [`build`] recorded
//! — keyed by the host's [`NodeId`], which `dom` never reissues. `dom` has no
//! callback for an element being freed, and `disconnected_callback` is not
//! that (a removed element can come back), so the entries of freed hosts are
//! dropped when the next markdown element is constructed: a stale entry
//! answers nothing, since no live element has its id.
//!
//! # Deferred
//!
//! Not implemented, each for the reason given:
//!
//! - The typewriter animation — `animation-type`, `animation-velocity`, `animation-paused`,
//!   `initial-animation-step`, `content-complete`, `typewriter-dynamic-height`,
//!   `typewriter-height-transition-duration`, `animation-frame-rate`, `markdown-style`'s
//!   `typewriterCursor`, and the `drawStart`, `drawEnd` and `animationStep` events: it needs a
//!   component timer (PR #408).
//! - Truncation — `text-maxline`, the `overflow` event and `markdown-style`'s `truncation` marker:
//!   a clamp across many paragraphs, which the per-paragraph `text-maxline` cannot express.
//! - `markdown-effect` (web-core's text mask): no counterpart for `background-clip: text` here.
//! - Selection — `text-selection`, `enable-selection`, `selectionchange`, selection colours and
//!   handles: no selection model.
//! - `childrenexpose` and `exposure-tags`: native-only exposure of rendered children.
//! - `allow-break-around-punctuation`, `image-downsampling`, `text-mark-attachments` and
//!   `markdown-max-height`: native-only attributes web-core does not read.
//!
//! [`NodeId`]: dom::NodeId

mod build;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod card_tests;
mod methods;
pub(super) mod parse;
mod rendered;
pub(super) mod style;
#[cfg(test)]
mod tests;

use std::cell::RefCell;

use dom::event::{ElementEvent, ElementEventKind, EventPhase};
use dom::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, ShadowRootMode};
use rustc_hash::FxHashMap;

use self::build::{Builder, IMAGE_TAG, Newlines};
use super::{ComponentDetail, ComponentEvents, InvalidParams, LynxDocument};

/// Native's tag, and the JSX tag the typings declare.
pub(super) const MARKDOWN_TAG: &str = "markdown";
/// web-core's tag.
pub(super) const X_MARKDOWN_TAG: &str = "x-markdown";
/// The Markdown source.
const CONTENT_ATTRIBUTE: &str = "content";
/// A UTF-16 slice of `content` to render instead of all of it.
const CONTENT_RANGE_ATTRIBUTE: &str = "content-range";
/// The id the events and `getParseResult` report.
const CONTENT_ID_ATTRIBUTE: &str = "content-id";
/// The JSON style object [`style`] translates.
const MARKDOWN_STYLE_ATTRIBUTE: &str = "markdown-style";
/// The root's id, and the part name web-core exports it under.
const ROOT_ID: &str = "markdown-root";
/// The root's class, which every [`style`] selector starts from.
const ROOT_CLASS: &str = "markdown-body";
/// Where the `markdown-style` sheet sits among the shadow root's sheets:
/// after [`SHADOW_RULES`].
const MARKDOWN_STYLE_SHEET: usize = 1;

/// A method the tests read the rendered text through, which no build but a
/// test's answers.
#[cfg(test)]
const RENDERED_TEXT_METHOD: &str = "renderedText";

/// The host's own rules, from `x-markdown.css`, for both tags.
pub(super) const UA_RULES: &str = "\
markdown, x-markdown { display: flex; flex-direction: column; align-items: stretch; color: inherit; }
markdown::part(root), x-markdown::part(root) { width: 100%; }
";

/// The shadow sheet, added first to each instance's shadow root: web-core's
/// template `<style>` (`htmlTemplates.ts:280-362`) in this engine's boxes,
/// after the HTML rendering defaults it relies on. See the module
/// documentation for the outline; each rule says where it comes from.
///
/// The HTML defaults are a browser's UA sheet under web-core, below every
/// template rule by origin. Here they share the template's origin, so they
/// are written with their selectors in `:where()`, at zero specificity, and
/// first: every template and `markdown-style` rule outranks them.
const SHADOW_RULES: &str = r#"
/* HTML §15.3.6 (sections and headings): font sizes and weight. */
:where(h1) { font-size: 2em; }
:where(h2) { font-size: 1.5em; }
:where(h3) { font-size: 1.17em; }
:where(h5) { font-size: 0.83em; }
:where(h6) { font-size: 0.67em; }
:where(h1, h2, h3, h4, h5, h6, th) { font-weight: bold; }
/* HTML §15.3.4 (phrasing content): strong, em, s, code. */
:where(strong) { font-weight: bolder; }
:where(em) { font-style: italic; }
:where(s) { text-decoration: line-through; }
/* HTML §15.3.3 and §15.3.8: blockquote's indent, the lists' 40px
   padding the outside marker sits in, pre's monospace. */
:where(blockquote) { margin: 1em 40px; }
:where(ul, ol) { padding-left: 40px; }
:where(pre) { font-family: monospace; }
/* HTML §15.3.9 (tables): 2px border-spacing, between the cells and around
   them, here the grid's gap and padding; 1px cell padding; th centred. */
:where(th, td) { padding: 1px; }
:where(th) { text-align: center; }
/* HTML §15.3.10 (hr): a 1px inset border around nothing. `margin: 0.5em
   auto` is `0.5em 0`: auto inline margins on a stretched flex item would
   shrink it to nothing. */
:where(hr) { display: flex; height: 0; margin: 0.5em 0; border: 1px inset; color: gray; }

/* The boxes: block containers are column flex boxes, text holders and
   inline elements -lynx-text (a paragraph where the parent is a container,
   a nested run inside a paragraph), the table a grid of its cells. */
:where(blockquote, ul, ol, li, pre) { display: flex; flex-direction: column; }
:where(p, h1, h2, h3, h4, h5, h6, th, td, code, a, strong, em, s, span, md-line, md-marker) { display: -lynx-text; }
:where(table) { display: grid; align-self: flex-start; gap: 2px; padding: 2px; }
:where(thead, tbody, tr, slot) { display: contents; }
/* The marker: the browser's outside ::marker, at the item's start edge. */
:where(li) { position: relative; }
:where(md-marker) { position: absolute; top: 0; right: 100%; padding-right: 0.5em; text-wrap-mode: nowrap; }
/* br: its line break, generated as raw-text generates its run. */
:where(br) { display: -lynx-text; content: "\A"; white-space-collapse: preserve-breaks; }

/* htmlTemplates.ts:284-290; display: block is a column flex box. */
.markdown-body {
  display: flex; flex-direction: column;
  line-height: 1.4; color: inherit; word-break: break-word; position: relative;
}
/* :291-293. */
.markdown-body p { margin: 0 0 0.75em 0; }
/* :294-301. */
.markdown-body h1, .markdown-body h2, .markdown-body h3,
.markdown-body h4, .markdown-body h5, .markdown-body h6 { margin: 0.8em 0 0.4em 0; }
/* :302-305. */
.markdown-body ul, .markdown-body ol { margin: 0 0 0.75em 1.5em; }
/* :306-312; overflow: auto is overflow-x: scroll, the block axis hidden. */
.markdown-body pre {
  margin: 0 0 0.75em 0; padding: 8px 12px; border-radius: 6px; background: #f6f8fa;
  overflow-x: scroll; overflow-y: hidden;
}
/* :313-316. */
.markdown-body code {
  font-family: ui-monospace, SFMono-Regular, SFMono, Menlo, Consolas, "Liberation Mono", monospace;
}
/* :317-320; display: block is the paragraph, which keeps the code's line
   breaks (HTML's pre is white-space: pre) and sizes to its longest line so
   pre scrolls it. */
.markdown-body pre code {
  display: -lynx-text; padding: 0;
  white-space-collapse: preserve-breaks; text-wrap-mode: nowrap; align-self: flex-start;
}
/* :321-325, for the engine's image: height: auto is the default and
   display: block is not built (an image is an atomic box in its line). */
.markdown-body image { max-width: 100%; }
/* :326-329; inline-block is the run the slotted view sits in. */
.markdown-body .md-inline-view { display: -lynx-text; vertical-align: baseline; }
/* :358-361. */
.markdown-body a { color: #0366d6; text-decoration: underline; }
"#;

/// Installs the component under both tag names, over the queue its events
/// go into. Must run after [`super::image::define`], whose component the
/// shadow tree's images are, and before any element could carry either tag,
/// which is [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents) {
    document.define(MARKDOWN_TAG, Box::new(Markdown::new(events.clone())));
    document.define(X_MARKDOWN_TAG, Box::new(Markdown::new(events)));
}

/// The markdown component: builds the shadow tree, re-renders it on
/// `content` and `content-range`, swaps the `markdown-style` sheet, turns
/// taps into `link` and `imageTap`, and answers the UI methods. See the
/// module documentation.
struct Markdown {
    events: ComponentEvents,
    states: RefCell<FxHashMap<NodeId, State>>,
}

/// What one element's shadow tree was built from.
#[derive(Default)]
struct State {
    /// The `#markdown-root` element.
    root: Option<NodeId>,
    /// The parse the root's children were built from.
    document: parse::Document,
    /// The root's children, one per top-level block of [`Self::document`].
    elements: Vec<NodeId>,
    /// The `content-range` in force.
    range: Option<(usize, usize)>,
    /// The `markdown-style` sheet's text in force.
    css: String,
    /// markdown-it's line breaks around the built elements.
    newlines: FxHashMap<NodeId, Newlines>,
}

impl Markdown {
    fn new(events: ComponentEvents) -> Self {
        Self {
            events,
            states: RefCell::new(FxHashMap::default()),
        }
    }

    /// Takes `element`'s state out for the length of a mutation, so a
    /// reaction the mutation runs can never find it borrowed.
    fn take_state(&self, element: NodeId) -> State {
        self.states
            .borrow_mut()
            .remove(&element)
            .unwrap_or_default()
    }

    fn put_state(&self, element: NodeId, state: State) {
        self.states.borrow_mut().insert(element, state);
    }

    /// Re-renders `element`'s content into its root and queues `parseEnd`.
    fn render(&self, document: &mut LynxDocument, element: NodeId, state: &mut State) {
        let Some(root) = state.root.or_else(|| markdown_root(document, element)) else {
            return;
        };
        state.root = Some(root);
        let content = attribute(document, element, CONTENT_ATTRIBUTE)
            .unwrap_or_default()
            .to_owned();
        let parsed = if content.is_empty() {
            parse::Document::default()
        } else {
            parse::parse(&content, state.range)
        };
        let kept = state
            .document
            .blocks
            .iter()
            .zip(&parsed.blocks)
            .take_while(|(old, new)| old == new)
            .count()
            .min(state.elements.len());
        for stale in state.elements.drain(kept..) {
            build::forget(document, &mut state.newlines, stale);
            if document.get(stale).is_some() {
                document.drop_subtree(stale);
            }
        }
        let mut builder = Builder {
            document,
            newlines: &mut state.newlines,
        };
        for block in &parsed.blocks[kept..] {
            let built = builder.block(root, block, 0, false);
            state.elements.push(built);
        }
        state.document = parsed;
        if !content.is_empty() {
            self.events.queue_detailed(
                element,
                "parseEnd",
                true,
                ComponentDetail::Id {
                    id: content_id(document, element),
                },
            );
        }
    }

    /// A `tap` in the bubble pass at a node of `element`'s shadow tree:
    /// `link` or `imageTap`, or nothing. See the module documentation.
    fn tap(&self, document: &LynxDocument, element: NodeId, target: NodeId, x: f32, y: f32) {
        let Some(root) = markdown_root(document, element) else {
            return;
        };
        if let Some(anchor) = closest(document, target, root, "a") {
            self.queue_link(document, element, anchor);
            return;
        }
        if let Some(image) = closest(document, target, root, IMAGE_TAG) {
            let url = attribute(document, image, "src")
                .unwrap_or_default()
                .to_owned();
            self.events.queue_detailed(
                element,
                "imageTap",
                true,
                ComponentDetail::Url {
                    url,
                    content_id: content_id(document, element),
                },
            );
            return;
        }
        if let Some(anchor) = anchor_at(document, target, dom::Point2D::new(x, y)) {
            self.queue_link(document, element, anchor);
        }
    }

    fn queue_link(&self, document: &LynxDocument, element: NodeId, anchor: NodeId) {
        let url = attribute(document, anchor, "href")
            .unwrap_or_default()
            .to_owned();
        self.events.queue_detailed(
            element,
            "link",
            true,
            ComponentDetail::UrlContent {
                url,
                content: text_content(document, anchor),
                content_id: content_id(document, element),
            },
        );
    }
}

impl CustomElement<()> for Markdown {
    fn observed_attributes(&self) -> Vec<String> {
        [
            CONTENT_ATTRIBUTE,
            MARKDOWN_STYLE_ATTRIBUTE,
            CONTENT_ID_ATTRIBUTE,
            CONTENT_RANGE_ATTRIBUTE,
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn observed_events(&self) -> &'static [&'static str] {
        &["tap"]
    }

    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        // Freed hosts' entries go here; see "State" in the module docs.
        self.states
            .borrow_mut()
            .retain(|host, _| document.get(*host).is_some());
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        document.add_shadow_stylesheet(shadow, SHADOW_RULES);
        document.add_shadow_stylesheet(shadow, "");
        let root = document.create_element("div", ());
        document.set_id_attribute(root, Some(ROOT_ID));
        document.set_attribute(root, "part", "root");
        document.set_classes(root, ROOT_CLASS);
        document.append_child(shadow, root);
        self.put_state(
            element,
            State {
                root: Some(root),
                ..State::default()
            },
        );
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        let mut state = self.take_state(element);
        match name {
            CONTENT_ATTRIBUTE => self.render(document, element, &mut state),
            CONTENT_RANGE_ATTRIBUTE => {
                match parse::parse_content_range(new) {
                    parse::ContentRangeUpdate::Clear => state.range = None,
                    parse::ContentRangeUpdate::Set(start, end) => state.range = Some((start, end)),
                    parse::ContentRangeUpdate::Keep => {}
                }
                self.render(document, element, &mut state);
            }
            MARKDOWN_STYLE_ATTRIBUTE => {
                let css = style::parse_markdown_style(new.unwrap_or_default()).css;
                if css != state.css
                    && let Some(shadow) = document.shadow_root(element)
                {
                    document.replace_shadow_stylesheet(shadow, MARKDOWN_STYLE_SHEET, &css);
                    state.css = css;
                }
            }
            // Read when an event or `getParseResult` reports it.
            _ => {}
        }
        self.put_state(element, state);
    }

    fn handle_event(&self, document: &mut LynxDocument, element: NodeId, event: &mut ElementEvent) {
        let ElementEventKind::Tap { x, y } = event.kind() else {
            return;
        };
        let target = event.target();
        if event.phase() == EventPhase::Bubbling
            && document.shadow_root(element).is_some()
            && document.shadow_root_of(target) == document.shadow_root(element)
        {
            self.tap(document, element, target, x, y);
        }
    }

    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let states = self.states.borrow();
        let empty = FxHashMap::default();
        let newlines = states.get(&element).map_or(&empty, |state| &state.newlines);
        let root = markdown_root(document, element);
        let answer = match (call.name, root) {
            ("getContent", _) => methods::get_content(document, element, call.params).map(Some),
            ("getImages", Some(root)) => Ok(Some(methods::get_images(document, root))),
            ("getParseResult", Some(root)) => {
                methods::get_parse_result(document, element, root, newlines, call.params).map(Some)
            }
            ("getTextBoundingRect", Some(root)) => {
                methods::get_text_bounding_rect(document, element, root, newlines, call.params)
            }
            ("getCharIndexByPoint", Some(root)) => {
                methods::get_char_index_by_point(document, element, root, newlines, call.params)
            }
            #[cfg(test)]
            (RENDERED_TEXT_METHOD, Some(root)) => {
                let text = rendered::text_units(document, root, newlines);
                let mut record = String::new();
                crate::main::record::write_record_field(
                    &mut record,
                    &String::from_utf16_lossy(&text),
                );
                Ok(Some(record))
            }
            _ => return MethodOutcome::NotFound,
        };
        match answer {
            Ok(Some(fields)) => MethodOutcome::Data(fields),
            Ok(None) => MethodOutcome::Failed(MethodError::Operation),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        }
    }
}

/// `element`'s shadow `#markdown-root`, the one element of its shadow tree.
fn markdown_root(document: &LynxDocument, element: NodeId) -> Option<NodeId> {
    let shadow = document.shadow_root(element)?;
    document
        .get(shadow)?
        .children()
        .find(|child| child.is_element())
        .map(dom::Node::id)
}

fn attribute<'a>(document: &'a LynxDocument, element: NodeId, name: &str) -> Option<&'a str> {
    document.get(element).and_then(|node| node.attribute(name))
}

/// The `content-id` the events carry: the attribute, or no key at all.
fn content_id(document: &LynxDocument, element: NodeId) -> Option<String> {
    attribute(document, element, CONTENT_ID_ATTRIBUTE).map(str::to_owned)
}

/// DOM's `closest(tag)` from `node`, stopping below `root`.
fn closest(document: &LynxDocument, node: NodeId, root: NodeId, tag: &str) -> Option<NodeId> {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == root {
            return None;
        }
        let node = document.get(id)?;
        if node.tag_name() == Some(tag) {
            return Some(id);
        }
        current = node.parent_id();
    }
    None
}

/// The innermost `a` holding the character a tap at `point` (viewport CSS
/// px) hits in the paragraph `target` belongs to.
fn anchor_at(document: &LynxDocument, target: NodeId, point: dom::Point2D<f32>) -> Option<NodeId> {
    let paragraph = document.paragraph_of(target)?;
    let origin = methods::content_origin(document, paragraph)?;
    let unit = document.text_index_at_point(paragraph, point - origin)?;
    let mut best: Option<(NodeId, usize)> = None;
    let mut stack = vec![paragraph];
    while let Some(id) = stack.pop() {
        let Some(node) = document.get(id) else {
            continue;
        };
        if node.tag_name() == Some("a")
            && let Some(range) = document.inline_source_range(id)
            && range.contains(&unit)
            && best.is_none_or(|(_, len)| range.len() < len)
        {
            best = Some((id, range.len()));
        }
        stack.extend(node.child_ids().iter().copied());
    }
    best.map(|(anchor, _)| anchor)
}

/// DOM's `textContent` of an element: its text nodes' data, in tree order.
fn text_content(document: &LynxDocument, element: NodeId) -> String {
    let mut text = String::new();
    let mut stack = vec![element];
    while let Some(id) = stack.pop() {
        let Some(node) = document.get(id) else {
            continue;
        };
        if node.is_text_node() {
            text.push_str(node.text().unwrap_or_default());
        }
        stack.extend(node.child_ids().iter().rev().copied());
    }
    text
}
