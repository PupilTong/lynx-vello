//! `x-input` and `x-textarea` — web-core's element names
//! (`web-elements/src/elements/XInput/XInput.ts`, `XTextarea/XTextarea.ts`) —
//! and the `x-input-ng`/`x-textarea-ng` aliases web-core maps to them
//! (`web-core/ts/constants.ts:105,107`), as Lynx elements over
//! `dom::editing` (`docs/editable-text-design.md`, E5).
//!
//! **Lynx tags only** (user ruling 2026-10-19). `input` and `textarea` are
//! HTML element names, and an element with an HTML name follows HTML, which
//! this component does not implement; so neither is defined here, though
//! web-core maps both to its fields (`constants.ts:104,106`) and native Lynx
//! registers its fields under them (`LynxUIInputAutoRegistry.m`,
//! `LynxUITextAreaAutoRegistry.m`). An HTML `<input>`/`<textarea>` would be
//! a separate element with HTML's semantics, not built.
//!
//! A Lynx-only component with no W3C equivalent, so `AGENTS.md`'s standards
//! policy puts it in the second bucket; web-core's structure and behaviour
//! are the ones followed (`web-elements/src/elements/XInput/*`,
//! `XTextarea/*`), as web-core wraps a real `<input>` or `<textarea>`, except
//! for `maxlines`, which web-core does not read and native Lynx enforces.
//!
//! # The structure
//!
//! [`TextField::constructed`] attaches a UA shadow tree of two elements:
//!
//! - `#input` (`#textarea` on a textarea), `part` of the same name: the **editor**, a
//!   `contenteditable="plaintext-only"` element — one `dom` editing host. Its `enterkeyhint`,
//!   `inputmode` and `type` carry what `confirm-type` and `type` say into
//!   [`dom::editing::TextInputState`]. The two kinds lay the value out in different paragraphs:
//!   - `x-input`'s editor is a `div` the engine's shadow sheet makes `display: block`, so its own
//!     standard inline formatting context lays out the one line, as a browser's `<input>` does. No
//!     Lynx text semantics reach it.
//!   - `x-textarea`'s editor is a Lynx `<text>` (the tag [`super::text`] defines as a Lynx text
//!     element), so its Lynx paragraph lays out the lines: verbatim white space, Lynx's `text-*`
//!     properties, the empty last line after a trailing line break, and the line count `maxlines`
//!     is enforced against. Its `color` is inherited from the field (`text`'s UA `color: initial`
//!     is overridden by the shadow sheet, as web-core's inner `<textarea>` inherits it).
//! - `#placeholder`, `part="placeholder"`: an absolutely positioned box over the host's content box
//!   holding one block with the `placeholder` text (for both kinds: it is not edited), shown by the
//!   shadow sheet only while the editor is `:empty` (`.editor:empty ~ #placeholder`), so no
//!   component code decides when it shows. The block is the field's **`::placeholder`**: it carries
//!   `pseudo="-webkit-input-placeholder"` ([`dom::PLACEHOLDER_PSEUDO_ID`], Blink's name for the
//!   element behind `::placeholder` in its own UA shadow trees), which makes it the host's
//!   `::placeholder` pseudo-element to `dom` — an author's `.hint::placeholder { color: red }`
//!   reaches it, over the attribute-driven defaults the box carries (an outer context's normal
//!   declaration wins, CSS Scoping §3.3). css-pseudo-4 §4.2 restricts what an author declares on
//!   `::placeholder` to the `::first-line` properties, `opacity`, `text-align`, `text-overflow` and
//!   the white-space properties, which Stylo enforces on every author declaration for the block,
//!   the shadow sheet's included; so the block's layout — `display: block`, `flex: 1 1 auto`,
//!   `min-width: 0`, `overflow: clip`, its `white-space` — is in [`UA_RULES`] as
//!   `x-input::placeholder` rules, where the restriction does not apply.
//!
//! **The host is the field's box.** web-core's host is `display: contents` and its inner
//! `<input>` inherits `width`, `height`, `border`, `padding`, `background` and the rest through
//! `::part(input)` rules (`x-input.css`); here the host keeps those properties itself and the
//! editor fills its content box, which draws the same field without copying a dozen properties
//! into the shadow tree. A press anywhere in the host — its padding, above or below a
//! vertically centred line — focuses the editor ([`focus_delegate`], HTML's `delegatesFocus`
//! for this one component).
//!
//! **Single line.** An `x-input`'s editor is `white-space: pre`: every character kept and no
//! wrapping; an `x-textarea`'s is `pre-wrap`, which a Lynx paragraph reads as "wrap" (it keeps
//! white space verbatim whatever the value). Both come from the shadow sheet, which outranks
//! the UA editing-host rule's `pre-wrap` default. The input's value never
//! contains a line break: an `input` that inserted one is followed by the component removing
//! every `\n` and `\r` from the value, as HTML's value sanitization strips them from an
//! `<input>`, and Enter (`insertLineBreak`) becomes `confirm`. Text wider than the field is
//! clipped at the editor's box; the field does not scroll to follow the caret.
//!
//! # Attributes
//!
//! | Attribute | Effect |
//! |---|---|
//! | `value` | the value, cut to `maxlength` UTF-16 units when that attribute is present (web-core's `_handleValue`); nothing when equal to the current value; the caret at the end, kept for the next focus when not focused |
//! | `placeholder` | the placeholder's text |
//! | `placeholder-color`, `-font-size`, `-font-weight`, `-font-family` | UA `attr()` declarations on the host, read by `#placeholder` (web-core's `--placeholder-*` properties, grey and `normal` by default) and inherited by its `::placeholder` block, where an author's `::placeholder` rule overrides them |
//! | `type` | `text`, `number`, `digit`, `password`, `tel`, `email`: the editor's `type` (`password` or `text`) and `inputmode` (web-core's `_handleType`); a `password` editor shows a disc per character (`-webkit-text-security: disc` in the shadow sheet); `number` also keeps digits and the last `.` only |
//! | `maxlength` | UTF-16 units, 140 when absent (web-core's default); see below |
//! | `confirm-type` | the editor's `enterkeyhint`, `send` when absent |
//! | `disabled`, `readonly` | present and not `"false"`: the editor is `contenteditable="false"`, so it cannot be focused or edited |
//! | `input-filter` | a pattern whose every match is removed from the value after each edit (web-core's `replace(new RegExp(filter, 'g'), '')`) |
//! | `send-composing-input` | present and not `"false"`: `input` fires during a composition too |
//! | `confirm-enter` | `x-textarea` only: Enter fires `confirm` after its line break |
//! | `maxlines` | `x-textarea` only: a positive whole number of lines the value may lay out in; see below |
//! | `min-height`, `max-height` | `x-textarea` only: a `<length>` (`rpx` included) clamping the host's box, from UA `attr()` declarations (below) |
//!
//! `min-height` and `max-height` are web-core's `bindToStyle` of the two
//! attributes onto its inner `<textarea>`'s inline style
//! (`XTextareaAttributes.ts`). The host is the field's box here, so the UA
//! sheet clamps the host's own box: `x-textarea[min-height] { min-height:
//! attr(min-height type(<length>), 0px) }` and the same for `max-height`
//! with `none`, typed `attr()` used directly in the property (`UA_RULES`).
//! A value that is no `<length>` takes that fallback, the value the field
//! has without the attribute. An author's own `min-height`/`max-height` on the host
//! outranks the attribute (author beats UA), whereas in web-core only the
//! attribute reaches the inner box: an accepted difference.
//!
//! # Edits: filters and `maxlength` after the fact
//!
//! `dom` runs the edit and the component cannot cancel it: its handler is the
//! Rust walk of the standard `input` the editor dispatches, after the commit.
//! So the component corrects the value afterwards, as web-core corrects its
//! `<input>`'s in its `input` listener, and writes the result back with
//! [`dom::Document::set_host_value`] with the caret moved by what was
//! removed before it. Outside a composition, after each `input` and at
//! `compositionend`, in this order: line breaks (`x-input`), `maxlength`,
//! the `number` filter, `input-filter`, `maxlines` (`x-textarea`).
//! `maxlength` is the browser's insertion rule applied afterwards: a value
//! longer than the limit loses the excess from the end of what was just
//! inserted (the units before the caret, at most the inserted text's
//! length, never half a surrogate pair), so a value already over the limit
//! is not cut by a deletion or a lowered limit.
//!
//! `maxlines` is native Lynx's textarea rule (`LynxUITextArea.kt`'s
//! `maxLinesFilter` keeps the longest prefix of an insertion that lays out
//! in the limit; `LynxUITextArea.m` trims to it): after the edit the value
//! is written and laid out, and when the editor's Lynx paragraph has more
//! lines than the limit ([`dom::Document::paragraph_line_count`]) the
//! longest prefix of what the edit inserted that fits is kept — found by
//! bisection over its characters, one layout pass per probe, so never half
//! a surrogate pair — and the caret is placed after it. Lines are the
//! paragraph's laid-out lines: wrapped lines count, and so does the empty
//! line after a trailing line break, which the Lynx paragraph shows. Only
//! what the edit inserted is taken back, so a value already over the limit
//! is not cut by a deletion, and a script's write is never cut. The value
//! stays visible and editable throughout: this is not `text-maxline`
//! truncation, which would hide the lines past the limit.
//!
//! # Events
//!
//! The editor's standard events are dispatched **non-composed**
//! (`MainThreadRuntime::apply_edit`, `dispatch_focus_events`: an editing
//! host inside a UA shadow tree), so they stay in the shadow tree and only
//! this component hears them, at its shadow root
//! ([`dom::CustomElement::handle_event`]) — web-core stops its inner `<input>`'s
//! `input` at its `#form` the same way. The component re-fires them at the
//! host in web-core's shape, under the Lynx names (`bindinput` listens for
//! `input`; web-core's DOM names `lynxinput`/`lynxfocus`/`lynxblur` exist
//! only to step around the browser's own events), non-bubbling, on
//! [`ComponentEvents`]:
//!
//! - `input` `{value, textLength, cursor, isComposing, selectionStart, selectionEnd}` after an edit
//!   that changed the value since the last one reported; during a composition only with
//!   `send-composing-input`, and otherwise once at `compositionend`.
//! - `confirm` `{value}`: Enter in an `x-input` (its line break is removed, and no `input` fires);
//!   Enter in an `x-textarea` with `confirm-enter`, after its `input`.
//! - `selection` `{selectionStart, selectionEnd}`: a change of the selection that changed no value
//!   and leaves text selected (a browser's `select`), from a pointer, a key or the
//!   `setSelectionRange`/`select` methods. The runtime tells the component after every applied
//!   command with the engine-only `selectionchange` at the editor.
//! - `focus` and `blur` `{value}`.
//!
//! The last reported value and selection are kept for the focused editor
//! alone ([`Reported`]): one field is edited at a time, so there is no
//! per-element table.
//!
//! # Methods
//!
//! `focus`, `blur`, `controlKeyBoard`, `setValue({value, index})`,
//! `getValue()`, `setSelectionRange({selectionStart, selectionEnd})`,
//! `select()`, `addText({text})`, `sendDelEvent({action, length})` and
//! `setInputFilter({pattern})`, web-core's set (`XInput.ts`,
//! `XTextarea.ts`) plus the typings' `getValue` members. A method that
//! focuses or blurs leaves its focus events queued in `dom`; the page
//! dispatches them from an entry of its own.
//!
//! A field keeps its selection while it is not focused, as a browser's
//! `<input>` keeps `selectionStart`/`selectionEnd` across a blur
//! ([`dom::Document::host_selection`]): `focus` restores the selection the
//! editor had when it lost focus, or the caret a script's value write
//! (`value`, `setValue`, `addText`, `sendDelEvent`) placed since, and only
//! a field never focused nor written gets a caret at the end of its value,
//! where a browser's fresh `<input>` reports its selection. `getValue`,
//! `addText` and `sendDelEvent` on a field that is not focused read that
//! kept selection.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use dom::editing::Command;
use dom::event::{ElementEvent, EventDetail, EventPhase};
use dom::{CustomElement, MethodCall, MethodError, MethodOutcome, NodeId, ShadowRootMode};
use serde_json::Value;

use super::text::TEXT_TAG;
use super::{ComponentDetail, ComponentEvents, InvalidParams, LynxDocument};
use crate::main::record::write_record_field;

/// The single-line tags: web-core's element name and its `-ng` alias. Not
/// the HTML name `input` (module documentation).
pub(super) const INPUT_TAGS: [&str; 2] = ["x-input", "x-input-ng"];
/// The multi-line tags: web-core's element name and its `-ng` alias. Not
/// the HTML name `textarea`.
pub(super) const TEXTAREA_TAGS: [&str; 2] = ["x-textarea", "x-textarea-ng"];

/// The engine-only event the runtime fires at an editing host after every
/// command it applied, which this component reads its `selection` from.
pub(crate) const SELECTION_CHANGE: &str = "selectionchange";

const VALUE: &str = "value";
const PLACEHOLDER: &str = "placeholder";
const TYPE: &str = "type";
const MAXLENGTH: &str = "maxlength";
const CONFIRM_TYPE: &str = "confirm-type";
const DISABLED: &str = "disabled";
const READONLY: &str = "readonly";
const INPUT_FILTER: &str = "input-filter";
const SEND_COMPOSING_INPUT: &str = "send-composing-input";
const CONFIRM_ENTER: &str = "confirm-enter";
const MAXLINES: &str = "maxlines";
/// Where the editor keeps the filter pattern: `input-filter` and
/// `setInputFilter` both write it, so the last write wins. web-core's
/// `setInputFilter` writes the same attribute on its inner `<input>`.
const PATTERN: &str = "pattern";

/// web-core's default `maxlength` (`InputBaseAttributes.ts`), which the
/// typings document as well.
const DEFAULT_MAXLENGTH: usize = 140;

const PLACEHOLDER_ID: &str = "placeholder";

/// The light-tree UA rules: the host is a flex box carrying the field's
/// box, 14px text (web-core's `font-size: 14px`), and the placeholder style
/// attributes as inherited custom properties. A property whose attribute is
/// absent and has no fallback is invalid at computed-value time, so the
/// placeholder inherits that property from the host instead.
///
/// The `::placeholder` rules lay the placeholder's block out: a block
/// filling its row and clipping, on one verbatim line in an input and
/// wrapping verbatim lines in a textarea. They are UA rules because the
/// block implements `::placeholder` (module documentation), whose property
/// restriction Stylo applies to every author declaration for it, the shadow
/// sheet's included, and not to a UA one.
///
/// A textarea's `min-height` and `max-height` attributes clamp the host's
/// own box, the field's box here, through typed `attr()` used directly in
/// the two properties (web-core binds them to its inner `<textarea>`'s
/// inline style). A value that is no `<length>` takes the fallback, the
/// value the field has without the attribute: `0px`, the common UA block's
/// `min-height: 0` (`ua_sheet`), and `none`, `max-height`'s initial value.
/// Without the fallback such a value would be invalid at computed-value
/// time, and `min-height` would become its initial `auto`, not that `0`.
pub(super) const UA_RULES: &str = r"
x-input, x-input-ng, x-textarea, x-textarea-ng {
  display: flex; font-size: 14px;
  --placeholder-color: attr(placeholder-color type(<color>), grey);
  --placeholder-font-size: attr(placeholder-font-size type(<length>));
  --placeholder-font-weight: attr(placeholder-font-weight type(*), normal);
  --placeholder-font-family: attr(placeholder-font-family type(*));
}
x-input, x-input-ng { flex-direction: row; align-items: center; }
x-textarea, x-textarea-ng { flex-direction: column; align-items: stretch; }
x-input::placeholder, x-input-ng::placeholder, x-textarea::placeholder, x-textarea-ng::placeholder {
  display: block; min-width: 0; flex: 1 1 auto; overflow: clip;
}
x-input::placeholder, x-input-ng::placeholder { white-space: pre; }
x-textarea::placeholder, x-textarea-ng::placeholder { white-space: pre-wrap; }
x-textarea[min-height], x-textarea-ng[min-height] {
  min-height: attr(min-height type(<length>), 0px);
}
x-textarea[max-height], x-textarea-ng[max-height] {
  max-height: attr(max-height type(<length>), none);
}
";

/// The shadow sheet common to both kinds; [`INPUT_SHADOW_RULES`] and
/// [`TEXTAREA_SHADOW_RULES`] follow it. An engine sheet
/// ([`dom::Document::add_engine_shadow_stylesheet`]): the input's editor is
/// `display: block`, which the `lynx` grammar keeps from author sheets. The
/// placeholder's block has no rule here: it is the `::placeholder`, laid
/// out by [`UA_RULES`] (module documentation). The editor sets no
/// `min-height`: an empty editor is one line tall because its paragraph lays
/// out one empty line, a layout rule, not CSS (the `lh` unit is not parsed
/// under the fork's `lynx` feature).
const SHADOW_RULES: &str = r"
.editor {
  min-width: 0; overflow: clip;
}
#placeholder {
  display: none; position: absolute; top: 0; right: 0; bottom: 0; left: 0;
  padding: inherit; flex-direction: row; pointer-events: none; overflow: clip;
  color: var(--placeholder-color);
  font-size: var(--placeholder-font-size);
  font-weight: var(--placeholder-font-weight);
  font-family: var(--placeholder-font-family);
}
.editor:empty ~ #placeholder { display: flex; }
";

/// One line, vertically centred, as a browser's `<input>` draws it, in the
/// editor's own inline formatting context. A `password` editor masks its
/// value as a browser's `<input type=password>` does: a disc per character,
/// the value itself unchanged.
const INPUT_SHADOW_RULES: &str = r#"
.editor { display: block; flex: 1 1 0%; white-space: pre; }
.editor[type="password"] { -webkit-text-security: disc; }
#placeholder { align-items: center; }
"#;

/// Lines from the top, wrapping, in the editor's Lynx paragraph. `color:
/// inherit` undoes `text`'s UA `color: initial`: the value wears the field's
/// colour, as web-core's inner `<textarea>` does (`x-textarea.css`).
const TEXTAREA_SHADOW_RULES: &str = r"
.editor { flex: 1 1 auto; white-space: pre-wrap; color: inherit; }
#placeholder { align-items: flex-start; }
";

/// Which of the two fields an element is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Input,
    Textarea,
}

impl Kind {
    /// The editor's id and part: web-core's inner element's.
    const fn editor_id(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Textarea => "textarea",
        }
    }

    /// The editor's tag: a `div` the shadow sheet makes a block container
    /// for the single line, a Lynx `<text>` for the lines of a textarea.
    const fn editor_tag(self) -> &'static str {
        match self {
            Self::Input => "div",
            Self::Textarea => TEXT_TAG,
        }
    }
}

/// What the component last told script about the focused editor: the value
/// its last `input` reported (or the one it had at focus, or the one a
/// script wrote), and the selection then.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    editor: NodeId,
    value: String,
    selection: Range<u32>,
}

/// State shared by the four definitions: the event queue and the focused
/// editor's [`Reported`].
#[derive(Default)]
struct Shared {
    events: ComponentEvents,
    reported: RefCell<Option<Reported>>,
}

/// Installs the component under its four tag names. Must run before any
/// element could carry them.
pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents) {
    let shared = Rc::new(Shared {
        events,
        reported: RefCell::new(None),
    });
    for (tags, kind) in [(INPUT_TAGS, Kind::Input), (TEXTAREA_TAGS, Kind::Textarea)] {
        for tag in tags {
            document.define(
                tag,
                Box::new(TextField {
                    kind,
                    shared: Rc::clone(&shared),
                }),
            );
        }
    }
}

/// The `x-input`/`x-textarea` component; see the module documentation.
struct TextField {
    kind: Kind,
    shared: Rc<Shared>,
}

/// The kind of a text field host, `None` for any other node.
fn kind_of(document: &LynxDocument, node: NodeId) -> Option<Kind> {
    let tag = document.get(node)?.tag_name()?;
    if INPUT_TAGS.contains(&tag) {
        Some(Kind::Input)
    } else if TEXTAREA_TAGS.contains(&tag) {
        Some(Kind::Textarea)
    } else {
        None
    }
}

/// The editor in `field`'s shadow tree: its first element.
fn editor(document: &LynxDocument, field: NodeId) -> Option<NodeId> {
    let root = document.shadow_root(field)?;
    document
        .get(root)?
        .children()
        .find(|child| child.is_element())
        .map(dom::Node::id)
}

/// The block holding the placeholder's text, the only child of
/// `#placeholder`.
fn placeholder_block(document: &LynxDocument, field: NodeId) -> Option<NodeId> {
    let root = document.shadow_root(field)?;
    let placeholder = document
        .get(root)?
        .children()
        .filter(|child| child.is_element())
        .nth(1)?;
    placeholder.first_child().map(dom::Node::id)
}

/// The editing host a press at `node` focuses when `node` is in no editing
/// host itself: the editor of the text field `node` is, or is inside (its
/// shadow tree's content), when that editor can be edited. HTML's
/// `delegatesFocus` for the one component that has an editor: a press in
/// the field's padding, or above or below its centred line, focuses it.
pub(crate) fn focus_delegate(document: &LynxDocument, node: NodeId) -> Option<NodeId> {
    let mut current = Some(node);
    while let Some(id) = current {
        let candidate = document.get(id)?;
        if kind_of(document, id).is_some() {
            return editor(document, id).filter(|&editor| document.is_editing_host(editor));
        }
        current = candidate.parent_id();
    }
    None
}

fn attribute<'a>(document: &'a LynxDocument, element: NodeId, name: &str) -> Option<&'a str> {
    document.get(element).and_then(|node| node.attribute(name))
}

/// A Lynx boolean attribute: present and not `"false"` (web-core removes a
/// `"false"` attribute before its handlers see it).
fn flag(document: &LynxDocument, element: NodeId, name: &str) -> bool {
    attribute(document, element, name).is_some_and(|value| value != "false")
}

/// JavaScript's `parseFloat`: the longest leading decimal number after
/// white space, `None` for none.
fn parse_float(text: &str) -> Option<f64> {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut end = 0;
    let mut longest = None;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let mut seen_dot = false;
    let mut seen_digit = false;
    while let Some(&byte) = bytes.get(end) {
        match byte {
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot => seen_dot = true,
            _ => break,
        }
        end += 1;
        if seen_digit {
            longest = Some(end);
        }
    }
    longest.and_then(|end| text[..end].parse().ok())
}

/// The UTF-16 length of `text`.
fn units(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The byte offset of UTF-16 unit `unit` in `text`, rounded down to a
/// character boundary and clamped to the end.
fn byte_of_unit(text: &str, unit: usize) -> usize {
    let mut seen = 0;
    for (byte, character) in text.char_indices() {
        let next = seen + character.len_utf16();
        if next > unit {
            return byte;
        }
        seen = next;
    }
    text.len()
}

/// UTF-16 unit `unit` of `text` moved back by what the byte ranges
/// `removed` (ascending, disjoint) take out before it.
fn shift_unit(text: &str, removed: &[Range<usize>], unit: usize) -> usize {
    let unit_byte = byte_of_unit(text, unit);
    let mut after = unit;
    for range in removed {
        let before = range.start.min(unit_byte)..range.end.min(unit_byte);
        after -= units(&text[before]);
    }
    after
}

/// `text` with the byte ranges `removed` (ascending, disjoint) taken out,
/// and `caret` (UTF-16) moved back by what was removed before it.
fn remove_ranges(text: &str, removed: &[Range<usize>], caret: usize) -> (String, usize) {
    let mut kept = String::with_capacity(text.len());
    let mut at = 0;
    for range in removed {
        kept.push_str(&text[at..range.start]);
        at = range.end;
    }
    kept.push_str(&text[at..]);
    (kept, shift_unit(text, removed, caret))
}

/// The byte ranges of every line break: an `x-input`'s value sanitization.
fn line_breaks(value: &str) -> Vec<Range<usize>> {
    value
        .char_indices()
        .filter(|&(_, character)| matches!(character, '\n' | '\r'))
        .map(|(byte, character)| byte..byte + character.len_utf8())
        .collect()
}

/// web-core's `type="number"` filter, `/[^0-9.]|\.(?=.*\.)/g`: everything
/// but ASCII digits and the last `.`.
fn number_rejects(value: &str) -> Vec<Range<usize>> {
    let last_dot = value.rfind('.');
    value
        .char_indices()
        .filter(|&(byte, character)| {
            !(character.is_ascii_digit() || (character == '.' && Some(byte) == last_dot))
        })
        .map(|(byte, character)| byte..byte + character.len_utf8())
        .collect()
}

/// A pattern web-core hands to `new RegExp`, compiled by `regex-lite`:
/// ECMAScript's `\uXXXX` escapes become `\x{XXXX}`; a pattern it cannot
/// compile filters nothing.
fn compile_filter(pattern: &str) -> Option<regex_lite::Regex> {
    let mut translated = String::with_capacity(pattern.len());
    let mut chars = pattern.char_indices().peekable();
    while let Some((at, character)) = chars.next() {
        if character == '\\' {
            let rest = &pattern[at + 1..];
            if let Some(hex) = rest.strip_prefix('u').and_then(|rest| rest.get(..4))
                && hex.chars().all(|digit| digit.is_ascii_hexdigit())
            {
                translated.push_str("\\x{");
                translated.push_str(hex);
                translated.push('}');
                for _ in 0..5 {
                    chars.next();
                }
                continue;
            }
            translated.push('\\');
            if let Some((_, escaped)) = chars.next() {
                translated.push(escaped);
            }
            continue;
        }
        translated.push(character);
    }
    regex_lite::Regex::new(&translated).ok()
}

/// The byte ranges `pattern` matches in `value`, empty matches skipped.
fn filter_rejects(pattern: &str, value: &str) -> Vec<Range<usize>> {
    compile_filter(pattern).map_or_else(Vec::new, |regex| {
        regex
            .find_iter(value)
            .filter(|found| !found.is_empty())
            .map(|found| found.range())
            .collect()
    })
}

/// The `maxlength` that applies: the attribute as `parseFloat` reads it,
/// whole units, 140 when absent; `None` (no limit) for a negative or
/// unreadable value, which a browser's `maxlength` ignores.
fn max_length(document: &LynxDocument, field: NodeId) -> Option<usize> {
    let Some(text) = attribute(document, field, MAXLENGTH) else {
        return Some(DEFAULT_MAXLENGTH);
    };
    let limit = parse_float(text)?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a non-negative length, truncated as `substring` truncates its argument"
    )]
    (limit >= 0.0).then_some(limit as usize)
}

/// The `maxlines` that applies to a textarea: the attribute as `parseFloat`
/// reads it, whole lines; `None` (no limit) when absent, unreadable or not
/// positive, as native Lynx's `maxLinesFilter` reads `maxLines <= 0`.
fn max_lines(document: &LynxDocument, field: NodeId) -> Option<usize> {
    let limit = parse_float(attribute(document, field, MAXLINES)?)?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a positive line count, truncated as native's integer attribute reads it"
    )]
    (limit >= 1.0).then_some(limit as usize)
}

/// The value as the editing session holds it so far, the caret, and the
/// UTF-16 range of the value the edit inserted (it ends at the caret), as
/// [`TextField::sanitize`] corrects them one rule at a time.
struct Correction {
    value: String,
    caret: usize,
    inserted_from: usize,
}

impl Correction {
    /// Takes the byte ranges `removed` (ascending, disjoint) out of the
    /// value, moving the caret and the inserted range's start with them.
    fn remove(&mut self, removed: &[Range<usize>]) {
        if removed.is_empty() {
            return;
        }
        self.inserted_from = shift_unit(&self.value, removed, self.inserted_from);
        (self.value, self.caret) = remove_ranges(&self.value, removed, self.caret);
    }

    /// The value with only the first `kept` UTF-16 units of the inserted
    /// range left in it, and the caret after them.
    fn keeping(&self, kept: usize) -> (String, usize) {
        let start = byte_of_unit(&self.value, self.inserted_from + kept);
        let end = byte_of_unit(&self.value, self.caret);
        let value = format!("{}{}", &self.value[..start], &self.value[end..]);
        (value, self.inserted_from + kept)
    }
}

impl TextField {
    /// Builds the shadow tree; see the module documentation.
    fn build(&self, document: &mut LynxDocument, element: NodeId) {
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        let kind_rules = match self.kind {
            Kind::Input => INPUT_SHADOW_RULES,
            Kind::Textarea => TEXTAREA_SHADOW_RULES,
        };
        document.add_engine_shadow_stylesheet(shadow, &format!("{SHADOW_RULES}{kind_rules}"));
        let editor = document.create_element(self.kind.editor_tag(), ());
        let id = self.kind.editor_id();
        document.set_id_attribute(editor, Some(id));
        document.set_attribute(editor, "part", id);
        document.set_classes(editor, "editor");
        document.set_attribute(editor, "contenteditable", "plaintext-only");
        document.set_attribute(editor, "enterkeyhint", "send");
        document.set_attribute(editor, "inputmode", "text");
        document.set_attribute(editor, TYPE, "text");
        document.append_child(shadow, editor);
        let placeholder = document.create_element("div", ());
        document.set_id_attribute(placeholder, Some(PLACEHOLDER_ID));
        document.set_attribute(placeholder, "part", PLACEHOLDER_ID);
        let block = document.create_element("div", ());
        document.set_attribute(block, "pseudo", dom::PLACEHOLDER_PSEUDO_ID);
        document.append_child(placeholder, block);
        document.append_child(shadow, placeholder);
    }

    /// The field's current value: the editor's text.
    fn value(document: &LynxDocument, editor: NodeId) -> String {
        let mut value = String::new();
        if let Some(node) = document.get(editor) {
            for child in node.children() {
                if let Some(text) = child.text().filter(|_| child.is_text_node()) {
                    value.push_str(text);
                }
            }
        }
        value
    }

    /// The value and the selection (UTF-16) of `editor`: the session's when
    /// it is focused, otherwise the one it kept since it lost focus or a
    /// script last wrote its value ([`dom::Document::host_selection`]), as
    /// a browser's field keeps its selection across a blur; a caret at the
    /// end for a field that was never focused nor written.
    fn snapshot(document: &LynxDocument, editor: NodeId) -> (String, Range<u32>, bool) {
        if document.focused() == Some(editor) {
            let state = document.text_input_state();
            let composing = state.composition.is_some();
            return (state.value, state.selection, composing);
        }
        let value = Self::value(document, editor);
        let selection = document.host_selection(editor).unwrap_or_else(|| {
            let end = u32::try_from(units(&value)).unwrap_or(u32::MAX);
            end..end
        });
        (value, selection, false)
    }

    /// Writes a value the way script does — no `input` — with the caret at
    /// `caret` (UTF-16), and makes it the reported value when the editor is
    /// focused.
    fn write_value(&self, document: &mut LynxDocument, editor: NodeId, value: &str, caret: usize) {
        document.set_host_value(editor, value, caret..caret);
        if document.focused() == Some(editor) {
            let (value, selection, _) = Self::snapshot(document, editor);
            *self.shared.reported.borrow_mut() = Some(Reported {
                editor,
                value,
                selection,
            });
        }
    }

    /// Writes the `value` attribute: cut to `maxlength` units when that
    /// attribute is present, as web-core's `_handleValue` cuts it, and
    /// nothing when the field already shows it.
    fn apply_value_attribute(&self, document: &mut LynxDocument, element: NodeId) {
        let Some(editor) = editor(document, element) else {
            return;
        };
        let mut value = attribute(document, element, VALUE)
            .unwrap_or_default()
            .to_owned();
        if let Some(limit) = attribute(document, element, MAXLENGTH).and_then(parse_float) {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "`substring` truncates its argument and clamps it at 0"
            )]
            let limit = limit.max(0.0) as usize;
            value.truncate(byte_of_unit(&value, limit));
        }
        if Self::value(document, editor) != value {
            let end = units(&value);
            self.write_value(document, editor, &value, end);
        }
    }

    /// Corrects the value after an edit outside a composition: line breaks,
    /// `maxlength`, the `number` filter, `input-filter`, in that order.
    /// `inserted` is the UTF-16 length of what the edit inserted, which is
    /// the most `maxlength` takes back.
    fn sanitize(
        &self,
        document: &mut LynxDocument,
        field: NodeId,
        editor: NodeId,
        inserted: usize,
    ) {
        let (original, selection, _) = Self::snapshot(document, editor);
        let caret = selection.end as usize;
        let mut fix = Correction {
            inserted_from: caret.saturating_sub(inserted),
            value: original.clone(),
            caret,
        };
        if self.kind == Kind::Input {
            fix.remove(&line_breaks(&fix.value));
        }
        if let Some(limit) = max_length(document, field) {
            let length = units(&fix.value);
            if length > limit {
                let take = (length - limit).min(fix.caret - fix.inserted_from);
                let end = byte_of_unit(&fix.value, fix.caret);
                // Never half a pair: round the start down to its character.
                let start = byte_of_unit(&fix.value, fix.caret - take);
                fix.remove(std::slice::from_ref(&(start..end)));
            }
        }
        if self.kind == Kind::Input && attribute(document, field, TYPE) == Some("number") {
            fix.remove(&number_rejects(&fix.value));
        }
        if let Some(pattern) = attribute(document, editor, PATTERN).filter(|p| !p.is_empty()) {
            let rejects = filter_rejects(pattern, &fix.value);
            fix.remove(&rejects);
        }
        if fix.value != original {
            document.set_host_value(editor, &fix.value, fix.caret..fix.caret);
        }
        if self.kind == Kind::Textarea
            && let Some(limit) = max_lines(document, field)
        {
            Self::fit_lines(document, editor, &fix, limit);
        }
    }

    /// `maxlines` after an edit (module documentation): with `fix` written,
    /// keeps the longest prefix of the inserted text whose value lays out in
    /// `limit` lines of the editor's paragraph, by bisection over the
    /// inserted characters, one layout pass per probe. An editor with no
    /// committed paragraph (not rendered) is left as it is.
    fn fit_lines(document: &mut LynxDocument, editor: NodeId, fix: &Correction, limit: usize) {
        let inserted = &fix.value
            [byte_of_unit(&fix.value, fix.inserted_from)..byte_of_unit(&fix.value, fix.caret)];
        if inserted.is_empty() {
            return;
        }
        document.layout();
        if document
            .paragraph_line_count(editor)
            .is_none_or(|count| count <= limit)
        {
            return;
        }
        let fits = |document: &mut LynxDocument, kept: usize| {
            let (value, caret) = fix.keeping(kept);
            document.set_host_value(editor, &value, caret..caret);
            document.layout();
            document
                .paragraph_line_count(editor)
                .is_some_and(|count| count <= limit)
        };
        // UTF-16 lengths of the inserted text's character prefixes: kept
        // lengths never end inside a surrogate pair.
        let prefixes: Vec<usize> = inserted
            .chars()
            .scan(0, |length, character| {
                let before = *length;
                *length += character.len_utf16();
                Some(before)
            })
            .collect();
        // `prefixes[0]` (nothing kept) is the answer when nothing fits; the
        // whole insertion is known not to.
        let (mut low, mut high) = (0, prefixes.len() - 1);
        while low < high {
            let middle = (low + high).div_ceil(2);
            if fits(document, prefixes[middle]) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let (value, caret) = fix.keeping(prefixes[low]);
        document.set_host_value(editor, &value, caret..caret);
    }

    /// Queues `input` when the value differs from the last one reported.
    fn report_input(
        &self,
        document: &LynxDocument,
        field: NodeId,
        editor: NodeId,
        composing: bool,
    ) {
        let (value, selection, _) = Self::snapshot(document, editor);
        let mut reported = self.shared.reported.borrow_mut();
        if reported
            .as_ref()
            .is_some_and(|last| last.editor == editor && last.value == value)
        {
            if let Some(last) = reported.as_mut() {
                last.selection = selection;
            }
            return;
        }
        *reported = Some(Reported {
            editor,
            value: value.clone(),
            selection: selection.clone(),
        });
        self.shared.events.queue_detailed(
            field,
            "input",
            ComponentDetail::Input {
                value,
                selection,
                is_composing: composing,
            },
        );
    }

    fn queue_value(
        &self,
        document: &LynxDocument,
        field: NodeId,
        editor: NodeId,
        name: &'static str,
    ) {
        let (value, ..) = Self::snapshot(document, editor);
        self.shared
            .events
            .queue_detailed(field, name, ComponentDetail::Value { value });
    }

    /// Queues `selection` when the selection moved with no change of value
    /// and leaves text selected; records it either way.
    fn report_selection(&self, document: &LynxDocument, field: NodeId, editor: NodeId) {
        let (value, selection, _) = Self::snapshot(document, editor);
        let mut reported = self.shared.reported.borrow_mut();
        let Some(last) = reported.as_mut().filter(|last| last.editor == editor) else {
            return;
        };
        let moved = last.value == value && last.selection != selection;
        last.selection = selection.clone();
        if moved && !selection.is_empty() {
            self.shared.events.queue_detailed(
                field,
                "selection",
                ComponentDetail::Selection { selection },
            );
        }
    }

    fn on_input(
        &self,
        document: &mut LynxDocument,
        field: NodeId,
        editor: NodeId,
        detail: &EventDetail,
    ) {
        let EventDetail::Input {
            input_type,
            data,
            is_composing,
        } = detail
        else {
            return;
        };
        let line_break = *input_type == "insertLineBreak";
        if !is_composing {
            let inserted = if line_break {
                1
            } else {
                data.as_deref().map_or(0, units)
            };
            self.sanitize(document, field, editor, inserted);
        }
        if line_break && self.kind == Kind::Input {
            self.queue_value(document, field, editor, "confirm");
            return;
        }
        if *is_composing && !flag(document, field, SEND_COMPOSING_INPUT) {
            return;
        }
        self.report_input(document, field, editor, *is_composing);
        if line_break && flag(document, field, CONFIRM_ENTER) {
            self.queue_value(document, field, editor, "confirm");
        }
    }

    /// Ends a field's editing state when it can no longer be edited.
    fn update_editable(document: &mut LynxDocument, element: NodeId) {
        let Some(editor) = editor(document, element) else {
            return;
        };
        let editable = !flag(document, element, DISABLED) && !flag(document, element, READONLY);
        let value = if editable { "plaintext-only" } else { "false" };
        if attribute(document, editor, "contenteditable") != Some(value) {
            document.set_attribute(editor, "contenteditable", value);
        }
    }

    /// `focus()`: focuses the editor, which restores the selection it kept
    /// since it lost focus or a script last wrote its value, as a browser's
    /// `input.focus()` does. A field never focused nor written has none, and
    /// gets a caret at the end of the value, where a browser's fresh
    /// `<input>` reports its selection.
    ///
    /// The Lynx `focus` is queued here, inside the call, as a browser fires
    /// `focus` inside `input.focus()`, so it goes ahead of a `selection` a
    /// `setSelectionRange` in the same job queues; the standard `focus` the
    /// runtime dispatches later finds the editor already reported and
    /// queues nothing more.
    fn focus(&self, document: &mut LynxDocument, field: NodeId, editor: NodeId) -> MethodOutcome {
        if document.focused() == Some(editor) {
            return MethodOutcome::Done;
        }
        let restores = document.host_selection(editor).is_some();
        if !document.focus(editor) {
            return MethodOutcome::Failed(MethodError::Operation);
        }
        if !restores {
            let end = units(&Self::value(document, editor));
            document.apply_editing(Command::SetSelection { range: end..end });
        }
        self.on_focus(document, field, editor);
        MethodOutcome::Done
    }

    /// The editor gained focus: its value and selection become the reported
    /// ones and the Lynx `focus` is queued, once per focus.
    fn on_focus(&self, document: &LynxDocument, field: NodeId, editor: NodeId) {
        let (value, selection, _) = Self::snapshot(document, editor);
        let mut reported = self.shared.reported.borrow_mut();
        if reported.as_ref().is_some_and(|last| last.editor == editor) {
            return;
        }
        *reported = Some(Reported {
            editor,
            value: value.clone(),
            selection,
        });
        self.shared
            .events
            .queue_detailed(field, "focus", ComponentDetail::Value { value });
    }

    /// A selection a method set: applied when the editor is focused, then
    /// reported as a `selection` would be.
    fn select_range(
        &self,
        document: &mut LynxDocument,
        field: NodeId,
        editor: NodeId,
        range: Range<usize>,
    ) {
        if document.focused() != Some(editor) {
            return;
        }
        document.apply_editing(Command::SetSelection { range });
        self.report_selection(document, field, editor);
    }
}

/// The params object, or `InvalidParams`.
fn params_object(params: &str) -> Result<serde_json::Map<String, Value>, InvalidParams> {
    match serde_json::from_str(params) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(Value::Null) => Ok(serde_json::Map::new()),
        _ => Err(InvalidParams),
    }
}

/// A non-negative whole number param, truncated; `None` when absent or not
/// a number.
fn index_param(params: &serde_json::Map<String, Value>, name: &str) -> Option<usize> {
    let number = params.get(name)?.as_f64()?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped at 0 and truncated, as a UTF-16 offset"
    )]
    Some(number.max(0.0) as usize)
}

fn string_param<'a>(params: &'a serde_json::Map<String, Value>, name: &str) -> Option<&'a str> {
    params.get(name)?.as_str()
}

impl CustomElement<()> for TextField {
    fn observed_attributes(&self) -> Vec<String> {
        [
            VALUE,
            PLACEHOLDER,
            TYPE,
            CONFIRM_TYPE,
            DISABLED,
            READONLY,
            INPUT_FILTER,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn observed_events(&self) -> &'static [&'static str] {
        &["input", "compositionend", "focus", "blur", SELECTION_CHANGE]
    }

    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        self.build(document, element);
    }

    /// web-core replays its `value` handler at connection, after the other
    /// attributes, so a `maxlength` written after `value` still cuts it.
    fn connected_callback(&self, document: &mut LynxDocument, element: NodeId) {
        if attribute(document, element, VALUE).is_some() {
            self.apply_value_attribute(document, element);
        }
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        let Some(editor) = editor(document, element) else {
            return;
        };
        match name {
            VALUE => self.apply_value_attribute(document, element),
            PLACEHOLDER => {
                let Some(block) = placeholder_block(document, element) else {
                    return;
                };
                let content = new.unwrap_or_default();
                let existing = document
                    .get(block)
                    .and_then(|node| node.first_child())
                    .map(dom::Node::id);
                if let Some(node) = existing {
                    document.set_text_node_data(node, content);
                } else {
                    let node = document.create_text_node(content, ());
                    document.append_child(block, node);
                }
            }
            TYPE => {
                // web-core's `_handleType`: the inner element is `password`
                // or `text`, and the kind picks its `inputmode`.
                let (kind, mode) = match new.unwrap_or_default() {
                    "digit" => ("text", "numeric"),
                    "number" => ("text", "decimal"),
                    "email" => ("text", "email"),
                    "tel" => ("text", "tel"),
                    "password" if self.kind == Kind::Input => ("password", "text"),
                    _ => ("text", "text"),
                };
                document.set_attribute(editor, TYPE, kind);
                document.set_attribute(editor, "inputmode", mode);
            }
            CONFIRM_TYPE => {
                document.set_attribute(editor, "enterkeyhint", new.unwrap_or("send"));
            }
            DISABLED | READONLY => Self::update_editable(document, element),
            INPUT_FILTER => match new {
                Some(pattern) => document.set_attribute(editor, PATTERN, pattern),
                None => document.remove_attribute(editor, PATTERN),
            },
            _ => {}
        }
    }

    /// The editor's standard events, heard at the shadow root: `input` and
    /// `compositionend` on their bubble pass, the non-bubbling `focus`,
    /// `blur` and `selectionchange` on the capture pass, which is the only
    /// one they have there.
    fn handle_event(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        event: &mut ElementEvent<'_>,
    ) {
        let Some(editor) = editor(document, element) else {
            return;
        };
        if event.target() != editor {
            return;
        }
        let name = event.type_name();
        let bubbles = matches!(name, "input" | "compositionend");
        let phase = event.phase();
        if (bubbles && phase != EventPhase::Bubbling)
            || (!bubbles && phase != EventPhase::Capturing)
        {
            return;
        }
        match name {
            "input" => self.on_input(document, element, editor, event.detail()),
            "compositionend" => {
                let inserted = match event.detail() {
                    EventDetail::Composition { data } => units(data),
                    _ => 0,
                };
                self.sanitize(document, element, editor, inserted);
                self.report_input(document, element, editor, false);
            }
            "focus" => self.on_focus(document, element, editor),
            "blur" => {
                self.queue_value(document, element, editor, "blur");
                let mut reported = self.shared.reported.borrow_mut();
                if reported.as_ref().is_some_and(|last| last.editor == editor) {
                    *reported = None;
                }
            }
            SELECTION_CHANGE => self.report_selection(document, element, editor),
            _ => {}
        }
    }

    fn invoke(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        call: MethodCall<'_>,
    ) -> MethodOutcome {
        let Some(editor) = editor(document, element) else {
            return MethodOutcome::NotFound;
        };
        let params = || params_object(call.params);
        let answer: Result<Option<String>, InvalidParams> = match call.name {
            "focus" => return self.focus(document, element, editor),
            "blur" => {
                if document.focused() == Some(editor) {
                    document.blur();
                }
                Ok(None)
            }
            "controlKeyBoard" => {
                // web-core's `controlKeyBoard`: 0 and 1 focus, 2 and 3 blur.
                match params().map(|params| params.get("action").and_then(Value::as_u64)) {
                    Ok(Some(0 | 1)) => {
                        return self.focus(document, element, editor);
                    }
                    Ok(Some(2 | 3)) => {
                        if document.focused() == Some(editor) {
                            document.blur();
                        }
                        Ok(None)
                    }
                    Ok(_) => Ok(None),
                    Err(error) => Err(error),
                }
            }
            "setValue" => params().and_then(|params| {
                let value = string_param(&params, VALUE)
                    .ok_or(InvalidParams)?
                    .to_owned();
                // web-core places the caret only for a truthy `index`; a
                // value setter otherwise leaves it at the end.
                let caret = index_param(&params, "index")
                    .filter(|&index| index > 0)
                    .unwrap_or(usize::MAX)
                    .min(units(&value));
                self.write_value(document, editor, &value, caret);
                Ok(None)
            }),
            "getValue" => {
                let (value, selection, composing) = Self::snapshot(document, editor);
                let mut record = String::new();
                write_record_field(&mut record, &value);
                write_record_field(&mut record, &selection.start.to_string());
                write_record_field(&mut record, &selection.end.to_string());
                write_record_field(&mut record, if composing { "1" } else { "0" });
                Ok(Some(record))
            }
            "setSelectionRange" => params().map(|params| {
                let length = units(&Self::value(document, editor));
                let start = index_param(&params, "selectionStart")
                    .unwrap_or(0)
                    .min(length);
                let end = index_param(&params, "selectionEnd")
                    .unwrap_or(start)
                    .min(length);
                self.select_range(document, element, editor, start..end.max(start));
                None
            }),
            "select" => {
                let length = units(&Self::value(document, editor));
                self.select_range(document, element, editor, 0..length);
                Ok(None)
            }
            "addText" => params().and_then(|params| {
                let text = string_param(&params, "text").ok_or(InvalidParams)?;
                let (value, selection, _) = Self::snapshot(document, editor);
                let at = byte_of_unit(&value, selection.start as usize);
                let next = format!("{}{text}{}", &value[..at], &value[at..]);
                let end = units(&next);
                self.write_value(document, editor, &next, end);
                Ok(None)
            }),
            "sendDelEvent" => params().map(|params| {
                let action = params.get("action").and_then(Value::as_f64);
                let length = if action == Some(1.0) {
                    1
                } else {
                    index_param(&params, "length").unwrap_or(0)
                };
                let (value, selection, _) = Self::snapshot(document, editor);
                let caret = selection.start as usize;
                let start = byte_of_unit(&value, caret.saturating_sub(length));
                let end = byte_of_unit(&value, caret);
                let (next, _) = remove_ranges(&value, std::slice::from_ref(&(start..end)), caret);
                let end = units(&next);
                self.write_value(document, editor, &next, end);
                None
            }),
            "setInputFilter" => params().and_then(|params| {
                let pattern = string_param(&params, "pattern").ok_or(InvalidParams)?;
                document.set_attribute(editor, PATTERN, pattern);
                Ok(None)
            }),
            _ => return MethodOutcome::NotFound,
        };
        match answer {
            Ok(None) => MethodOutcome::Done,
            Ok(Some(record)) => MethodOutcome::Data(record),
            Err(InvalidParams) => MethodOutcome::Failed(MethodError::InvalidParams),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem at 10px lays out exactly.

    use std::sync::Arc;

    use dom::editing::Command;
    use dom::event::EventDetail;
    use dom::layout::ParagraphKind;
    use dom::stylo::properties::PropertyId;
    use dom::{MethodCall, MethodError, MethodOutcome, NodeId, Point2D, StylesheetOrigin};

    use super::super::test_support::{style_of, with_component_events};
    use super::super::{
        ComponentDetail, ComponentEvent, ComponentEvents, LynxDocument, PageConfig,
    };
    use super::{
        INPUT_TAGS, SELECTION_CHANGE, TEXTAREA_TAGS, editor, focus_delegate, placeholder_block,
    };

    const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

    /// A field of `tag`, 200×40 with 5px padding, in 10px Ahem with a 10px
    /// line, laid out.
    fn make(tag: &str, attributes: &[(&str, &str)]) -> (LynxDocument, ComponentEvents, NodeId) {
        let (mut document, events) = with_component_events(PageConfig::default());
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let field = document.create_element(tag, ());
        for (name, value) in attributes {
            document.set_attribute(field, name, value);
        }
        document.set_inline_style(
            field,
            "width: 200px; height: 40px; padding: 5px; font-family: Ahem; font-size: 10px; \
             line-height: 10px",
        );
        let page = document.document_element().id();
        document.append_child(page, field);
        document.layout();
        (document, events, field)
    }

    fn value_of(document: &LynxDocument, element: NodeId, property: &str) -> String {
        let id = PropertyId::parse_enabled_for_all_content(property)
            .unwrap_or_else(|()| panic!("unknown property `{property}`"));
        let declaration = id
            .as_shorthand()
            .err()
            .unwrap_or_else(|| panic!("`{property}` is a shorthand"));
        style_of(document, element).computed_value_to_string(declaration)
    }

    fn rect(document: &LynxDocument, element: NodeId) -> (f32, f32, f32, f32) {
        let rect = document
            .bounding_client_rect(element)
            .expect("a rendered element");
        (
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        )
    }

    fn editor_of(document: &LynxDocument, field: NodeId) -> NodeId {
        editor(document, field).expect("every field has its editor")
    }

    fn placeholder_box(document: &LynxDocument, field: NodeId) -> NodeId {
        let block = placeholder_block(document, field).expect("the placeholder's block");
        document.get(block).and_then(dom::Node::parent_id).unwrap()
    }

    /// The events queued since the last call, as `(name, detail)`.
    fn queued(events: &ComponentEvents) -> Vec<(&'static str, ComponentDetail)> {
        events
            .take()
            .into_iter()
            .map(|event| match event {
                ComponentEvent::Detailed { name, detail, .. } => (name, detail),
                other => panic!("no such event here: {other:?}"),
            })
            .collect()
    }

    fn input(value: &str, selection: std::ops::Range<u32>, is_composing: bool) -> ComponentDetail {
        ComponentDetail::Input {
            value: value.to_owned(),
            selection,
            is_composing,
        }
    }

    fn text_value(value: &str) -> ComponentDetail {
        ComponentDetail::Value {
            value: value.to_owned(),
        }
    }

    /// What the runtime does for one command at the focused editor: the
    /// edit, then the events after the change through the Rust walk —
    /// non-composed, as an editor in a shadow tree dispatches them — then
    /// the engine's `selectionchange`.
    fn edit(document: &mut LynxDocument, command: Command) {
        let host = document.focused().expect("a focused editor");
        let outcome = document.apply_editing(command);
        for event in &outcome.events {
            let detail = match event.kind {
                dom::editing::EditEventKind::BeforeInput | dom::editing::EditEventKind::Input => {
                    EventDetail::Input {
                        input_type: event.input_type,
                        data: event.data.as_deref().map(Arc::from),
                        is_composing: event.is_composing,
                    }
                }
                _ => EventDetail::Composition {
                    data: Arc::from(event.data.as_deref().unwrap_or_default()),
                },
            };
            document.dispatch_element_event(host, event.kind.name(), &detail, true, false, 0.0);
        }
        if outcome.applied {
            document.dispatch_element_event(
                host,
                SELECTION_CHANGE,
                &EventDetail::None,
                false,
                false,
                0.0,
            );
        }
    }

    /// Focus moves the way the runtime moves it: `dom` queues the events and
    /// they are dispatched, non-composed, at their targets.
    fn deliver_focus_events(document: &mut LynxDocument) {
        for event in document.take_focus_events() {
            document.dispatch_element_event(
                event.target,
                event.kind.name(),
                &EventDetail::Focus {
                    related_target: event.related_target,
                },
                event.kind.bubbles(),
                false,
                0.0,
            );
        }
    }

    fn focus(document: &mut LynxDocument, field: NodeId) {
        let editor = editor_of(document, field);
        assert!(document.focus(editor));
        deliver_focus_events(document);
    }

    fn invoke(
        document: &mut LynxDocument,
        field: NodeId,
        name: &str,
        params: &str,
    ) -> MethodOutcome {
        document.invoke_element_method(field, MethodCall { name, params })
    }

    fn value(document: &LynxDocument, field: NodeId) -> String {
        super::TextField::value(document, editor_of(document, field))
    }

    // --- structure and style -------------------------------------------------

    #[test]
    fn every_tag_builds_an_editor_and_a_placeholder() {
        assert_eq!(
            INPUT_TAGS.iter().chain(&TEXTAREA_TAGS).count(),
            4,
            "the four Lynx tags"
        );
        for tag in INPUT_TAGS.iter().chain(&TEXTAREA_TAGS) {
            let (document, _, field) = make(tag, &[]);
            let editor = editor_of(&document, field);
            assert!(document.is_editing_host(editor), "{tag}");
            let id = if INPUT_TAGS.contains(tag) {
                "input"
            } else {
                "textarea"
            };
            assert_eq!(document.get(editor).unwrap().attribute("part"), Some(id));
            assert_eq!(value_of(&document, field, "font-size"), "10px", "{tag}");
        }
    }

    /// `input` and `textarea` are HTML names: no field is built under them
    /// (user ruling 2026-10-19).
    #[test]
    fn the_html_names_are_not_text_fields() {
        for tag in ["input", "textarea"] {
            let (document, _, element) = make(tag, &[("placeholder", "p")]);
            assert_eq!(document.shadow_root(element), None, "{tag}");
            assert_eq!(focus_delegate(&document, element), None, "{tag}");
        }
    }

    /// `x-input`'s editor is a `div` laid out in its own standard inline
    /// formatting context, with no Lynx text semantics; `x-textarea`'s is a
    /// Lynx `<text>` whose Lynx paragraph lays its lines out and which wears
    /// the field's colour.
    #[test]
    fn the_input_edits_an_inline_formatting_context_and_the_textarea_a_lynx_text() {
        let (document, _, input) = make("x-input", &[("value", "ab")]);
        let editor = editor_of(&document, input);
        assert_eq!(document.get(editor).unwrap().tag_name(), Some("div"));
        assert_eq!(value_of(&document, editor, "display"), "block");
        assert_eq!(
            document.paragraph_kind(editor),
            Some(ParagraphKind::InlineFormattingContext)
        );

        let (mut document, _, textarea) = make("x-textarea", &[("value", "ab")]);
        document.set_inline_style_property(textarea, "color", "rgb(1, 2, 3)");
        document.layout();
        let editor = editor_of(&document, textarea);
        assert_eq!(document.get(editor).unwrap().tag_name(), Some("text"));
        assert_eq!(document.paragraph_kind(editor), Some(ParagraphKind::Lynx));
        assert_eq!(value_of(&document, editor, "color"), "rgb(1, 2, 3)");
        // The Lynx paragraph shows the empty line after a trailing break.
        document.set_attribute(textarea, "value", "ab\n");
        document.layout();
        assert_eq!(document.paragraph_line_count(editor), Some(2));
    }

    /// The single-line editor keeps every character and does not wrap; the
    /// textarea's wraps. Both fill the content box. An empty editor is one
    /// line tall because its paragraph lays out one empty line, a layout
    /// rule, not CSS.
    #[test]
    fn the_input_is_one_line_and_the_textarea_wraps() {
        let (mut document, _, input) = make("x-input", &[("value", "a  b")]);
        let editor = editor_of(&document, input);
        assert_eq!(
            value_of(&document, editor, "white-space-collapse"),
            "preserve"
        );
        assert_eq!(value_of(&document, editor, "text-wrap-mode"), "nowrap");
        // Centred on the 30px content box: one 10px line at y = 5 + 10.
        assert_eq!(rect(&document, editor), (5.0, 15.0, 190.0, 10.0));
        document.set_attribute(input, "value", &"x".repeat(40));
        document.layout();
        assert_eq!(
            rect(&document, editor).3,
            10.0,
            "40 squares stay on one line"
        );

        let (mut document, _, textarea) = make("x-textarea", &[]);
        let editor = editor_of(&document, textarea);
        assert_eq!(value_of(&document, editor, "text-wrap-mode"), "wrap");
        assert_eq!(rect(&document, editor), (5.0, 5.0, 190.0, 30.0));
        // Unsized, the textarea grows with its wrapped lines: 30 squares
        // and spaces on 190px wrap into four 10px lines.
        document.set_inline_style(
            textarea,
            "width: 200px; padding: 5px; font-family: Ahem; font-size: 10px; line-height: 10px",
        );
        document.set_attribute(textarea, "value", &"xx ".repeat(20));
        document.layout();
        assert_eq!(rect(&document, editor).3, 40.0);
    }

    /// The placeholder shows only while the value is empty, in grey unless
    /// `placeholder-color` says otherwise, over the content box.
    #[test]
    fn the_placeholder_shows_while_the_value_is_empty() {
        let (mut document, _, field) = make(
            "x-input",
            &[("placeholder", "hint"), ("placeholder-color", "red")],
        );
        let placeholder = placeholder_box(&document, field);
        let block = placeholder_block(&document, field).unwrap();
        assert_eq!(value_of(&document, placeholder, "display"), "flex");
        assert_eq!(value_of(&document, placeholder, "color"), "rgb(255, 0, 0)");
        assert_eq!(rect(&document, placeholder), (0.0, 0.0, 200.0, 40.0));
        assert_eq!(rect(&document, block), (5.0, 15.0, 190.0, 10.0));
        let shown = document.get(block).unwrap().first_child().unwrap().text();
        assert_eq!(shown, Some("hint"));

        document.set_attribute(field, "value", "x");
        document.layout();
        assert_eq!(value_of(&document, placeholder, "display"), "none");
        document.set_attribute(field, "value", "");
        document.layout();
        assert_eq!(value_of(&document, placeholder, "display"), "flex");

        document.remove_attribute(field, "placeholder-color");
        document.set_attribute(field, "placeholder-font-size", "20px");
        document.set_attribute(field, "placeholder-font-weight", "bold");
        document.layout();
        assert_eq!(
            value_of(&document, placeholder, "color"),
            "rgb(128, 128, 128)"
        );
        assert_eq!(value_of(&document, placeholder, "font-size"), "20px");
        assert_eq!(value_of(&document, placeholder, "font-weight"), "700");
        document.remove_attribute(field, "placeholder-font-size");
        document.layout();
        assert_eq!(
            value_of(&document, placeholder, "font-size"),
            "10px",
            "inherited"
        );
    }

    /// The placeholder's block is the field's `::placeholder`: an author's
    /// `::placeholder` rule styles it, over the `placeholder-*` attributes'
    /// defaults, and follows the host's class; what css-pseudo-4 keeps off
    /// `::placeholder` (here `padding`) is not applied; the box over the
    /// content box keeps the defaults; the `::placeholder` rules of the UA
    /// sheet still lay the block out.
    #[test]
    fn a_placeholder_rule_styles_the_placeholder_text() {
        for tag in INPUT_TAGS.iter().chain(&TEXTAREA_TAGS) {
            let (mut document, _, field) = make(
                tag,
                &[
                    ("placeholder", "hint"),
                    ("placeholder-color", "red"),
                    ("class", "hinted"),
                ],
            );
            document.add_stylesheet(
                ".hinted::placeholder { color: rgb(0, 0, 255); font-weight: bold; \
                 font-size: 20px; padding-left: 7px; }",
                StylesheetOrigin::Author,
            );
            document.layout();
            let placeholder = placeholder_box(&document, field);
            let block = placeholder_block(&document, field).unwrap();
            assert_eq!(
                document.get(block).unwrap().attribute("pseudo"),
                Some(dom::PLACEHOLDER_PSEUDO_ID),
                "{tag}"
            );
            assert_eq!(
                value_of(&document, block, "color"),
                "rgb(0, 0, 255)",
                "{tag}"
            );
            assert_eq!(value_of(&document, block, "font-weight"), "700", "{tag}");
            assert_eq!(value_of(&document, block, "font-size"), "20px", "{tag}");
            assert_eq!(value_of(&document, block, "padding-left"), "0px", "{tag}");
            assert_eq!(value_of(&document, block, "display"), "block", "{tag}");
            assert_eq!(
                rect(&document, block).2,
                190.0,
                "{tag}: the block fills the row"
            );
            assert_eq!(
                value_of(&document, placeholder, "color"),
                "rgb(255, 0, 0)",
                "{tag}"
            );
            assert_eq!(
                value_of(&document, placeholder, "font-size"),
                "10px",
                "{tag}"
            );

            document.remove_attribute(field, "class");
            document.layout();
            assert_eq!(
                value_of(&document, block, "color"),
                "rgb(255, 0, 0)",
                "{tag}"
            );
            assert_eq!(value_of(&document, block, "font-size"), "10px", "{tag}");
        }
    }

    // --- attributes ----------------------------------------------------------

    #[test]
    fn type_and_confirm_type_reach_the_text_input_state() {
        let (mut document, _, field) = make("x-input", &[("type", "password")]);
        focus(&mut document, field);
        let state = document.text_input_state();
        assert_eq!(state.input_type, dom::editing::InputType::Password);
        assert_eq!(state.enter_key_hint, dom::editing::EnterKeyHint::Send);
        for (kind, mode) in [
            ("number", dom::editing::InputMode::Decimal),
            ("digit", dom::editing::InputMode::Numeric),
            ("email", dom::editing::InputMode::Email),
            ("tel", dom::editing::InputMode::Tel),
            ("text", dom::editing::InputMode::Text),
        ] {
            document.set_attribute(field, "type", kind);
            let state = document.text_input_state();
            assert_eq!(state.input_mode, mode, "{kind}");
            assert_eq!(state.input_type, dom::editing::InputType::Text, "{kind}");
        }
        document.set_attribute(field, "confirm-type", "search");
        assert_eq!(
            document.text_input_state().enter_key_hint,
            dom::editing::EnterKeyHint::Search
        );
    }

    /// `type=password` masks the input's editor with a disc per character,
    /// through the shadow sheet; any other type, and a textarea, does not.
    #[test]
    fn a_password_input_masks_its_value_and_a_text_one_does_not() {
        // A tab is the tell: shown, it runs to the stop at 80px under
        // `white-space: pre`; masked, it is one 10px Ahem disc.
        let (mut document, _, field) = make("x-input", &[("type", "password"), ("value", "a\tb")]);
        let editor = editor_of(&document, field);
        assert_eq!(value_of(&document, editor, "-webkit-text-security"), "disc");
        assert_eq!(value(&document, field), "a\tb");
        let width = |document: &LynxDocument| {
            let boxes = document
                .text_range_boxes(editor, 0, 3)
                .expect("a committed paragraph");
            boxes.iter().map(|rect| rect.size.width).sum::<f32>()
        };
        assert_eq!(width(&document), 30.0);

        document.set_attribute(field, "type", "text");
        document.layout();
        assert_eq!(value_of(&document, editor, "-webkit-text-security"), "none");
        assert_eq!(width(&document), 90.0);

        let (document, _, field) = make("x-textarea", &[("type", "password")]);
        let editor = editor_of(&document, field);
        assert_eq!(value_of(&document, editor, "-webkit-text-security"), "none");
    }

    /// `value` is cut to `maxlength` only when that attribute is present,
    /// and a later `maxlength` does not cut what is shown.
    #[test]
    fn the_value_attribute_is_cut_to_a_present_maxlength() {
        let (mut document, _, field) =
            make("x-input", &[("maxlength", "5"), ("value", "hello world")]);
        assert_eq!(value(&document, field), "hello");
        document.set_attribute(field, "maxlength", "2");
        assert_eq!(value(&document, field), "hello");
        document.set_attribute(field, "value", "12345678");
        assert_eq!(value(&document, field), "12");
        document.remove_attribute(field, "maxlength");
        document.set_attribute(field, "value", &"y".repeat(200));
        assert_eq!(
            value(&document, field).len(),
            200,
            "the default limit cuts no value"
        );
    }

    #[test]
    fn disabled_and_readonly_turn_editing_off() {
        for attribute in ["disabled", "readonly"] {
            let (mut document, _, field) = make("x-input", &[]);
            let editor = editor_of(&document, field);
            focus(&mut document, field);
            document.set_attribute(field, attribute, "");
            assert!(!document.is_editing_host(editor), "{attribute}");
            assert_eq!(document.focused(), None, "{attribute}: focus fixed up");
            assert!(!document.focus(editor));
            assert_eq!(focus_delegate(&document, field), None);
            document.set_attribute(field, attribute, "false");
            assert!(document.is_editing_host(editor), "{attribute}=false");
            assert_eq!(focus_delegate(&document, field), Some(editor));
        }
    }

    /// A press anywhere in the field — its own box, its placeholder — is
    /// delegated to the editor.
    #[test]
    fn a_press_in_the_field_delegates_focus_to_the_editor() {
        let (mut document, _, field) = make("x-input", &[("placeholder", "p")]);
        let editor = editor_of(&document, field);
        assert_eq!(focus_delegate(&document, field), Some(editor));
        let placeholder = placeholder_block(&document, field).unwrap();
        assert_eq!(focus_delegate(&document, placeholder), Some(editor));
        let page = document.document_element().id();
        assert_eq!(focus_delegate(&document, page), None);
        document.commit();
        // The placeholder takes no hit: the top hit in the padding is the
        // host itself.
        let hits = document.elements_from_point(Point2D::new(2.0, 2.0));
        assert_eq!(hits.first(), Some(&field));
    }

    // --- events --------------------------------------------------------------

    #[test]
    fn typing_fires_input_and_focus_and_blur_carry_the_value() {
        let (mut document, events, field) = make("x-input", &[("value", "ab")]);
        focus(&mut document, field);
        assert_eq!(queued(&events), [("focus", text_value("ab"))]);
        edit(&mut document, Command::SetSelection { range: 2..2 });
        edit(&mut document, Command::Insert("c".to_owned()));
        assert_eq!(queued(&events), [("input", input("abc", 3..3, false))]);
        document.blur();
        deliver_focus_events(&mut document);
        assert_eq!(queued(&events), [("blur", text_value("abc"))]);
    }

    /// Enter in an `x-input` confirms and leaves the value as it was; the
    /// line breaks of a paste are stripped.
    #[test]
    fn enter_confirms_an_input_and_line_breaks_are_stripped() {
        let (mut document, events, field) = make("x-input", &[("value", "hi")]);
        focus(&mut document, field);
        edit(&mut document, Command::SetSelection { range: 2..2 });
        queued(&events);
        edit(&mut document, Command::InsertLineBreak);
        assert_eq!(queued(&events), [("confirm", text_value("hi"))]);
        assert_eq!(value(&document, field), "hi");
        edit(&mut document, Command::Insert("a\nb\r".to_owned()));
        assert_eq!(queued(&events), [("input", input("hiab", 4..4, false))]);
    }

    /// A textarea keeps its line breaks and confirms only with
    /// `confirm-enter`.
    #[test]
    fn a_textarea_confirms_on_enter_only_with_confirm_enter() {
        let (mut document, events, field) = make("x-textarea", &[]);
        focus(&mut document, field);
        queued(&events);
        edit(&mut document, Command::InsertLineBreak);
        assert_eq!(queued(&events), [("input", input("\n", 1..1, false))]);
        document.set_attribute(field, "confirm-enter", "");
        edit(&mut document, Command::InsertLineBreak);
        assert_eq!(
            queued(&events),
            [
                ("input", input("\n\n", 2..2, false)),
                ("confirm", text_value("\n\n"))
            ]
        );
    }

    /// The browser's `maxlength`: what is inserted past the limit is taken
    /// back from the end of the insertion, the caret with it, and an edit
    /// that changes nothing fires nothing. 140 when absent.
    #[test]
    fn maxlength_takes_back_what_an_insertion_put_past_the_limit() {
        let (mut document, events, field) = make("x-input", &[("maxlength", "4"), ("value", "ab")]);
        focus(&mut document, field);
        edit(&mut document, Command::SetSelection { range: 1..1 });
        queued(&events);
        edit(&mut document, Command::Insert("xyz".to_owned()));
        assert_eq!(queued(&events), [("input", input("axyb", 3..3, false))]);
        edit(&mut document, Command::Insert("q".to_owned()));
        assert_eq!(value(&document, field), "axyb");
        assert!(queued(&events).is_empty(), "nothing changed");
        // A surrogate pair is never split: both halves go.
        edit(&mut document, Command::DeleteBackward);
        edit(&mut document, Command::Insert("😀".to_owned()));
        assert_eq!(value(&document, field), "axb");

        let (mut document, _, field) = make("x-input", &[]);
        focus(&mut document, field);
        edit(&mut document, Command::Insert("z".repeat(150)));
        assert_eq!(value(&document, field).len(), 140);
    }

    /// `maxlines`: an insertion that would lay the value out past the limit
    /// keeps its longest prefix that fits, the caret after it; a value
    /// within the limit is kept whole, and a deletion never cuts one already
    /// over it.
    #[test]
    fn maxlines_takes_back_what_an_insertion_put_past_the_limit() {
        let (mut document, events, field) =
            make("x-textarea", &[("maxlines", "2"), ("value", "one")]);
        focus(&mut document, field);
        edit(&mut document, Command::SetSelection { range: 3..3 });
        queued(&events);
        edit(&mut document, Command::InsertLineBreak);
        edit(&mut document, Command::Insert("two".to_owned()));
        assert_eq!(value(&document, field), "one\ntwo", "two lines are kept");
        assert_eq!(
            queued(&events),
            [
                ("input", input("one\n", 4..4, false)),
                ("input", input("one\ntwo", 7..7, false))
            ]
        );
        // A third line, from a break: refused whole, and no `input`.
        edit(&mut document, Command::InsertLineBreak);
        assert_eq!(value(&document, field), "one\ntwo");
        assert_eq!(document.text_input_state().selection, 7..7);
        assert!(queued(&events).is_empty(), "nothing changed");
        // A paste whose third line starts at its break keeps what is before.
        edit(&mut document, Command::Insert("s\nthree".to_owned()));
        assert_eq!(value(&document, field), "one\ntwos");
        assert_eq!(
            queued(&events),
            [("input", input("one\ntwos", 8..8, false))]
        );
        // Wrapping counts: 19 squares fill the 190px line, so what follows
        // the 15th `x` starts a third line. Never half a surrogate pair.
        edit(
            &mut document,
            Command::Insert(format!("{}😀x", "x".repeat(15))),
        );
        let kept = value(&document, field);
        assert_eq!(kept.lines().nth(1).unwrap().encode_utf16().count(), 19);
        assert!(kept.ends_with('x'), "no emoji half: {kept:?}");
        document.layout();
        assert_eq!(
            document.paragraph_line_count(editor_of(&document, field)),
            Some(2)
        );

        // A value over the limit, written by script, is not cut by a
        // deletion, and the field without `maxlines` takes every line.
        let (mut document, _, field) =
            make("x-textarea", &[("maxlines", "2"), ("value", "a\nb\nc")]);
        focus(&mut document, field);
        edit(&mut document, Command::SetSelection { range: 5..5 });
        edit(&mut document, Command::DeleteBackward);
        assert_eq!(value(&document, field), "a\nb\n");
        document.remove_attribute(field, "maxlines");
        edit(&mut document, Command::Insert("d\ne".to_owned()));
        assert_eq!(value(&document, field), "a\nb\nd\ne");
    }

    #[test]
    fn number_and_input_filter_remove_what_they_reject() {
        let (mut document, events, field) = make("x-input", &[("type", "number")]);
        focus(&mut document, field);
        queued(&events);
        for text in ["1", ".", "2", "a", "."] {
            edit(&mut document, Command::Insert(text.to_owned()));
        }
        // The last dot is kept, as web-core's `\.(?=.*\.)` keeps it.
        assert_eq!(value(&document, field), "12.");
        assert_eq!(document.text_input_state().selection, 3..3);

        let (mut document, events, field) =
            make("x-textarea", &[("input-filter", "[^0-9\\u4e00-\\u9fa5]")]);
        focus(&mut document, field);
        queued(&events);
        edit(&mut document, Command::Insert("a1中b2".to_owned()));
        assert_eq!(queued(&events), [("input", input("1中2", 3..3, false))]);
        // `setInputFilter` replaces the pattern.
        assert_eq!(
            invoke(&mut document, field, "setInputFilter", r#"{"pattern":"x"}"#),
            MethodOutcome::Done
        );
        edit(&mut document, Command::Insert("xyx".to_owned()));
        assert_eq!(value(&document, field), "1中2y");
    }

    /// During a composition `input` waits for `compositionend`, unless
    /// `send-composing-input` asks for each update.
    #[test]
    fn composing_input_waits_for_the_end_unless_asked() {
        let (mut document, events, field) = make("x-input", &[]);
        focus(&mut document, field);
        queued(&events);
        let compose = |text: &str| Command::SetComposition {
            text: text.to_owned(),
            selection: 1..1,
        };
        edit(&mut document, compose("n"));
        edit(&mut document, compose("ni"));
        assert!(queued(&events).is_empty());
        edit(
            &mut document,
            Command::CommitText {
                text: "你".to_owned(),
                relative_cursor: 0,
            },
        );
        assert_eq!(queued(&events), [("input", input("你", 1..1, false))]);

        document.set_attribute(field, "send-composing-input", "");
        edit(&mut document, compose("h"));
        assert_eq!(queued(&events), [("input", input("你h", 2..2, true))]);
    }

    /// `selection` fires for a selection that moved with no change of value
    /// and leaves text selected.
    #[test]
    fn selection_fires_for_a_selected_range() {
        let (mut document, events, field) = make("x-input", &[("value", "hello")]);
        focus(&mut document, field);
        queued(&events);
        edit(&mut document, Command::SetSelection { range: 1..1 });
        assert!(queued(&events).is_empty(), "a caret move selects nothing");
        edit(&mut document, Command::SetSelection { range: 1..3 });
        assert_eq!(
            queued(&events),
            [("selection", ComponentDetail::Selection { selection: 1..3 })]
        );
        assert_eq!(
            invoke(&mut document, field, "select", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(
            queued(&events),
            [("selection", ComponentDetail::Selection { selection: 0..5 })]
        );
    }

    /// `x-textarea`'s `min-height` and `max-height` attributes clamp the
    /// host's box through UA `attr()` rules; removing one, or a value that
    /// is no length, leaves the content height, and `x-input` reads neither.
    #[test]
    fn min_height_and_max_height_attributes_clamp_a_textarea() {
        const UNSIZED: &str =
            "width: 200px; padding: 5px; font-family: Ahem; font-size: 10px; line-height: 10px";
        // The initial values, as a field with neither attribute computes
        // them.
        let initial = |tag: &str| {
            let (mut document, _, field) = make(tag, &[]);
            document.set_inline_style(field, UNSIZED);
            document.layout();
            (
                value_of(&document, field, "min-height"),
                value_of(&document, field, "max-height"),
            )
        };
        for tag in TEXTAREA_TAGS {
            let (min, max) = initial(tag);
            let (mut document, _, field) = make(tag, &[("min-height", "60px")]);
            document.set_inline_style(field, UNSIZED);
            document.layout();
            let editor = editor_of(&document, field);
            assert_eq!(value_of(&document, field, "min-height"), "60px", "{tag}");
            assert_eq!(rect(&document, field).3, 60.0, "{tag}");
            assert_eq!(rect(&document, editor), (5.0, 5.0, 190.0, 50.0), "{tag}");

            // One line of content: 10px and the padding.
            document.set_attribute(field, "value", "x");
            document.remove_attribute(field, "min-height");
            document.layout();
            assert_eq!(value_of(&document, field, "min-height"), min, "{tag}");
            assert_eq!(rect(&document, field).3, 20.0, "{tag}");

            // Four wrapped lines would be 50px tall.
            document.set_attribute(field, "value", &"xx ".repeat(20));
            document.set_attribute(field, "max-height", "30px");
            document.layout();
            assert_eq!(value_of(&document, field, "max-height"), "30px", "{tag}");
            assert_eq!(rect(&document, field).3, 30.0, "{tag}");
            document.remove_attribute(field, "max-height");
            document.layout();
            assert_eq!(value_of(&document, field, "max-height"), max, "{tag}");
            assert_eq!(rect(&document, field).3, 50.0, "{tag}");

            // A value that is no length takes the fallback: what the field
            // computes without the attribute.
            document.set_attribute(field, "min-height", "tall");
            document.set_attribute(field, "max-height", "30");
            document.layout();
            assert_eq!(value_of(&document, field, "min-height"), min, "{tag}");
            assert_eq!(value_of(&document, field, "max-height"), max, "{tag}");
            assert_eq!(rect(&document, field).3, 50.0, "{tag}");
        }

        for tag in INPUT_TAGS {
            let (min, max) = initial(tag);
            let (mut document, _, field) =
                make(tag, &[("min-height", "60px"), ("max-height", "5px")]);
            document.set_inline_style(field, UNSIZED);
            document.layout();
            assert_eq!(value_of(&document, field, "min-height"), min, "{tag}");
            assert_eq!(value_of(&document, field, "max-height"), max, "{tag}");
        }
    }

    // --- methods -------------------------------------------------------------

    #[test]
    fn get_value_set_value_and_set_selection_range() {
        let (mut document, events, field) = make("x-input", &[("value", "hello")]);
        // Not focused: the caret is at the end.
        assert_eq!(
            invoke(&mut document, field, "getValue", "{}"),
            MethodOutcome::Data("5:hello1:51:51:0".to_owned())
        );
        assert_eq!(
            invoke(&mut document, field, "focus", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(document.focused(), Some(editor_of(&document, field)));
        assert_eq!(document.text_input_state().selection, 5..5);
        assert_eq!(
            invoke(
                &mut document,
                field,
                "setSelectionRange",
                r#"{"selectionStart":2,"selectionEnd":4}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(&mut document, field, "getValue", "{}"),
            MethodOutcome::Data("5:hello1:21:41:0".to_owned())
        );
        assert_eq!(
            invoke(
                &mut document,
                field,
                "setValue",
                r#"{"value":"add-text","index":3}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(&mut document, field, "addText", r#"{"text":"3"}"#),
            MethodOutcome::Done
        );
        assert_eq!(value(&document, field), "add3-text");
        assert_eq!(
            invoke(
                &mut document,
                field,
                "sendDelEvent",
                r#"{"action":0,"length":2}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(value(&document, field), "add3-te");
        assert_eq!(
            invoke(&mut document, field, "setValue", "{}"),
            MethodOutcome::Failed(MethodError::InvalidParams)
        );
        assert_eq!(
            invoke(&mut document, field, "blur", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(document.focused(), None);
        assert!(document.has_focus_events());
        // Script's own writes fire no `input`; the focus events are the
        // runtime's to deliver.
        deliver_focus_events(&mut document);
        let names: Vec<_> = queued(&events).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["focus", "selection", "blur"]);
        assert_eq!(
            invoke(&mut document, field, "scrollToPosition", "{}"),
            MethodOutcome::NotFound
        );
    }

    /// A field keeps its selection while it is not focused, as a browser's
    /// `<input>` keeps `selectionStart`/`selectionEnd` across a blur:
    /// `getValue` answers it, and `focus` restores it with no `selection`
    /// event, since the `focus` report carries it. A script's write while
    /// the field is not focused leaves its caret for the next focus.
    #[test]
    fn a_field_keeps_its_selection_while_it_is_not_focused() {
        let (mut document, events, field) = make("x-input", &[("value", "hello")]);
        let editor = editor_of(&document, field);
        focus(&mut document, field);
        assert_eq!(
            invoke(
                &mut document,
                field,
                "setSelectionRange",
                r#"{"selectionStart":1,"selectionEnd":3}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(&mut document, field, "blur", "{}"),
            MethodOutcome::Done
        );
        deliver_focus_events(&mut document);
        queued(&events);
        assert_eq!(document.focused(), None);
        assert_eq!(
            invoke(&mut document, field, "getValue", "{}"),
            MethodOutcome::Data("5:hello1:11:31:0".to_owned())
        );

        assert_eq!(
            invoke(&mut document, field, "focus", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(document.text_input_state().selection, 1..3);
        deliver_focus_events(&mut document);
        assert_eq!(
            queued(&events),
            [("focus", text_value("hello"))],
            "no `selection`: the focus report carries it"
        );

        // `setValue` while not focused: the caret at the end, or at `index`.
        assert_eq!(
            invoke(&mut document, field, "blur", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(&mut document, field, "setValue", r#"{"value":"abcdef"}"#),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(&mut document, field, "getValue", "{}"),
            MethodOutcome::Data("6:abcdef1:61:61:0".to_owned())
        );
        assert_eq!(
            invoke(&mut document, field, "focus", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(document.text_input_state().selection, 6..6);
        assert_eq!(
            invoke(&mut document, field, "blur", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(
                &mut document,
                field,
                "setValue",
                r#"{"value":"abcdef","index":2}"#
            ),
            MethodOutcome::Done
        );
        assert_eq!(
            invoke(&mut document, field, "focus", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(document.text_input_state().selection, 2..2);
        assert_eq!(document.focused(), Some(editor));

        // A fresh field has kept nothing: `focus` puts the caret at the end.
        let (mut document, _, field) = make("x-textarea", &[]);
        let editor = editor_of(&document, field);
        assert_eq!(document.host_selection(editor), None);
        assert_eq!(
            invoke(&mut document, field, "getValue", "{}"),
            MethodOutcome::Data("0:1:01:01:0".to_owned())
        );
        assert_eq!(
            invoke(&mut document, field, "focus", "{}"),
            MethodOutcome::Done
        );
        assert_eq!(document.text_input_state().selection, 0..0);
    }
}
