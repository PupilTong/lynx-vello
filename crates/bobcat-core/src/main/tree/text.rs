//! The `text` tag's attribute-to-CSS limits and defaults: what color a run wears,
//! and what may generate a box inside one; and the `text` component, whose
//! one UI method is `getTextBoundingRect` ([`text_bounding_rect`]).
//!
//! The other half of Lynx text — how a run reaches the engine and where it
//! lays out — is [`super::raw_text`], which owns the `raw-text` generated-content rule and
//! the rules that dissolve a carrier into the `text` it is written inside.
//!
//! # UI methods
//!
//! The Lynx docs give `text` three (`lynxjs.org/api/elements/built-in/text`,
//! the typings in `lynx/js_libraries/types/types/common/element/text.d.ts:220-258`,
//! `@since 2.18`); web-core's `XText` has none of them.
//!
//! - `getTextBoundingRect({start, end})` is built ([`text_bounding_rect`]), over
//!   [`terminus::Document::text_range_boxes`], with iOS's per-line answer
//!   (`lynx/platform/darwin/ios/lynx/ui/text/LynxUIText.m:790-807`, `LynxTextView.m:205-235`).
//! - `getSelectedText()` and `setTextSelection({startX, startY, endX, endY, ...})` are not built
//!   and answer code 3: the engine has no text selection model, so there is nothing selected to
//!   report and nothing a selection could be set on.
//!
//! # Events
//!
//! `text` and `inline-text` fire Lynx's `selectionchange` (`bindselectionchange`)
//! with the detail `{start, end, direction}`, web-core's
//! (`lynx-stack/packages/web-platform/web-elements/src/elements/XText/XTextSelectionEvents.ts`,
//! on both `XText` and `InlineText`; `inline-truncation` has none), which
//! native Android (`AndroidText.java:482-490`) and iOS
//! (`LynxTextView.m:319-328`) share:
//!
//! - `start` and `end` are the smaller and the larger of the document selection's two boundary
//!   points, as offsets into the element's whole text content — every text node of its subtree, in
//!   tree order, in UTF-16 units — and `direction` is `"forward"` when the anchor is at or before
//!   the focus, `"backward"` otherwise.
//! - A collapsed selection, one with either end outside the element, and no selection at all are
//!   the empty record `{start: -1, end: -1, direction: "forward"}`.
//! - It is derived from the W3C `selectionchange`: the runtime fires that one at the document node
//!   and then calls [`report_selection_changes`], which queues this one at each `text` and
//!   `inline-text` whose record changed, for the same entry to deliver after the W3C one. web-core
//!   listens for the document's `selectionchange` in the same way.
//! - Only while something listens for `selectionchange` on the element
//!   ([`terminus::Document::would_reach_listener`]), as web-core attaches its document listener
//!   only while the event is bound.
//! - A record equal to the last one sent to the element is not sent again, and the empty record is
//!   sent only to an element whose last record was not empty: web-core's signature check.
//! - Not bubbling, composed ([`ComponentEvents::queue_detailed`]).
//!
//! The document's selection exists only inside an editing host
//! ([`terminus::Document::selection`]): a `text` with `contenteditable`, or an
//! element inside one. Its boundary points are offsets into the host's
//! value, its direct text children, and [`text_selection_in`] maps each into
//! an element's text content the way web-core's
//! `range.selectNodeContents(root); range.setEnd(node, offset);
//! range.toString().length` does, for the one place a boundary can be here.
//! So a `text` around a host that is a nested `inline-text` reports offsets
//! that count the text before the host. There is one difference from
//! web-core: an `inline-text` *inside* the host never holds a boundary,
//! because the host's value is its direct text children alone, so it reports
//! the empty record; a browser's selection can enter that `inline-text`, and
//! web-core fires offsets at it. The selection of read-only text — a
//! `text-selection` `text` selected by a pointer — is not built.
//!
//! The component holds no state and has no lifecycle callback; the last
//! record sent to each element is the runtime's, in a table it passes to
//! [`report_selection_changes`] ([`SelectionReports`]). Its definition, and
//! the two behaviourless ones beside it for `inline-text` and
//! `inline-truncation`, carry one fact besides the method: the three tags are
//! Lynx text elements ([`terminus::CustomElement::is_lynx_text`]), which is what
//! lays each out as a Lynx paragraph or a run of one. `:defined` already
//! matches every element in `terminus`, and the `raw-text` rules are selector rules
//! on the tag name, which a definition does not touch.

/// `text`'s own defaults, from `web-elements`' `x-text.css`.
///
/// `color: initial` is why Lynx text does not wear an ancestor's color. Native
/// Lynx runs with inheritance off unless the page turns it on; web-core runs
/// on the browser's always-on inheritance and buys the same result by
/// resetting `color` at every text root, which is the parity this engine
/// committed to (`docs/tracking/deviations.md`). A nested run opts back in
/// with `inherit`, and that carries a Lynx text gradient exactly as it carries
/// a solid color — the fork's `color` holds either, and the glyph painter
/// reads whichever it finds. The reset therefore also stops an ancestor's
/// gradient at the text root, where web-core stops it with the same
/// declaration. A page whose config says `enableCSSInheritance: true` gets
/// [`CSS_INHERITANCE_RULES`] after these rules, which overrides the reset for
/// a `text` inside a `view`.
///
/// A `text` renders text: anything else written directly inside one generates
/// no box, and the tags that are content opt back in. `wrapper` dissolves,
/// `view` and `image` stay atomic boxes in the flattened paragraph.
/// `raw-text` opts back in from
/// [`super::raw_text::UA_RULES`], where the rest of a carrier's policy already
/// lives.
/// `attr(... number)` accepts numeric attributes without evaluating CSS
/// expressions. Non-inherited integer registrations reject fractional counts;
/// invalid or absent limits are unlimited.
/// `--lynx-tail-color-convert` carries the `tail-color-convert` boolean the
/// same way: zero, its initial value, leaves the truncation marker in the
/// colour of the run the cut landed in, which is native Lynx's default.
///
/// `text-overflow` is an attribute as well as a property on native Lynx —
/// `TextElement::ProcessAttributeForNormalLayoutMode` caches the attribute onto
/// `kPropertyIDTextOverflow` (`core/renderer/dom/fiber/text_element.cc:176-182`)
/// — and web-core observes it nowhere, so a 2026-09-16 ruling follows native
/// here (`docs/tracking/deviations.md`). It is a selector rule rather than a
/// presentational hint because it needs no parsing of its own: native's enum
/// reader accepts the two literals and nothing else, so two UA rules keyed on
/// those literals say the whole thing. Plain declarations, so author CSS still
/// outranks the attribute the way it outranks every other UA rule.
///
/// A `text`'s paragraph keeps its text verbatim, as every native Lynx backend
/// does: no CSS
/// white-space processing (`core/renderer/dom/fiber/raw_text_element.cc:16-46`
/// stores the text as written, and Android `BaseTextShadowNode.java:494-498`,
/// iOS `LynxBaseTextShadowNode.m:240-249` and textra
/// `text_layout_textra.cc:569-575` hand it to the platform engine unchanged).
/// Spaces, tabs and newlines all stay; tabs stop every 20px as Android's
/// `Layout` does; a newline that ends the text adds an empty last line, as
/// Android's `StaticLayout` does; only `white-space: nowrap` stops the
/// wrapping, and then nothing after the first newline shows — native parses
/// no other `white-space` keyword, so `pre` and the rest wrap. Every Lynx
/// paragraph does this; `<markdown>`'s shadow paragraphs are standard inline
/// formatting contexts, not Lynx paragraphs, and keep css-text-3. This is a
/// ruled deviation from web-core, whose `x-text` hands its text to the
/// browser's CSS white-space processing.
///
/// An `inline-truncation` is content wherever it is written *directly* inside
/// a `text`, and generates no box anywhere else — the same scope web-core's
/// `:scope > inline-truncation` query honours. It carries
/// `--lynx-inline-truncation` because `crates/terminus` names no Lynx tag: which
/// subtree a paragraph takes its custom truncation content from is a
/// computed-style fact there, exactly as the paragraph limits are.
///
/// # Why this sheet gives a text no `display`
///
/// A text tag — `text`, `inline-text`, `inline-truncation` — lays out as a
/// Lynx paragraph, or a run of one, because of what the element is: [`define`]
/// marks the three definitions as Lynx text elements
/// ([`terminus::CustomElement::is_lynx_text`]), as an HTML `img` is replaced content
/// whatever its `display` says. Its computed `display` keeps the author's value.
/// `none` hides it, as it does in native Lynx and in web-core; every other
/// value but `contents` leaves it a paragraph. That is native Lynx's shape: a
/// text's inline-ness is structural (`TextElement::ConvertToInlineElement`,
/// `core/renderer/dom/fiber/text_element.cc:196-205`), so native ignores
/// `display: flex` on a text. `contents` on a text has no Lynx behaviour to
/// follow: it is undefined behaviour (user ruling, 2026-10-17), which acts as
/// computed only so that it does not crash. web-core lets the author's value
/// take over the `x-text` host (`x-text { display: flex }` is author-origin,
/// `x-text.css:7-13`) while its shadow tree keeps the text, so the host's box
/// differs there (`docs/tracking/deviations.md`).
///
/// Until 2026-10-15 the sheet declared `display: -lynx-text !important`,
/// which also outranked the author's `display: none`, and until 2026-10-17 a
/// plain `display: -lynx-text`, which the stylo fork's style adjuster
/// restored on the three tags under any author value but `none`. The value
/// was removed on 2026-10-17. Without the `!important`, a UA rule that hides
/// an element's children hides a `text` among them too, as a browser would:
/// `list > *:not(list-item):not(wrapper)`, `x-swiper > :not(x-swiper-item,
/// wrapper)`, `image > *` and `svg > *`. The rule for anything written
/// directly inside a `text` therefore names the three text tags, and keeps
/// its old specificity with `:where`, so the `wrapper`, `view` and `image`
/// rules after it still outrank it. `inline-truncation` shows only directly
/// inside a `text`, so the rule hiding it names every other parent.
///
/// # Why an inline `image` has no padding
///
/// `padding: 0 !important` on an `image` that is inline content of a paragraph
/// is the one `!important` this sheet spends, and it buys
/// web-core's geometry rather than a default. There, the authored host element
/// has no box
/// at all: `x-text > x-image` — and the `inline-text >`, `inline-truncation >`
/// and `lynx-wrapper` variants of it — is `display: contents !important`
/// (`x-text.css:69-82`), so the box on the line is the shadow `::part(img)`,
/// which is built by inheriting a fixed list: `width`, `height`, `border`,
/// `border-radius`, `background-color`, `vertical-align`, `object-fit`,
/// `flex`, `align-self` and `margin` (`x-text.css:120-135`). `padding` is not
/// on that list, and the one `padding: inherit` in web-elements belongs to
/// `x-image[auto-size]::part(img)`, an attribute this engine does not
/// implement. An author's `padding` on an inline image therefore reaches no
/// box in the reference and must reach none here.
///
/// Native is split. `TextLayoutTextra::HandleInlineImageProps`
/// (`core/renderer/ui_wrapper/layout/textra/text_layout_textra.cc:448-537`)
/// fills the placeholder's `ImageProps` from the specified width and height,
/// the four margins, the border radius and `vertical-align`, and never reads
/// `padding`; Android's `InlineImageSpan` and iOS's default shadow-node path
/// read the same values and no padding either. Harmony, and iOS's
/// layout-in-element path, instead measure the image as a starlight leaf, whose
/// `ClampExactWidth` floors the border box at padding plus border
/// (`core/renderer/starlight/layout/layout_object.cc:515-519`). The rule here
/// follows web-core, which the larger part of native matches.
///
/// Here the authored `image` *is* the box, so nothing removes its padding for
/// free: under the Lynx `box-sizing: border-box` default a
/// `width: 22px; padding-left: 50px` image would floor its border box at 50 and
/// advance the line by 50 where web-core advances by 22. A normal UA
/// declaration cannot say so — it loses to the author's own `padding-left` —
/// and web-core's erasure of the host box is itself `!important`, so no author
/// CSS can undo it. `margin` is a different case and is *not* touched: the shadow
/// part inherits it, so it reaches the line in both engines. Nor is a `view`,
/// which web-core leaves as a real `inline-flex` box with its padding intact.
pub(super) const UA_RULES: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-tail-color-convert { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
text { color: initial; }
text {
  --lynx-text-maxline: attr(text-maxline number, -1);
  --lynx-text-maxlength: attr(text-maxlength number, -1);
}
text[tail-color-convert="true"] { --lynx-tail-color-convert: 1; }
text[text] { content: attr(text); }
text[text-overflow="ellipsis"] { text-overflow: ellipsis; }
text[text-overflow="clip"] { text-overflow: clip; }
inline-image, :not(text) > inline-truncation { display: none; }
text > :where(:not(text, inline-text, inline-truncation)) { display: none; }
text > wrapper { display: contents; }
text > view, text > image { display: flex; }
text > inline-truncation { --lynx-inline-truncation: 1; }
text > text, text > wrapper > text { color: inherit; }
text > image, text > wrapper > image, inline-text > image, inline-text > wrapper > image, text > inline-truncation > image, text > inline-truncation > wrapper > image { padding: 0 !important; }
"#;

/// What `enableCSSInheritance: true` adds to the sheet, after [`UA_RULES`];
/// [`super::ua_sheet`] writes it when the page config has the switch on and
/// leaves it out otherwise.
///
/// It is web-core's rule for the same switch (lynx-stack #3907, `8d0b358bf`,
/// `web-core/css/in_shadow.css`):
/// `[lynx-enable-css-inheritance="true"] :where(x-view > x-text, x-view >
/// lynx-wrapper, x-view > lynx-wrapper > x-text)` over fourteen `inherit`
/// declarations. The scope is that rule's and no wider: a `text` directly
/// inside a `view`, a `wrapper` directly inside a `view`, and a `text`
/// directly inside such a wrapper. A `text` under `page`, `scroll-view`,
/// `list-item` or any other container keeps the reset, as it does there.
/// Native Lynx's switch of the same name does something else — a list of
/// properties that inherit through every element, where nothing inherits
/// with it off — and this rule does not build that
/// (`docs/tracking/deviations.md`).
///
/// # Which declarations are written
///
/// Two of the fourteen change a computed value in this engine:
///
/// - `color: inherit` replaces the `color: initial` [`UA_RULES`] gives every `text`. The fork's
///   `color` holds a Lynx text gradient as its own value, so this one declaration inherits a view's
///   gradient as well as its solid color.
/// - `text-decoration: inherit`. The shorthand's longhands are not inherited properties, so under
///   an underlined view a `text` computes `none` without it. `view > wrapper` is in the selector
///   for this declaration: the wrapper has to hold the view's value for the `text` inside it to
///   inherit it.
///
/// The other twelve are left out because none of them can change one:
///
/// - `--lynx-text-bg-color`, `background-clip` and `-webkit-background-clip` belong to web-core's
///   lowering of a gradient `color` for a browser — transparent glyphs over a background image
///   clipped to them, which `x-text.css` resets at the text root. This engine lowers no gradient
///   that way, so there is no reset here for them to override.
/// - `direction`, `font-family`, `font-size`, `font-style`, `font-weight`, `letter-spacing`,
///   `line-height`, `text-align` and `text-shadow` are inherited properties, and no rule of this
///   sheet sets one of them on a `text` or a `wrapper`. Each already reaches the `text` from the
///   view with the switch off, as it does under web-core, where `x-text.css` resets none of them.
///
/// # Precedence
///
/// web-core's selector has the specificity of one attribute selector, since
/// `:where()` adds none: above every web-elements default for `x-text`, and
/// above no authored rule, each of whose selectors web-core extends with
/// `:not([l-e-name])`. Here the rule is user-agent origin, so every authored
/// declaration and every inline style outranks it whatever its selector, and
/// its tag selectors outrank `text`'s `color: initial` on specificity. The
/// selector list is not wrapped in `:where()` for that reason: at no
/// specificity it would lose to the reset it exists to replace.
pub(super) const CSS_INHERITANCE_RULES: &str = "\
view > text, view > wrapper, view > wrapper > text { color: inherit; text-decoration: inherit; }
";

use std::ops::Range;

use rustc_hash::FxHashMap;
use serde_json::{Map, Value};
use smallvec::SmallVec;
use terminus::editing::DocumentSelection;
use terminus::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId};

use super::scroll_container::object;
use super::{ComponentDetail, ComponentEvents, InvalidParams, LynxDocument, SELECTION_CHANGE};
use crate::main::record::write_record_field;

/// The paragraph tag the component is defined on. A nested `text` is a run
/// of its ancestor's paragraph and establishes none, so its method answers
/// code 8.
pub(super) const TEXT_TAG: &str = "text";

/// Whether `node` is a live `text` element: the one tag a `layout` listener
/// turns the paragraph's layout report on for
/// ([`crate::main::layout_change`]). A nested `text` is one too, and reports
/// nothing, since it establishes no paragraph.
pub(crate) fn is_text(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(terminus::Node::tag_name)
        .is_some_and(|tag| tag == TEXT_TAG)
}

/// Installs the `text` component, and `inline-text` and `inline-truncation`
/// beside it as Lynx text elements with no component behaviour. Must run
/// before any element could carry the tags, which is
/// [`Document::define`](terminus::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(TEXT_TAG, Box::new(Text));
    document.define(INLINE_TEXT_TAG, Box::new(TextRun));
    document.define(INLINE_TRUNCATION_TAG, Box::new(TextRun));
}

/// The nested run tag.
const INLINE_TEXT_TAG: &str = "inline-text";

/// The truncation marker tag, whose content replaces a cut paragraph's tail.
const INLINE_TRUNCATION_TAG: &str = "inline-truncation";

/// `inline-text` and `inline-truncation`: Lynx text elements, defined for
/// that kind alone.
struct TextRun;

impl CustomElement<()> for TextRun {
    fn is_lynx_text(&self) -> bool {
        true
    }
}

/// The `text` component: `getTextBoundingRect` ([`text_bounding_rect`]).
/// See the module documentation for the two methods it does not build.
struct Text;

impl CustomElement<()> for Text {
    fn is_lynx_text(&self) -> bool {
        true
    }

    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        if call.name != "getTextBoundingRect" {
            return MethodOutcome::NotFound;
        }
        match text_bounding_rect(document, element, call.params) {
            Ok(Some(fields)) => MethodOutcome::Data(fields),
            Ok(None) => MethodOutcome::Failed(MethodError::Operation),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        }
    }
}

/// Per text element, the last non-empty `selectionchange` record sent to it
/// — its range and whether it was forward — which [`report_selection_changes`]
/// compares the next record against, as web-core's `#lastSelectionSignature`.
///
/// A side table rather than a field of the element, because no element
/// component holds it: `text` is stateless, and `inline-text` has no
/// behaviour at all. It is the runtime's, one per document (`DocumentSlot`).
/// An entry exists only for an element that was sent a non-empty record,
/// and leaves when the element is sent the empty one, when nothing listens
/// for the event on it any more, or when it is freed, so the table holds
/// the one or two text elements the one selection lies in.
#[derive(Default)]
pub(crate) struct SelectionReports(FxHashMap<NodeId, (Range<u32>, bool)>);

/// Whether `node` fires Lynx's `selectionchange`: a `text` or an
/// `inline-text` (module documentation).
fn reports_selection(node: &terminus::Node<()>) -> bool {
    matches!(node.tag_name(), Some(TEXT_TAG | INLINE_TEXT_TAG))
}

/// Called by the runtime after it fired the W3C `selectionchange` at the
/// document node: queues the Lynx `selectionchange` on `events` at every
/// `text` and `inline-text` whose record changed (module documentation).
///
/// The elements asked are the ones the record can have changed for: the
/// `text` and `inline-text` ancestors of the editing host, the host
/// included, through `parent_id` — a UA shadow tree's host is not crossed,
/// so a text field's editor reports to no text outside it — and every
/// element `reports` holds, whose selection may have left it. An element
/// nothing listens on is forgotten, as web-core forgets its signature when
/// the event is unbound, so a listener added later starts from the empty
/// record.
pub(crate) fn report_selection_changes(
    document: &LynxDocument,
    reports: &mut SelectionReports,
    events: &ComponentEvents,
) {
    let selection = document.selection();
    let mut candidates: SmallVec<[NodeId; 4]> = SmallVec::new();
    let mut current = selection.map(|selection| selection.host);
    while let Some(id) = current {
        let Some(node) = document.get(id) else {
            break;
        };
        if reports_selection(node) {
            candidates.push(id);
        }
        current = node.parent_id();
    }
    for &element in reports.0.keys() {
        if !candidates.contains(&element) {
            candidates.push(element);
        }
    }
    for element in candidates {
        if document.get(element).is_none()
            || !document.would_reach_listener(element, SELECTION_CHANGE, false, true)
        {
            reports.0.remove(&element);
            continue;
        }
        let record =
            selection.and_then(|selection| text_selection_in(document, element, selection));
        match (reports.0.get(&element), record) {
            (None, None) => {}
            (Some(last), Some(record)) if *last == record => {}
            (_, Some((range, forward))) => {
                reports.0.insert(element, (range.clone(), forward));
                events.queue_detailed(
                    element,
                    SELECTION_CHANGE,
                    ComponentDetail::TextSelection {
                        range: Some(range),
                        forward,
                    },
                );
            }
            (Some(_), None) => {
                reports.0.remove(&element);
                events.queue_detailed(
                    element,
                    SELECTION_CHANGE,
                    ComponentDetail::TextSelection {
                        range: None,
                        forward: true,
                    },
                );
            }
        }
    }
}

/// The document's selection as `element`'s `selectionchange` record: the
/// range between its two boundary points as offsets into `element`'s text
/// content, and whether it runs forward (anchor at or before focus); `None`,
/// the empty record, when the selection is collapsed — web-core's
/// `anchor !== focus` — or its host is neither `element` nor inside it, or
/// a boundary cannot be placed in `element`'s text ([`text_offset`]).
fn text_selection_in(
    document: &LynxDocument,
    element: NodeId,
    selection: DocumentSelection,
) -> Option<(Range<u32>, bool)> {
    let mut current = Some(selection.host);
    while current != Some(element) {
        current = document.get(current?).and_then(terminus::Node::parent_id);
    }
    if selection.anchor == selection.focus {
        return None;
    }
    let anchor = text_offset(document, element, selection.host, selection.anchor)?;
    let focus = text_offset(document, element, selection.host, selection.focus)?;
    Some((
        anchor.min(focus)..anchor.max(focus),
        selection.anchor <= selection.focus,
    ))
}

/// One boundary point — `unit` UTF-16 units into `host`'s value, which is
/// its direct text children — as an offset into `root`'s text content:
/// web-core's `range.selectNodeContents(root); range.setEnd(node, offset);
/// range.toString().length`.
///
/// Walks `root`'s subtree in tree order, counting the UTF-16 length of every
/// text node. A direct text child of `host` that holds the unit — it lies at
/// or before the node's end — answers the count before it plus the unit;
/// one that does not takes its length off the unit. A text node that is not
/// a direct child of `host` only adds to the count. `None` when the walk
/// ends with the unit not placed, where web-core's `setEnd` throws and its
/// offset is `-1`.
fn text_offset(document: &LynxDocument, root: NodeId, host: NodeId, unit: u32) -> Option<u32> {
    let mut remaining = unit;
    let mut count: u32 = 0;
    let mut stack: SmallVec<[NodeId; 16]> = SmallVec::new();
    stack.push(root);
    while let Some(id) = stack.pop() {
        let Some(node) = document.get(id) else {
            continue;
        };
        if let Some(text) = node.text() {
            let length =
                u32::try_from(text.chars().map(char::len_utf16).sum::<usize>()).unwrap_or(u32::MAX);
            if node.parent_id() == Some(host) {
                if remaining <= length {
                    return Some(count.saturating_add(remaining));
                }
                remaining -= length;
            }
            count = count.saturating_add(length);
            continue;
        }
        // Children in reverse, so the first is taken next: tree order.
        stack.extend(node.child_ids().iter().rev().copied());
    }
    None
}

/// `getTextBoundingRect({start, end})` on `text`, with `params` as the JSON
/// text the realm serialized: the boxes the source range `[start, end)` of
/// the paragraph occupies, one per line, in viewport CSS px.
///
/// - `params` must be a JSON object, and `start` and `end` JSON numbers, else code 4. Each is
///   truncated toward zero, as iOS's `integerValue` and Android's `getInt` truncate
///   (`LynxUIText.m:790-807`, `UIText.java:343-364`), and must then be non-negative, else code 4
///   (iOS and Android). A missing one is code 4, iOS's answer; Android's `getInt` throws and its
///   catch answers 1. `start > end` is code 4 too, which is Android's answer; iOS answers 1. A
///   string is code 4, where iOS's `intValue` would read its digits. Offsets are UTF-16 units of
///   the paragraph's source text, an inline image or view counting one, as both natives count them
///   (an `NSString`, a Java `String`).
/// - The boxes are [`terminus::Document::text_range_boxes`]: one per line the range shows content
///   on, from the first selected cluster's inline start to the last one's inline end, with the
///   line's top and height. That is iOS's reading (`enumerateEnclosingRectsForGlyphRange`,
///   `LynxTextView.m:205-235`); Android widens every line but the first and the last to the full
///   layout width (`AndroidText.java:438-475`). `start == end` is one zero-width box at the content
///   box's origin, as tall as the line holding `start`, which is both natives' answer.
/// - Each box moves from the paragraph's own space into the space of `boundingClientRect`: plus the
///   element's [`terminus::Document::bounding_client_rect`] origin and its border and padding, the
///   content box's offset in the border box, as iOS's `getMapFromRect:lineBox:` adds them
///   (`LynxUIText.m:890-903`). `relativeTo` is not read, as `boundingClientRect` reads none of its
///   params here.
/// - `Ok(None)`, code 8, when nothing answers: the element has no box, establishes no paragraph (a
///   nested `text`), `end` is past the source length, or the range shows no content (text past a
///   truncation cut, the whitespace a soft wrap consumed). The natives answer 1, `UNKNOWN`, "Can
///   not find text bounding rect"; the engine has no code 1, and 8, `OPERATION_ERROR`, is its code
///   for a method that ran and found nothing to act on.
///
/// The data fields are the number of boxes and then `left`, `top`, `width`,
/// `height` per box; `element-papi.ts` builds `{boundingRect, boxes}` from
/// them. It reads the last completed layout and never flushes, as every UI
/// method does here.
fn text_bounding_rect(
    document: &LynxDocument,
    text: NodeId,
    params: &str,
) -> Result<Option<String>, InvalidParams> {
    let params = object(params)?;
    let start = range_offset(&params, "start")?;
    let end = range_offset(&params, "end")?;
    if start > end {
        return Err(InvalidParams);
    }
    let Some(element_box) = document.bounding_client_rect(text) else {
        return Ok(None);
    };
    let Some(layout) = document.rounded_layout(text) else {
        return Ok(None);
    };
    let Some(boxes) = document
        .text_range_boxes(text, start, end)
        .filter(|boxes| !boxes.is_empty())
    else {
        return Ok(None);
    };
    let origin = terminus::Vector2D::new(
        element_box.origin.x + layout.border.left + layout.padding.left,
        element_box.origin.y + layout.border.top + layout.padding.top,
    );
    let mut record = String::new();
    write_record_field(&mut record, &boxes.len().to_string());
    for rect in boxes {
        let rect = rect.translate(origin);
        for value in [
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ] {
            write_record_field(&mut record, &value.to_string());
        }
    }
    Ok(Some(record))
}

/// `start` or `end` of `getTextBoundingRect`'s params: a JSON number
/// truncated toward zero, non-negative after truncation, else code 4. A
/// count past `u32::MAX` saturates, which is past every paragraph's length
/// and so answers code 8 for `end`.
fn range_offset(params: &Map<String, Value>, key: &str) -> Result<u32, InvalidParams> {
    let offset = params
        .get(key)
        .and_then(Value::as_f64)
        .ok_or(InvalidParams)?
        .trunc();
    if offset < 0.0 {
        return Err(InvalidParams);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a non-negative integer; one past u32::MAX saturates and is past every paragraph"
    )]
    Ok(offset as u32)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem and explicit line heights have exact metrics.

    use terminus::stylo::color::AbsoluteColor;
    use terminus::stylo::values::computed::{ColorPropertyValue, Display};

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document, element_under, style_of};

    const MAX_LINES: &str = "text-maxline";
    const MAX_CHARS: &str = "text-maxlength";
    const TAIL_COLOR_CONVERT: &str = "tail-color-convert";
    const TEXT_OVERFLOW: &str = "text-overflow";

    /// Writes or removes an attribute the way the runtime does — the DOM
    /// mutation and nothing else. Stylo tracks the attribute dependency.
    fn set_limit(
        document: &mut LynxDocument,
        element: terminus::NodeId,
        name: &str,
        value: Option<&str>,
    ) {
        if let Some(value) = value {
            document.set_attribute(element, name, value);
        } else {
            document.remove_attribute(element, name);
        }
    }

    fn append_run(document: &mut LynxDocument, parent: terminus::NodeId, content: &str) {
        let raw = element_under(document, parent, "raw-text", "");
        document.set_attribute(raw, "text", content);
    }

    fn paragraph(content: &str) -> (LynxDocument, terminus::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "width:100px;font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, content);
        (document, text)
    }

    fn assert_height(document: &mut LynxDocument, text: terminus::NodeId, height: f32) {
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").height,
            height
        );
        assert_eq!(
            document.rounded_layout(text).expect("text box").size.height,
            height
        );
    }

    #[test]
    fn text_maxline_updates_and_removal_resize_text_and_following_content() {
        for content in ["abc def", "abc\ndef"] {
            let (mut document, text) = paragraph(content);
            let sibling = child(&mut document, "view", "width:10px;height:10px");
            assert_height(&mut document, text, 42.0);

            for (value, height) in [
                (Some("1"), 21.0),
                (Some("2"), 42.0),
                (Some("1"), 21.0),
                (None, 42.0),
            ] {
                set_limit(&mut document, text, MAX_LINES, value);
                assert_height(&mut document, text, height);
                assert_eq!(
                    document
                        .rounded_layout(sibling)
                        .expect("following box")
                        .location
                        .y,
                    height,
                    "content after the paragraph follows its new height"
                );
            }
        }
    }

    #[test]
    fn text_limits_require_integers_and_reset_invalid_values() {
        let (mut document, text) = paragraph("abc def");
        for (value, height) in [
            ("1", 21.0),
            ("0", 42.0),
            ("1.0", 42.0),
            ("-1", 42.0),
            ("1e0", 42.0),
            ("", 42.0),
            ("  +1px", 42.0),
            ("garbage", 42.0),
            ("1e+", 42.0),
            ("1.5", 42.0),
            ("calc(1)", 42.0),
            ("1 trailing", 42.0),
            ("\u{feff}1", 42.0),
            ("Infinity", 42.0),
            ("1", 21.0),
            ("4294967296", 42.0),
        ] {
            set_limit(&mut document, text, MAX_LINES, Some(value));
            assert_height(&mut document, text, height);
        }
        // Maxlength permits zero, but never truncates a fractional input.
        for (value, width) in [("1.9", 60.0), ("0", 0.0), ("-1", 60.0)] {
            set_limit(&mut document, text, MAX_CHARS, Some(value));
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    #[test]
    fn text_limits_belong_to_the_whole_paragraph_across_nested_runs() {
        let (mut document, text) = paragraph("abc ");
        let nested = element_under(&mut document, text, "text", "");
        append_run(&mut document, nested, "def");
        set_limit(&mut document, nested, MAX_CHARS, Some("0"));
        set_limit(&mut document, nested, MAX_LINES, Some("1"));
        assert_height(&mut document, text, 42.0);

        set_limit(&mut document, text, MAX_LINES, Some("1"));
        assert_height(&mut document, text, 21.0);
        set_limit(&mut document, text, MAX_CHARS, Some("2"));
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").width,
            40.0
        );

        set_limit(&mut document, text, MAX_LINES, None);
        assert_height(&mut document, text, 21.0);
        set_limit(&mut document, text, MAX_CHARS, None);
        assert_height(&mut document, text, 42.0);
    }

    /// Only text's UA rule reads these attributes; the registered properties
    /// also prevent a parent's limits from inheriting into a paragraph.
    #[test]
    fn registered_text_limits_do_not_inherit_from_the_parent() {
        let (mut document, text) = paragraph("abc def");
        let page = document.document_element().id();
        set_limit(&mut document, page, MAX_LINES, Some("1"));
        set_limit(&mut document, page, MAX_CHARS, Some("0"));
        assert_height(&mut document, text, 42.0);
        assert_eq!(document.text_block_size(text).unwrap().width, 60.0);
        assert_eq!(
            custom_value(&document, page, "lynx-text-maxline", "-1"),
            "-1",
            "`page` is not a `text`, so the attribute is not read",
        );

        document.set_inline_style_property(page, "--lynx-text-maxline", "1");
        document.set_inline_style_property(page, "--lynx-text-maxlength", "0");
        assert_height(&mut document, text, 42.0);
        assert_eq!(
            document.text_block_size(text).unwrap().width,
            60.0,
            "and a declared limit is non-inheriting, so it stops at the page",
        );
    }

    /// The UA rules read only the supported attributes on the text tag.
    #[test]
    fn text_attribute_rules_are_scoped_to_text() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        // `text-overflow` is a UA selector rule rather than a hint, and
        // `ellipsize-mode` is inert: neither may reach a declaration.
        for name in [TEXT_OVERFLOW, "ellipsize-mode", "text-maxline-x"] {
            set_limit(&mut document, text, name, Some("1"));
        }
        // And the same three names on a tag whose UA rule does not read them.
        let view = child(&mut document, "view", "");
        for name in [MAX_LINES, MAX_CHARS, TAIL_COLOR_CONVERT] {
            set_limit(&mut document, view, name, Some("1"));
        }
        document.layout();

        for (property, initial) in [
            ("lynx-text-maxline", "-1"),
            ("lynx-text-maxlength", "-1"),
            ("lynx-tail-color-convert", "0"),
        ] {
            assert_eq!(
                custom_value(&document, text, property, initial),
                initial,
                "an unrelated attribute changes no value: {property}",
            );
            assert_eq!(
                custom_value(&document, view, property, initial),
                initial,
                "a paragraph limit means nothing on a tag that is not a \
                 `text`: {property}",
            );
        }
    }

    #[test]
    fn inline_css_can_override_a_text_limit() {
        for (attribute, property, value, width, height) in [
            (MAX_LINES, "--lynx-text-maxline", "2", 60.0, 42.0),
            (MAX_LINES, "--lynx-text-maxline", "calc(2 - 1)", 60.0, 21.0),
            (MAX_LINES, "--lynx-text-maxline", "1.5", 60.0, 42.0),
            (MAX_LINES, "--lynx-text-maxline", "1px", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", "2", 40.0, 21.0),
            (
                MAX_CHARS,
                "--lynx-text-maxlength",
                "calc(1 + 1)",
                40.0,
                21.0,
            ),
            (MAX_CHARS, "--lynx-text-maxlength", "1.5", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", "1px", 60.0, 42.0),
        ] {
            let (mut document, text) = paragraph("abc def");
            set_limit(&mut document, text, attribute, Some("1"));
            document.set_inline_style_property(text, property, value);
            assert_height(&mut document, text, height);
            assert_eq!(document.text_block_size(text).unwrap().width, width);
            assert_eq!(document.get(text).unwrap().attribute(attribute), Some("1"));
        }
    }

    #[test]
    fn author_css_overrides_text_attributes_and_removing_it_restores_the_attribute() {
        for (attribute, property, width, height) in [
            (MAX_LINES, "--lynx-text-maxline", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", 40.0, 21.0),
        ] {
            let (mut document, text) = paragraph("abc def");
            set_limit(&mut document, text, attribute, Some("1"));
            document.add_class(text, "override");
            document.add_stylesheet(
                &format!("@layer limits {{ .override {{ {property}: 2; }} }}"),
                terminus::StylesheetOrigin::Author,
            );
            for value in [Some("1"), Some("3"), None, Some("1")] {
                set_limit(&mut document, text, attribute, value);
                assert_height(&mut document, text, height);
                assert_eq!(document.text_block_size(text).unwrap().width, width);
            }
            document.remove_class(text, "override");
            assert_height(&mut document, text, 21.0);
            assert_eq!(
                document.text_block_size(text).unwrap().width,
                if attribute == MAX_LINES { 60.0 } else { 20.0 }
            );
        }
    }

    #[test]
    fn invalid_limits_preserve_attribute_selector_values() {
        for (attribute, first, invalid) in [(MAX_LINES, "1", "1.0"), (MAX_CHARS, "2", "2.9")] {
            let (mut document, text) = paragraph("abc def");
            document.add_stylesheet(
                &format!("[{attribute}=\"{invalid}\"] {{ padding-top: 7px; }}"),
                terminus::StylesheetOrigin::Author,
            );
            set_limit(&mut document, text, attribute, Some(first));
            document.layout();
            assert_eq!(document.rounded_layout(text).unwrap().size.height, 21.0);
            set_limit(&mut document, text, attribute, Some(invalid));
            document.layout();
            assert_eq!(
                document.rounded_layout(text).unwrap().size.height,
                42.0 + 7.0
            );
            assert_eq!(
                document.get(text).unwrap().attribute(attribute),
                Some(invalid)
            );
        }
    }

    /// The computed value of the registered property `name`, as CSS text. An
    /// unset property reports its registered initial value.
    fn custom_value(
        document: &LynxDocument,
        element: terminus::NodeId,
        name: &str,
        initial: &str,
    ) -> String {
        use terminus::stylo::custom_properties::Name;

        style_of(document, element)
            .custom_properties()
            .non_inherited
            .get(&Name::from(name))
            .map_or_else(|| initial.to_owned(), |value| value.to_variable_value().css)
    }

    /// A wide Ahem `text` at 20px whose run a `raw-text` carries, laid out.
    fn verbatim_paragraph(content: &str) -> (LynxDocument, terminus::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "width:400px;font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, content);
        document.layout();
        (document, text)
    }

    fn ink(document: &LynxDocument, text: terminus::NodeId) -> (f32, f32) {
        let size = document.text_block_size(text).expect("paragraph");
        (size.width, size.height)
    }

    /// Every native backend hands a `text`'s string to its platform engine
    /// unchanged (`raw_text_element.cc:16-46`, Android
    /// `BaseTextShadowNode.java:494-498`, iOS `LynxBaseTextShadowNode.m:240-249`),
    /// and so does every Lynx paragraph: leading spaces and space runs
    /// stay, every newline breaks the line and the spaces after it stay,
    /// trailing spaces hang.
    #[test]
    fn a_text_keeps_its_white_space_verbatim() {
        let (document, text) = verbatim_paragraph("  a   b  ");
        assert_eq!(ink(&document, text), (7.0 * 20.0, 21.0));

        let (document, text) = verbatim_paragraph("ab\n  cd");
        assert_eq!(
            ink(&document, text),
            (4.0 * 20.0, 2.0 * 21.0),
            "the raw-text carrier's newline breaks the line; its indent stays"
        );
    }

    /// A nested run's own `white-space-collapse` is not read: native parses no
    /// such property, so a nested `text` or `inline-text` run is kept verbatim
    /// like the root's own, whatever it declares.
    #[test]
    fn a_nested_run_is_kept_verbatim_whatever_its_white_space() {
        let (mut document, text) = verbatim_paragraph("a");
        let nested = element_under(&mut document, text, "text", "white-space: normal");
        append_run(&mut document, nested, "  b");
        let inline = element_under(&mut document, text, "inline-text", "white-space: normal");
        append_run(&mut document, inline, "  c");
        document.layout();
        assert_eq!(ink(&document, text), (7.0 * 20.0, 21.0));
    }

    /// A `text` attribute's generated run is verbatim too: its newline is a
    /// forced break, as the attribute form is on every native backend.
    #[test]
    fn a_text_attribute_s_newline_breaks_the_line() {
        let (mut document, text) = verbatim_paragraph("");
        document.set_attribute(text, "text", "ab\ncd");
        document.layout();
        assert_eq!(ink(&document, text), (2.0 * 20.0, 2.0 * 21.0));
    }

    /// One computed longhand of `element`, as CSS text.
    fn computed(document: &LynxDocument, element: terminus::NodeId, property: &str) -> String {
        use terminus::stylo::properties::PropertyId;

        let id = PropertyId::parse_enabled_for_all_content(property)
            .unwrap_or_else(|()| panic!("unknown property `{property}`"));
        let longhand = id
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
        style_of(document, element).computed_value_to_string(longhand)
    }

    /// Native parses `white-space` as `normal` or `nowrap` and drops every
    /// other keyword (`enum_handler.cc:76-89`), so on a `text` only `nowrap`
    /// stops the wrapping and shows nothing after the first newline (Android
    /// `TextRenderer.java:850-867`); `pre`, `pre-wrap`, `pre-line` and
    /// `break-spaces` wrap at the box width with every line shown, as
    /// `normal` does. Spaces and newlines are kept under every keyword.
    #[test]
    fn only_nowrap_stops_a_text_from_wrapping() {
        // Fifteen `a`, a space, ten `b`, a newline, `cc`: at 400px (twenty
        // em squares) the first line ends after the `a`s.
        const CONTENT: &str = "aaaaaaaaaaaaaaa bbbbbbbbbb\ncc";
        for (keyword, wrap_mode, collapse, expected) in [
            ("normal", "wrap", "collapse", (15.0 * 20.0, 3.0 * 21.0)),
            ("pre", "nowrap", "preserve", (15.0 * 20.0, 3.0 * 21.0)),
            ("pre-wrap", "wrap", "preserve", (15.0 * 20.0, 3.0 * 21.0)),
            (
                "pre-line",
                "wrap",
                "preserve-breaks",
                (15.0 * 20.0, 3.0 * 21.0),
            ),
            (
                "break-spaces",
                "wrap",
                "break-spaces",
                (15.0 * 20.0, 3.0 * 21.0),
            ),
            ("nowrap", "nowrap", "collapse", (26.0 * 20.0, 21.0)),
        ] {
            let (mut document, text) = verbatim_paragraph(CONTENT);
            document.set_inline_style_property(text, "white-space", keyword);
            document.layout();
            assert_eq!(
                computed(&document, text, "text-wrap-mode"),
                wrap_mode,
                "{keyword}"
            );
            assert_eq!(
                computed(&document, text, "white-space-collapse"),
                collapse,
                "{keyword}"
            );
            assert_eq!(ink(&document, text), expected, "{keyword}");
        }
    }

    /// Changing `white-space` between `nowrap` and `pre` keeps
    /// `text-wrap-mode: nowrap` and changes only the collapse half, which is
    /// what tells them apart on a `text`: the paragraph is rebuilt either way.
    #[test]
    fn switching_nowrap_to_pre_shows_every_line_again() {
        let (mut document, text) = verbatim_paragraph("ab\ncdef");
        for (keyword, expected) in [
            ("nowrap", (2.0 * 20.0, 21.0)),
            ("pre", (4.0 * 20.0, 2.0 * 21.0)),
            ("nowrap", (2.0 * 20.0, 21.0)),
        ] {
            document.set_inline_style_property(text, "white-space", keyword);
            document.layout();
            assert_eq!(ink(&document, text), expected, "{keyword}");
        }
    }

    /// A newline that ends a `text`'s text adds an empty last line, as
    /// Android's `StaticLayout` does and Lynx measures
    /// (`TextRenderer.java:720`) — wrapped or not, written in a carrier or in
    /// the `text` attribute. Under `nowrap` it adds nothing: the text already
    /// ends at its first newline.
    #[test]
    fn a_newline_that_ends_a_text_adds_an_empty_line() {
        let (document, text) = verbatim_paragraph("ab\n");
        assert_eq!(ink(&document, text), (2.0 * 20.0, 2.0 * 21.0));

        let (document, text) = verbatim_paragraph("aaaaaaaaaaaaaaa bbbbbbbbbb\n");
        assert_eq!(ink(&document, text), (15.0 * 20.0, 3.0 * 21.0));

        let (mut document, text) = verbatim_paragraph("");
        document.set_attribute(text, "text", "ab\n");
        document.layout();
        assert_eq!(ink(&document, text), (2.0 * 20.0, 2.0 * 21.0));

        let (mut document, text) = verbatim_paragraph("ab\n");
        document.set_inline_style_property(text, "white-space", "nowrap");
        document.layout();
        assert_eq!(ink(&document, text), (2.0 * 20.0, 21.0));
    }

    /// `tail-color-convert` is a native BOOL whose default is off, so only the
    /// literal `true` — what `ReactLynx`'s `tail-color-convert={true}` reaches
    /// the DOM as — selects the enabled rule. Anything else, a removal included,
    /// leaves the rule unmatched and uses the registered initial 0.
    #[test]
    fn tail_color_convert_selects_only_the_literal_true() {
        let (mut document, text) = paragraph("abc def");
        for (value, expected) in [
            (Some("true"), "1"),
            (Some("false"), "0"),
            (Some("TRUE"), "0"),
            (Some("1"), "0"),
            (Some(""), "0"),
            (Some("true"), "1"),
            (None, "0"),
        ] {
            set_limit(&mut document, text, TAIL_COLOR_CONVERT, value);
            document.layout();
            assert_eq!(
                custom_value(&document, text, "lynx-tail-color-convert", "0"),
                expected,
                "tail-color-convert={value:?}",
            );
            assert_eq!(
                document.get(text).unwrap().attribute(TAIL_COLOR_CONVERT),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    #[test]
    fn text_overflow_selects_the_existing_ellipsis_path() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        for (overflow, width) in [("clip", 20.0), ("ellipsis", 80.0), ("clip", 20.0)] {
            document.set_inline_style_property(text, "text-overflow", overflow);
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    /// The width of the paragraph `text` establishes, after a layout.
    fn paragraph_width(document: &mut LynxDocument, text: terminus::NodeId) -> f32 {
        document.layout();
        document.text_block_size(text).expect("paragraph").width
    }

    /// Native Lynx reads `text-overflow` off the element as well as out of
    /// CSS (`text_element.cc:176-182`), and the UA sheet's two attribute rules
    /// are how this engine honours that. Native's enum reader accepts the two
    /// literals only, so every other value — a removal, a different case, an
    /// empty string — leaves the initial `clip`.
    #[test]
    fn text_overflow_attribute_selects_the_ellipsis_path_through_the_ua_sheet() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        for (value, width) in [
            (Some("ellipsis"), 80.0),
            (Some("clip"), 20.0),
            (Some("ellipsis"), 80.0),
            (None, 20.0),
            (Some("ELLIPSIS"), 20.0),
            (Some("garbage"), 20.0),
            (Some(""), 20.0),
            (Some("ellipsis"), 80.0),
        ] {
            set_limit(&mut document, text, TEXT_OVERFLOW, value);
            assert_eq!(
                paragraph_width(&mut document, text),
                width,
                "text-overflow={value:?}",
            );
            assert_eq!(
                document.get(text).unwrap().attribute(TEXT_OVERFLOW),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    /// The attribute's rules are UA origin and plain, so a page's own CSS
    /// overrides them exactly as it overrides the rest of the UA sheet.
    #[test]
    fn author_css_outranks_the_text_overflow_attribute() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        set_limit(&mut document, text, TEXT_OVERFLOW, Some("ellipsis"));
        document.add_class(text, "override");
        document.add_stylesheet(
            "@layer limits { .override { text-overflow: clip; } }",
            terminus::StylesheetOrigin::Author,
        );
        assert_eq!(paragraph_width(&mut document, text), 20.0);
        assert_eq!(
            document.get(text).unwrap().attribute(TEXT_OVERFLOW),
            Some("ellipsis"),
            "the author declaration wins the cascade without touching the attribute",
        );

        document.remove_class(text, "override");
        assert_eq!(paragraph_width(&mut document, text), 80.0);
    }

    #[test]
    fn a_text_does_not_wear_an_ancestor_s_color() {
        let mut document = document();
        let view = child(&mut document, "view", "color: rgb(1, 2, 3)");
        let text = element_under(&mut document, view, "text", "");
        let nested = element_under(&mut document, view, "view", "");
        document.layout();

        assert_eq!(
            style_of(&document, nested).get_color().solid_color(),
            style_of(&document, view).get_color().solid_color(),
            "a view still inherits color: only text is reset"
        );
        assert_eq!(
            style_of(&document, text).get_color().solid_color(),
            AbsoluteColor::BLACK,
            "`color: initial` stops the cascade at the text root"
        );
    }

    #[test]
    fn a_nested_text_wears_its_parent_text_s_color() {
        let mut document = document();
        let text = child(&mut document, "text", "color: rgb(1, 2, 3)");
        let direct = element_under(&mut document, text, "text", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "text", "");
        document.layout();

        let parent = style_of(&document, text).get_color().solid_color();
        assert_ne!(parent, AbsoluteColor::BLACK);
        for nested in [direct, through_wrapper] {
            assert_eq!(
                style_of(&document, nested).get_color().solid_color(),
                parent
            );
        }
    }

    /// A Lynx `color` holds a gradient as readily as a solid, and the glyph
    /// painter reads that whole value — so the reset and the opt-in have to
    /// carry the gradient too, not just the solid it collapses to.
    #[test]
    fn the_color_reset_and_its_opt_in_carry_a_text_gradient() {
        const GRADIENT: &str = "color: linear-gradient(90deg, rgb(1, 2, 3), rgb(4, 5, 6))";

        let mut document = document();
        let painted = child(&mut document, "text", GRADIENT);
        let nested = element_under(&mut document, painted, "text", "");
        let view = child(&mut document, "view", GRADIENT);
        let under_view = element_under(&mut document, view, "text", "");
        document.layout();

        for gradient in [painted, nested] {
            assert!(
                matches!(
                    style_of(&document, gradient).get_color(),
                    ColorPropertyValue::Gradient(_)
                ),
                "`inherit` hands a nested run the gradient, not the black it collapses to"
            );
        }
        assert!(
            matches!(
                style_of(&document, under_view).get_color(),
                ColorPropertyValue::Color(_)
            ),
            "`color: initial` stops an ancestor's gradient at the text root"
        );
    }

    /// Custom truncation content is content only where web-core looks for it:
    /// `XTextTruncation.ts` queries `:scope > inline-truncation`, so a marker
    /// written directly inside a `text` becomes a text scope and one written
    /// anywhere else — through a wrapper, or outside a paragraph entirely —
    /// keeps the `display: none` the tag carries by default.
    #[test]
    fn an_inline_truncation_is_content_only_as_a_text_s_own_child() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let marker = element_under(&mut document, text, "inline-truncation", "");
        let second = element_under(&mut document, text, "inline-truncation", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "inline-truncation", "");
        let outside = child(&mut document, "inline-truncation", "");
        document.layout();

        for (label, scope) in [("first", marker), ("second", second)] {
            assert_eq!(
                display(&document, scope),
                Display::Flex,
                "a text's own inline-truncation child keeps the initial display: {label}",
            );
            assert_eq!(
                document.paragraph_of(scope),
                Some(text),
                "and is a text scope of the paragraph around it: {label}",
            );
            assert_eq!(
                custom_value(&document, scope, "lynx-inline-truncation", "0"),
                "1",
                "and says so in computed style, which is how the layout host \
                 finds it without naming the tag: {label}",
            );
        }
        for (label, elsewhere) in [("under a wrapper", through_wrapper), ("outside", outside)] {
            assert_eq!(
                display(&document, elsewhere),
                Display::None,
                "and nowhere else: {label}",
            );
        }
    }

    #[test]
    fn a_text_renders_text_and_the_tags_that_are_content() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let boxes = ["view", "image"].map(|tag| (tag, element_under(&mut document, text, tag, "")));
        let nested = element_under(&mut document, text, "text", "");
        let nested_inline = element_under(&mut document, text, "inline-text", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "view", "");
        let foreign = element_under(&mut document, text, "x-foreign", "");
        let outside = child(&mut document, "x-foreign", "");
        document.layout();

        for (tag, content) in boxes {
            assert_eq!(display(&document, content), Display::Flex, "{tag}");
        }
        for (tag, scope) in [("text", nested), ("inline-text", nested_inline)] {
            assert_eq!(
                display(&document, scope),
                Display::Flex,
                "a nested text scope keeps the initial display: {tag}"
            );
            assert_eq!(
                document.paragraph_of(scope),
                Some(text),
                "and is part of the paragraph, not a box in it: {tag}"
            );
        }
        assert_eq!(display(&document, wrapper), Display::Contents);
        assert_eq!(display(&document, through_wrapper), Display::Linear);
        assert_eq!(
            display(&document, foreign),
            Display::None,
            "anything else written inside a text generates no box"
        );
        assert_eq!(
            display(&document, outside),
            Display::Flex,
            "and the suppression reaches no further than a text's own children"
        );
    }

    /// A text tag — a block `text`, a nested run, an `inline-text`, a text's
    /// own `inline-truncation` — keeps the author's `display` as its computed
    /// value and lays out by its kind: `none` hides it, and every other value
    /// but `contents`
    /// (`a_text_with_display_contents_is_undefined_and_only_does_not_crash`)
    /// leaves it a Lynx paragraph, or a text scope of the paragraph around it.
    #[test]
    fn an_author_display_hides_a_text_scope_only_with_none() {
        for (author, computed) in [
            ("display: none", Display::None),
            ("display: flex", Display::Flex),
            ("display: grid", Display::Grid),
            ("display: linear", Display::Linear),
        ] {
            let mut document = document();
            let text = child(&mut document, "text", author);
            let run = document.create_text_node("ab", ());
            document.append_child(text, run);
            let host = child(&mut document, "text", "");
            let scopes = [
                (
                    "nested text",
                    element_under(&mut document, host, "text", author),
                ),
                (
                    "inline-text",
                    element_under(&mut document, host, "inline-text", author),
                ),
                (
                    "inline-truncation",
                    element_under(&mut document, host, "inline-truncation", author),
                ),
            ];
            for (_, scope) in scopes {
                let run = document.create_text_node("ab", ());
                document.append_child(scope, run);
            }
            document.layout();

            assert_eq!(display(&document, text), computed, "text, {author}");
            let hidden = computed == Display::None;
            assert_eq!(
                document.text_block_size(text).is_none(),
                hidden,
                "a text establishes its paragraph unless it is hidden: {author}"
            );
            for (label, scope) in scopes {
                assert_eq!(display(&document, scope), computed, "{label}, {author}");
                assert_eq!(
                    document.paragraph_of(scope),
                    (!hidden).then_some(host),
                    "a scope is part of its host's paragraph unless it is hidden: \
                     {label}, {author}"
                );
            }
        }
    }

    /// `<text style="display: contents">` is undefined behaviour, by the
    /// user's ruling of 2026-10-17: Lynx gives a text no `contents` behaviour
    /// to follow, and the engine handles the value only so that it does not
    /// crash. What this test observes is **not a supported behaviour**, and
    /// nothing may rely on it. Today `contents` acts as computed, as on every
    /// element, before the element's kind is read
    /// (`terminus::layout::style::display_mode`): the text generates no box and
    /// establishes no paragraph, and its run is loose text of the `view`
    /// around it.
    #[test]
    fn a_text_with_display_contents_is_undefined_and_only_does_not_crash() {
        let mut document = document();
        let view = child(&mut document, "view", "");
        let text = element_under(&mut document, view, "text", "display: contents");
        let run = document.create_text_node("hello", ());
        document.append_child(text, run);
        document.layout();
        document.render();

        assert_eq!(display(&document, text), Display::Contents);
        assert_eq!(
            document.bounding_client_rect(text),
            None,
            "the text element has no box"
        );
        assert_eq!(document.text_block_size(text), None);
    }

    /// Runs `getTextBoundingRect` (or `name`) on `text` with `params`.
    fn invoke(
        document: &mut LynxDocument,
        text: terminus::NodeId,
        name: &str,
        params: &str,
    ) -> terminus::MethodOutcome {
        document.invoke_element_method(text, terminus::MethodCall { name, params })
    }

    /// The record `text_bounding_rect` answers: the count, then `left`,
    /// `top`, `width`, `height` per box.
    fn boxes_record(boxes: &[[f32; 4]]) -> terminus::MethodOutcome {
        let mut record = String::new();
        crate::main::record::write_record_field(&mut record, &boxes.len().to_string());
        for values in boxes {
            for value in values {
                crate::main::record::write_record_field(&mut record, &value.to_string());
            }
        }
        terminus::MethodOutcome::Data(record)
    }

    #[test]
    fn get_text_bounding_rect_answers_one_box_per_line_in_viewport_coordinates() {
        const AHEM: &[u8] = include_bytes!("../../../../seldon/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(
            document.register_fonts(terminus::FontBlob::from_static(AHEM)),
            1
        );
        let text = child(
            &mut document,
            "text",
            "position:absolute;left:20px;top:30px;padding:5px;border:2px solid;width:100px;\
             font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, "abc def");
        document.layout();
        // The content box starts at (20 + 2 + 5, 30 + 2 + 5) = (27, 37); the
        // paragraph wraps after "abc ", 21px lines.
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":1,"end":6}"#
            ),
            boxes_record(&[[47.0, 37.0, 40.0, 21.0], [27.0, 58.0, 40.0, 21.0]]),
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":0,"end":3}"#
            ),
            boxes_record(&[[27.0, 37.0, 60.0, 21.0]]),
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":5,"end":5}"#
            ),
            boxes_record(&[[27.0, 37.0, 0.0, 21.0]]),
            "an empty range is the natives' zero box at the content box's origin"
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":1.9,"end":2.5}"#
            ),
            boxes_record(&[[47.0, 37.0, 20.0, 21.0]]),
            "offsets truncate toward zero"
        );
        assert_eq!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":-0.5,"end":1,"x":0}"#
            ),
            boxes_record(&[[27.0, 37.0, 20.0, 21.0]]),
            "-0.5 truncates to 0, which is not negative; other keys are ignored"
        );
    }

    #[test]
    fn get_text_bounding_rect_refuses_bad_params_and_finds_nothing_with_code_8() {
        let (mut document, text) = paragraph("abc def");
        let nested = element_under(&mut document, text, "text", "");
        append_run(&mut document, nested, "gh");
        let view = child(&mut document, "view", "width:10px;height:10px");
        document.layout();
        let invalid = terminus::MethodOutcome::Failed(terminus::MethodError::InvalidParams);
        let not_found = terminus::MethodOutcome::Failed(terminus::MethodError::Operation);
        for params in [
            "{}",
            "null",
            "[]",
            "3",
            r#"{"start":1}"#,
            r#"{"end":1}"#,
            r#"{"start":"1","end":"2"}"#,
            r#"{"start":-1,"end":2}"#,
            r#"{"start":0,"end":-1}"#,
            r#"{"start":3,"end":1}"#,
            "not json",
        ] {
            assert_eq!(
                invoke(&mut document, text, "getTextBoundingRect", params),
                invalid,
                "{params}"
            );
        }
        for params in [
            // Past the source: "abc def" and the nested "gh" are nine units.
            r#"{"start":0,"end":10}"#,
            r#"{"start":1e12,"end":1e12}"#,
            // The space the soft wrap consumed is on no line.
            r#"{"start":3,"end":4}"#,
        ] {
            assert_eq!(
                invoke(&mut document, text, "getTextBoundingRect", params),
                not_found,
                "{params}"
            );
        }
        assert!(matches!(
            invoke(
                &mut document,
                text,
                "getTextBoundingRect",
                r#"{"start":7,"end":9}"#
            ),
            terminus::MethodOutcome::Data(_)
        ));
        assert_eq!(
            invoke(
                &mut document,
                nested,
                "getTextBoundingRect",
                r#"{"start":0,"end":1}"#
            ),
            not_found,
            "a nested text is a run of its ancestor's paragraph and establishes none"
        );
        for name in ["getSelectedText", "setTextSelection", "boundingClientRect"] {
            assert_eq!(
                invoke(&mut document, text, name, "{}"),
                terminus::MethodOutcome::NotFound,
                "{name}: no selection model; the base set answers the rest"
            );
        }
        assert_eq!(
            invoke(
                &mut document,
                view,
                "getTextBoundingRect",
                r#"{"start":0,"end":1}"#
            ),
            terminus::MethodOutcome::NotFound,
            "only `text` has the method"
        );
    }

    // --- `selectionchange` ---------------------------------------------------

    /// A text node `content` appended to `parent`: a DOM text child, which
    /// an editing host's value is made of (a `raw-text` carrier is not).
    fn append_text(document: &mut LynxDocument, parent: terminus::NodeId, content: &str) {
        let node = document.create_text_node(content, ());
        document.append_child(parent, node);
    }

    /// Focuses `host` and selects `anchor`..`focus` of its value, in UTF-16
    /// units.
    fn select(document: &mut LynxDocument, host: terminus::NodeId, anchor: usize, focus: usize) {
        if document.focused() != Some(host) {
            assert!(document.focus(host));
        }
        assert!(
            document
                .apply_editing(terminus::editing::Command::SetSelection {
                    range: anchor..focus,
                })
                .applied
        );
    }

    /// The record of `element` for the document's selection now.
    fn record_of(
        document: &LynxDocument,
        element: terminus::NodeId,
    ) -> Option<(std::ops::Range<u32>, bool)> {
        super::text_selection_in(document, element, document.selection()?)
    }

    /// `<text contenteditable>before<inline-text>inline</inline-text>after</text>`:
    /// the host's value is its direct text children, `beforeafter`, and a
    /// boundary in `after` is placed after the `inline-text`'s six units in
    /// the `text`'s content. The `inline-text` inside the host holds no
    /// boundary, so its record is the empty one.
    #[test]
    fn a_text_s_selection_counts_its_whole_text_content() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        document.set_attribute(text, "contenteditable", "");
        append_text(&mut document, text, "before");
        let inline = element_under(&mut document, text, "inline-text", "");
        append_text(&mut document, inline, "inline");
        append_text(&mut document, text, "after");
        document.layout();

        select(&mut document, text, 1, 4);
        assert_eq!(record_of(&document, text), Some((1..4, true)));
        assert_eq!(record_of(&document, inline), None);

        // Value unit 8 is the third unit of `after`, 6 + 6 + 2 = 14 into the
        // content; 7 is 13.
        select(&mut document, text, 8, 7);
        assert_eq!(record_of(&document, text), Some((13..14, false)));
        assert_eq!(record_of(&document, inline), None);

        select(&mut document, text, 3, 3);
        assert_eq!(record_of(&document, text), None, "collapsed");

        let mut other = super::super::test_support::document();
        let plain = child(&mut other, "text", "");
        other.set_attribute(plain, "contenteditable", "");
        append_text(&mut other, plain, "abc");
        other.layout();
        select(&mut other, plain, 0, 2);
        assert_eq!(record_of(&other, plain), Some((0..2, true)));
    }

    /// A `text` around a host that is a nested `inline-text` counts the text
    /// before the host, and the host reports its own offsets.
    #[test]
    fn an_inline_text_host_s_selection_counts_from_its_text() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        append_text(&mut document, text, "ab");
        let inline = element_under(&mut document, text, "inline-text", "");
        document.set_attribute(inline, "contenteditable", "");
        append_text(&mut document, inline, "cdef");
        document.layout();

        select(&mut document, inline, 1, 3);
        assert_eq!(record_of(&document, inline), Some((1..3, true)));
        assert_eq!(record_of(&document, text), Some((3..5, true)));
        let view = child(&mut document, "view", "");
        assert_eq!(record_of(&document, view), None, "outside the element");
    }

    /// The Lynx `selectionchange` is queued only for a listened-to element,
    /// only when its record changed, and the empty record only after a
    /// non-empty one; an element nobody listens on any more is forgotten.
    #[test]
    fn selection_reports_are_deduped_and_the_empty_record_follows_a_selection() {
        use terminus::event::{ListenerKey, ListenerKinds};

        use super::super::test_support::with_component_events;
        use super::super::{ComponentDetail, ComponentEvent, PageConfig};
        use super::{SELECTION_CHANGE, SelectionReports, report_selection_changes};

        let (mut document, events) = with_component_events(PageConfig::default());
        let text = child(&mut document, "text", "");
        document.set_attribute(text, "contenteditable", "");
        append_text(&mut document, text, "abc");
        document.layout();
        let mut reports = SelectionReports::default();
        let report = |document: &LynxDocument, reports: &mut SelectionReports| {
            report_selection_changes(document, reports, &events);
            events.take()
        };
        let queued = |range, forward| {
            vec![ComponentEvent::Detailed {
                node: text,
                name: SELECTION_CHANGE,
                detail: ComponentDetail::TextSelection { range, forward },
            }]
        };

        select(&mut document, text, 0, 2);
        assert_eq!(report(&document, &mut reports), [], "nothing listens");

        let realm = ListenerKey(1);
        document.add_listener(text, SELECTION_CHANGE, ListenerKinds::PATH_BUBBLE, realm);
        assert_eq!(report(&document, &mut reports), queued(Some(0..2), true));
        assert_eq!(report(&document, &mut reports), [], "the same record");
        select(&mut document, text, 2, 0);
        assert_eq!(report(&document, &mut reports), queued(Some(0..2), false));

        assert!(document.blur());
        assert_eq!(report(&document, &mut reports), queued(None, true));
        assert_eq!(report(&document, &mut reports), [], "empty after empty");

        // A collapsed selection is the empty record too, and is not sent to
        // an element whose last record was empty.
        select(&mut document, text, 1, 1);
        assert_eq!(report(&document, &mut reports), []);
        select(&mut document, text, 1, 3);
        assert_eq!(report(&document, &mut reports), queued(Some(1..3), true));

        // Unbound: forgotten, so a new listener starts from the empty
        // record and hears the same range again.
        document.remove_listener(text, SELECTION_CHANGE, realm);
        assert!(document.blur());
        assert_eq!(report(&document, &mut reports), []);
        document.add_listener(text, SELECTION_CHANGE, ListenerKinds::PATH_BUBBLE, realm);
        select(&mut document, text, 1, 3);
        assert_eq!(report(&document, &mut reports), queued(Some(1..3), true));
    }
}
