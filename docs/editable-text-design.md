# Editable text: `contenteditable` in the engine, IME bridged per host

Status: design ruled 2026-10-14; implementation in progress (series E1–E5 below).

## Ruling

Editable text is laid out, painted and edited **inside the engine** (route B,
the Chromium/WebKit/Clay/Flutter model). The host platform contributes only
what an engine cannot: the soft keyboard, the input method (composition,
candidates, autocorrect), the clipboard, and on mobile the system selection
widgets. The alternative, an OS-native text field composited above the vello
surface, was surveyed and rejected: the committed frame cannot produce the
"everything painted above this element" region such a view would need to mask
itself with, and every project that tried it (Flutter, Lynx native, Compose
Multiplatform) carries a fallback path for exactly that case.

Other rulings that shape the design:

- `dom` follows the **browser standard**: HTML `contenteditable`, the
  Selection/caret model, `beforeinput`/`input` (`InputEvent.inputType`,
  `data`, `isComposing`), `compositionstart`/`compositionupdate`/
  `compositionend`, `focus`/`blur`. Lynx's `x-input`/`x-textarea` and their
  `lynxinput`-shaped events are a layer *over* this, as web-core wraps a real
  `<input>`.
- The engine-side command set is Chromium's (`ImeSetComposition`,
  `ImeCommitText`, `ImeFinishComposingText`, `SetCompositionFromExistingText`,
  `DeleteSurroundingText`, `ExtendSelectionAndDelete`,
  `SetEditableSelectionOffsets`) and the published state is Chromium's
  `TextInputState` (type, mode, action, whole value, selection, composition,
  caret and composition character bounds). Both are proven on five platforms.
- Value authority races (a stale script write overtaking a keystroke) are
  **not managed**: last write wins.
- **No event is cancelable from script** (user ruling 2026-10-19, keeping the
  2026-08-17 one): `beforeinput` and the other editing events are dispatched
  not cancelable, an edit always applies, and the `keydown` and pointer
  editing default actions run whatever a listener does. A Lynx event object
  has no `preventDefault`, and web-core's root listeners are
  `passive: true`. The embedder's `InputEvent::default_prevented` is the only
  veto; a Rust component that ever needs one sets a `canceled` flag on `dom`'s
  `EventWalk` beside its stop flag (`docs/tracking/deviations.md`).
- Pointer events over an editing host are routed exactly like any other
  element's; the engine, not the host, turns them into caret placement.
- Platform-specific text features that need a real native control
  (iOS autocorrect bubbles, inline prediction, strong-password autofill) are
  out of scope for this series; the hidden-native-responder pattern (makepad,
  Compose Multiplatform 1.11) is the recorded follow-up for iOS/Android.

## Why the host can answer the OS synchronously

Chromium waits up to 1.5 s for `firstRectForCharacterRange:`; WebKit waits 1 s
for the autocorrection context; Chromium Android blocks a dedicated IME thread
on `getTextBeforeCursor`. All three exist because the editor lives in another
process. Here the editor (bobcat-main) and the host thread share a process
and already share `Arc<CommittedFrame>`, so the host receives a complete
`TextInputState` on every change through the view's notice queue and answers
every synchronous OS query from it with no wait.

## Architecture

```
          bobcat-main (Document, realm)                 host thread (Painter, LynxView::pump)
 ┌─────────────────────────────────────────┐    ┌──────────────────────────────────────────┐
 │ dom::editing                             │    │ embedder IME client                       │
 │  • editing host = contenteditable elem   │    │  macOS: winit Ime + KeyEvent.text         │
 │  • Selection/Cursor over the host's      │    │  wasm:  hidden <textarea> proxy in facade │
 │    paragraph (its own, or its anonymous  │    │  headless: test driver                    │
 │    box's), offsets via its source map    │    │                                           │
 │  • composition range                     │    │                                           │
 │  • commands (insert, delete, move, …)    │◀───│ LynxView::edit(Command)    (counted FIFO) │
 │  • TextInputState snapshot per change    │───▶│ ViewNotice::TextInputState(TextInputState)│
 │ paint::text paints caret/selection/      │    │ Painter unchanged: text is vello content  │
 │   composition underline from that state  │    └──────────────────────────────────────────┘
 └─────────────────────────────────────────┘
```

### Editing host (`dom`)

- An element is an **editing host** when its `contenteditable` attribute is
  present and not `"false"` (case-insensitive). `"true"`, `""` and
  `"plaintext-only"` are all treated as plain-text editing in this series;
  rich-text editing of nested inline elements is deferred.
- **Editing is decoupled from the text container** (user rulings
  2026-10-19). A host keeps the layout its element kind and its `display`
  give it, and its text is edited in whichever paragraph that is:
  - a Lynx `<text contenteditable>` keeps its Lynx paragraph — verbatim
    white space, `text-maxline`/`text-maxlength` truncation — in its own
    `TextStore::Block`;
  - a block container (an engine sheet's `display: block`, such as E5's
    editor) keeps its own standard inline formatting context,
    `TextStore::Flow`;
  - any other element — a flex, grid, linear or relative `view` or `div` —
    lays its loose text out in the anonymous block box around it
    (`dom::tree::anonymous`, #434), whose standard inline formatting context
    is the paragraph. The host is that box's container.

  No `display` is forced: a `display: none` host is hidden and cannot be
  focused. The UA sheet only defaults white space, with no `!important`:
  `[contenteditable]:not([contenteditable="false" i]) { white-space: pre-wrap; }`,
  so typed spaces and line breaks show; a `<text>` ignores it, being
  verbatim, and an author `white-space: normal` collapses what the host
  shows while its value keeps every character.
- Content model in this series: the host's text is the concatenation of its
  direct text-node children, in content-walk order. The first edit
  normalizes the host to exactly one text node. Element children inside a
  host are not edited and make the host read-only for commands that would
  cross them (recorded limitation).
- Selection and caret are parley `editing::{Cursor, Selection}` values over
  the host's committed paragraph layout, refreshed after every relayout
  (`Selection::refresh`). The layout's bytes are not the value's: collapsed
  white space, an inline box's edges and a truncation tail move them. The
  paragraph's source units are the value's UTF-16 units, so every
  conversion goes value byte → UTF-16 unit → layout byte and back through
  the paragraph's own source map (`hughie::text::paragraph::OffsetMap`).
  Offsets published to the host are UTF-16 code units into the host's
  value.
- `Document` gains a minimal **focus model**: one focused element, `focus(node)`
  / `blur()`, `focus`/`blur` dispatched non-bubbling (and `focusin`/`focusout`
  bubbling) through the existing component-event path. Only editing hosts that
  are being rendered are focusable in this series.
- Painting (`paint::text`): selection highlight behind the glyphs, the caret
  (`caret-color`, falling back to `color`), and a thin solid underline under
  the composition range. Caret blink is deferred (needs a painter-side toggle;
  a main-thread timer would recommit every 500 ms).

### Commands (`dom::editing::Command`, applied on bobcat-main)

Chromium's set plus the local editing actions a keyboard produces:

| Command | Effect |
|---|---|
| `SetComposition { text, selection }` | replace the composition (or the selection when none) with `text`; selection inside it |
| `CommitText { text, relative_cursor }` | delete the composition, insert `text`, place the caret |
| `FinishComposition { keep_selection }` | commit the composition as-is |
| `SetCompositionFromExistingText { range }` | mark committed text as the composition (Android `setComposingRegion`) |
| `DeleteSurrounding { before, after }` | delete UTF-16 units around the selection |
| `ExtendSelectionAndDelete { before, after }` | widen then delete |
| `SetSelection { range }` | UTF-16 selection |
| `SelectFromPoint { point, extend }` / `SelectWordAtPoint` / `SelectLineAtPoint` | pointer placement, host-local CSS px |
| `Insert(text)`, `InsertLineBreak`, `DeleteBackward`, `DeleteForward`, `DeleteWordBackward`, `DeleteWordForward`, `Move(Granularity, Direction, extend)`, `SelectAll` | keyboard editing |

Every command that changes the value dispatches `beforeinput` (not
cancelable, see the ruling) then `input` with the standard `inputType`;
composition commands dispatch the `composition*` events, and `isComposing` is
true on inputs inside one.

### `TextInputState` (published to the host)

```
TextInputState {
  host: Option<NodeId>,            // None = nothing editable focused
  input_type: Text | Password | …, // from the element layer's attributes later
  input_mode, enter_key_hint,       // HTML inputmode / enterkeyhint attributes
  value: String,                    // whole value
  selection: Range<u32>,            // UTF-16
  composition: Option<Range<u32>>,  // UTF-16
  caret: Rect<f32>,                 // viewport CSS px, committed geometry
  editable_bounds: Rect<f32>,       // host border box, viewport CSS px
  composition_bounds: Vec<Rect<f32>>// one per UTF-16 unit of the composition
}
```

Published as `ViewNotice::TextInputState` after every commit in which it
changed, drained in `LynxView::pump` on the host thread. Geometry is the
committed one (main's adopted scroll offsets); it lags a fling by one mailbox
take, which only moves the candidate window, never the text. The per-frame
geometry query over `CommittedFrame` is the recorded follow-up if that matters.

### Host → engine

`LynxView::edit(Command)` on the host thread sends a counted
`ToMain::Edit(..)`, so it is ordered with pointer input and acknowledged by the
next frame post (E2 named it after `dom::editing::Command`, which it carries
as it is, rather than `ime`/`ImeCommand`: the same call carries a host's own
selection and editing commands). Keyboard keys travel as `InputKind::Key` (PR
#407); main inserts a printable key's W3C `key` value as the key's **default
action** when an editing host is focused.
Keys the IME consumed arrive as `Edit` commands instead and the platform's
`keydown` for them carries `key = "Process"`, matching browsers.

## Series (one PR each, not stacked)

| PR | Crates | Scope | Depends on |
|---|---|---|---|
| **E1** | dom, bobcat-core (UA sheet) | editing host, focus model, selection/caret/composition state, commands, caret + selection + composition painting, `TextInputState`, unit tests + goldens | — |
| **E2** | bobcat-core, bobcat-element (dom: two small additions) | `ToMain::Edit`, `LynxView::edit`, `ViewNotice::TextInputState` → `EngineEvent::TextInputStateChanged`, standard events into the realm (`EventDetail::{Input, Composition, Focus}` with string data), no cancellation channel (ruled 2026-10-19), pointer → caret placement and keyboard default actions on the main thread | E1, #407 |
| **E3** | bobcat-cli (macOS) | winit `Ime` → commands, `set_ime_allowed` / `set_ime_cursor_area` from the state, `KeyEvent.text` insertion, headless driver | E2 |
| **E4** | bobcat-wasm | hidden `<textarea>` proxy in `facade.ts` (`beforeinput`/`composition*` → messages → `LynxView::ime`), keyboard focus, `EditContext` where available | E2 |
| **E5** | bobcat-core (components), bobcat-element | `x-input` / `x-textarea`: UA shadow tree with a plain-text editing host, attributes (`value`, `placeholder`, `type`, `maxlength`, `confirm-type`, `disabled`, `readonly`, `input-filter`, `send-composing-input`), `lynxinput`/`confirm`/`selection`/`focus`/`blur` in web-core's shape, methods (`setValue`, `getValue`, `focus`, `blur`, `setSelectionRange`, `select`) | E2 |

Deferred after E5, in the order the survey ranks them: clipboard (needs a host
notice pair), caret blink, selection handles + magnifier + edit menu on touch
hosts, `::selection` colors, rich-text hosts, undo/redo, spellcheck markers,
the hidden native responder on iOS/Android, Windows TSF.

## Native vs web-core conflicts recorded for E5

`send-composing-input` default (iOS on, web-core attribute-gated → web-core);
`maxlength` unit (iOS grapheme clusters, browsers UTF-16 → browser);
`input-filter` is an attribute (iOS, web-core) not a method (Clay); textarea
`confirm` only with a non-default `confirm-type` (iOS) → web-core behaviour.

## E1: what was built

`dom::editing` (`crates/dom/src/editing/`; `paragraph.rs` resolves a host's
paragraph and converts offsets), the UA rule in
`crates/bobcat-core/src/main/tree/ua_sheet.rs` (`EDITING_HOST_RULE`), the
caret, selection and composition painting in `crates/dom/src/paint/text.rs`
and `walker.rs`, and the offset bridge in `hughie`
(`crates/hughie/src/text/paragraph/offsets.rs`). No host code, no realm
code.

Public surface:

- `Document::{is_editing_host, focus, blur, focused, take_focus_events}`;
  `editing::{FocusEvent, FocusEventKind}`.
- `Document::{prepare_editing, commit_editing, apply_editing}`;
  `editing::{Command, Granularity, Direction, PreparedEdit, EditOutcome,
  EditEvent, EditEventKind}`.
- `Document::text_input_state`; `editing::{TextInputState, InputType,
  InputMode, EnterKeyHint}`.
- `hughie::text::paragraph::OffsetMap` (`display_text`, `source_len`,
  `cut_byte`, `truncated`, `unit_to_byte`, `unit_to_byte_or_end`,
  `byte_to_unit`, `unit_before`), from `TextBlock::offset_map` and
  `InlineFlow::offset_map`.

### Choices made where the design left one

- **Focus events stay in `dom`.** `focus`/`blur` queue `FocusEvent`s
  (`blur`, `focusout`, `focus`, `focusin`, each with its `relatedTarget`) on
  the document, drained by `Document::take_focus_events`, the shape of
  `take_document_requests`. `bobcat-core`'s `ComponentEvents` is not touched:
  its drain dispatches every entry into the realm, and E1 wires nothing to
  the realm. E2 drains the queue into its dispatch with no `dom` change.
- **Focus fixup fires nothing.** A focused host that leaves the document
  (the removal path) or stops being a host (`contenteditable` removed or set
  to `"false"`) loses focus with no event, as HTML's focus fixup rule says.
  `focus()` on the focused element is a no-op that answers `true`, and puts
  the caret at the start of the value, as browsers do for `element.focus()`.
  A host inside a shadow tree counts as connected through its shadow host, so
  E5's UA shadow trees can be focused.
- **`:focus` is the focused host.** `focus`/`blur` set and clear
  `ElementState::FOCUS`; `:focus-within` and `:focus-visible` still match
  nothing.
- **Points are host border-box CSS px.** `SelectFromPoint` and the other
  pointer commands take the point relative to the host's border-box origin,
  the space `bounding_client_rect` puts the host in; `dom` subtracts the
  border and padding.
- **`CommitText`'s `relative_cursor` is Blink's**: relative to the end of the
  inserted text (`0` = just after it), in UTF-16 units, clamped to the value.
  An Android `newCursorPosition` converts as `n > 0 ? n - 1 : n - text_len`.
- **Event order follows UI Events and Input Events Level 2.**
  `compositionstart`, then per update `beforeinput` (`insertCompositionText`),
  `compositionupdate`, the change, `input`; `compositionend` alone at the
  end. A commit with text is `insertCompositionText` (Chromium), not Safari's
  `deleteCompositionText` + `insertFromComposition`. No `beforeinput` is
  cancelable here (the ruling above; Level 2 makes all but
  `insertCompositionText` cancelable). `Insert` during a composition ends it
  first (`compositionend`, then `insertText`). `DeleteSurrounding` reports
  one `beforeinput`/`input` pair for both sides (`deleteContentBackward` when
  anything before the selection goes, else `deleteContentForward`) where
  Blink reports two;
  `ExtendSelectionAndDelete` is `deleteContentBackward`. `deleteByCut` and
  `insertFromComposition` are not produced.
- **The prepare/commit seam.** `prepare_editing` computes the replacements,
  the selection and composition after them, and the event list, touching
  nothing but layout. `PreparedEdit::commit_index` says how many events go
  before the change. `commit_editing` refuses (`applied: false`) when the host
  lost focus or its value changed since the edit was prepared, so a listener
  that rewrote the value wins (last write wins). E2 dispatches the first part,
  commits, dispatches the rest.
- **Layout-reading commands lay out first.** Pointer placement, movement and
  the cluster and word deletions run `Document::layout` before they measure,
  as an editor must; offset-only commands do not. After a value change the
  selection is held as byte offsets until the next layout pass rebuilds the
  parley selection from them (`Selection::refresh` otherwise).
- **Normalization unlinks, never frees.** The first value change gives the
  whole value to the first text child and unlinks the others (their owner,
  such as a script handle, still owns them); a host with no text child gets a
  new one, owned by the host.
- **Selections are lenient, deletions exact.** `SetSelection` and
  `SetComposition`'s selection round an offset inside a surrogate pair down
  to the character's start and clamp past the end; `DeleteSurrounding` and
  `ExtendSelectionAndDelete` refuse a range that would split a pair.
- **`TextInputState`** carries the selection as `start ≤ end`; `caret` is the
  focus's caret box, 1 CSS px wide; `composition_bounds` gives each UTF-16
  unit the box of the grapheme cluster it belongs to (both halves of a pair
  share one). An empty host's caret is one used line tall (`line-height`,
  `normal` as 1.2 × `font-size`). Geometry is the last committed layout's;
  `text_input_state` runs no pass.
- **Painting.** The selection highlight is a UA constant (`#B4D5FE`), painted
  over inline backgrounds and under shadows and glyphs; the caret only shows
  for a collapsed selection; the composition underline is 1 CSS px in the
  text colour, half the descent under the baseline.
- **The paragraph of a host** (`Document::paragraph_node_of`): the host
  itself when it establishes a paragraph (a Lynx text element, a block
  container), else the anonymous block box of its first text child, whose
  container is the host. A host with element children can have several
  boxes; it is read-only, so the first one serves its selection. An empty
  host, or one holding only white space — which a flex, grid, linear or
  relative container does not render (css-flexbox-1 §4) — has no paragraph:
  it is edited on value offsets alone (a cluster is one code point there,
  a word or line move goes to the value's end), its caret sits at the
  content box's origin, one used line tall, and the walker paints that
  caret with the host's own box.
- **Offsets through the source map.** A value byte becomes a UTF-16 unit
  (`offsets.rs`), then a layout byte (`OffsetMap::unit_to_byte_or_end`); a
  layout byte becomes a unit read as a start (`byte_to_unit`) or, for a
  range's end, as an end (`unit_before`), then a value byte. A unit inside
  a collapsed white-space span lands on the byte the span emitted, and a
  layout byte there reads back as the span's first unit, so a caret placed
  between two collapsed spaces is shown on the one space and, once a layout
  pass has placed it, reported at the span's start. An inline box's edge
  bytes read as the unit after them, and a character move that only
  crossed an edge cluster steps on.
- **Trust is structural.** A paragraph's units are the value's when every
  child of the host is a text node, no generated `content` replaces them,
  and every run of the paragraph is the host's own
  (`EditableParagraph::shows_value`). Otherwise — generated `content`, an
  element child's text — every command but `SetSelection` and `SelectAll`
  is refused, and `text_input_state` reports a parley selection over such a
  paragraph collapsed at the value's end. A paragraph that is only stale
  behind the session's own change needs no exception: the change leaves
  the selection as value offsets until the next layout pass places it, and
  the commands that measure lay out first.
- **A truncation cut is per offset.** A `<text>`'s `text-maxline`/
  `text-maxlength` cut leaves bytes at and past `OffsetMap::cut_byte`
  showing the tail, no value text, and a `nowrap` `<text>` shows nothing
  after its first line. A command that must measure there — a move, a
  cluster or word deletion, a pointer placement landing on the tail — is
  refused; offset-only commands still apply, and a selection the layout
  cannot show stays held as value offsets and is painted at the end of
  what the layout shows.
- **Focus needs rendering.** `focus` refuses a host that is `display: none`
  or inside an element that is; a host no flush has styled yet is
  accepted. After every layout pass a focused host that is no longer
  rendered loses focus with no event, as HTML's focus fixup rule says.
- **`SetComposition` with empty text and no composition** deletes a range
  selection, as Blink's third case in `InputMethodController::SetComposition`
  does (for IMEs such as IBus-Anthy): no composition event, one
  `beforeinput`/`input` pair of `deleteContentBackward` (Blink's
  `DeleteSelection` and `TypingCommand::GetInputType` for
  `kDeleteSelection`), then the selection placed `selection` UTF-16 units from
  where the deleted range began (`SetEditableSelectionOffsets`). A collapsed
  selection changes nothing.

### Limitations

- **`caret-color` is not read.** The stylo fork compiles `caret-color` for
  Gecko only (its longhand has no `servo_pref`), so the caret is always the
  text colour (`caret-color: auto`). Enabling it is a fork change.
- **Read-only hosts.** A host with an element child refuses every
  value-changing command. A nested `contenteditable` inside a host is
  flattened into the outer paragraph and has no paragraph of its own.
- **Hosts whose paragraph shows other text are read-only.** Generated
  `content` leaves only `SetSelection` and `SelectAll` working, and past a
  truncation cut nothing that measures works (see above).
- **Collapsed white space snaps.** Under an author `white-space` that
  collapses, a caret between collapsed characters is shown on the one the
  run emitted and, after a layout pass, reported at the run's start; a
  deletion before it takes one of the value's characters.
- **A host's own scroll offset is not applied** to pointer points and
  geometry of a host that is itself its paragraph.
- **Clusters are parley's.** Cluster deletion and movement follow parley's
  shaping clusters, which depend on the font: with a face that lacks a
  combining mark, `e` and U+0301 can be separate clusters.
- **Visual, left-to-right movement.** `Character` and `Word` moves are
  visual; `Backward` means left. Right-to-left text moves visually too, which
  is the reverse of logical order there.
- **A trailing newline makes no line in an inline formatting context.** As
  in CSS, such a paragraph ending in `"\n"` has no empty last line box; the
  caret after it is placed on parley's empty trailing line, below the host's
  box (Chromium inserts a `<br>` there; a plain-text host here does not). A
  `<text>` host shows that empty last line, as native Lynx does.
- **Tabs.** A preserved tab advances to the paragraph's next tab stop: every
  8 space advances in an inline formatting context (`tab-size` is fixed at 8,
  the property being disabled under `lynx`), every 20px in a `<text>`.
- **No transforms in geometry.** `TextInputState` geometry, like
  `bounding_client_rect`, ignores transforms.
- **Not built yet:** caret blink, `::selection`, `selectionchange`,
  clipboard, undo. (The realm wiring of the focus and edit events is E2,
  below.)

## E2: what was built

The realm and host wiring of E1's kernel, in `bobcat-core` and
`packages/bobcat-element`: `crates/bobcat-core/src/main/runtime/editing.rs`
(the default actions, the edit and focus dispatches, the routing account, the
`TextInputState` publication), `LynxView::edit`, `ToMain::Edit`,
`ViewNotice::TextInputState`, `EngineEvent::TextInputStateChanged`, the new
detail kinds in `main/runtime/lib.rs` and `element-papi.ts`, and two small
`dom` additions: `Document::has_editing_hosts` and
`EventDetail::{Input, Composition, Focus}`.

Public surface:

- `LynxView::edit(&self, command: bobcat_core::editing::Command) ->
  Result<(), EngineError>` (`NotReady` before MTS boot or after the end, like
  the page updates).
- `EngineEvent::TextInputStateChanged(Box<bobcat_core::editing::TextInputState>)`.
- `bobcat_core::editing::{Command, Direction, Granularity, TextInputState,
  InputType, InputMode, EnterKeyHint}` (re-exports of `dom::editing`).
- `dom::Document::has_editing_hosts`; `dom::event::EventDetail::{Input,
  Composition, Focus}`.

### How an edit runs

`MainThreadRuntime::apply_edit` is the one path for every editing command,
whatever produced it (`ToMain::Edit`, a `keydown`'s default action, a press's
caret placement): `prepare_editing`; dispatch `events()[..commit_index]` at
the host, bubbling and composed; `commit_editing`; stop if the commit refused
(a listener moved the focus or rewrote the value); dispatch the rest. Every
dispatch is the kernel's one `MainThreadRuntime::dispatch` (#448–#451): it
asks the listener table whether any realm or component hears the name and
builds no path when none does, then walks the path once, components at their
steps and each invited realm called once at the top. No event is cancelable
(the ruling above): `__BobcatDispatchEvent` keeps its signature, answers
nothing, and its event object has no `cancelable`, `defaultPrevented` or
`preventDefault()`. A listener that throws is reported as `ListenerFailed`
and stops nothing.

New detail kinds: `DETAIL_INPUT` (5: `inputType`, `data` or null,
`isComposing`), `DETAIL_COMPOSITION` (6: `data`), `DETAIL_FOCUS` (7: the
`relatedTarget`'s id or null). Each puts its W3C members on the event object
and leaves the `detail` `{}`, as `DETAIL_KEY` does. The Rust constant for the
pointer/wheel/touch kind was renamed `DETAIL_POSITION` to match the realm's.

### Choices made where the brief left one

- **The command is `edit`, not `ime`.** `LynxView::edit(Command)` carries
  `dom::editing::Command` unchanged, which holds the keyboard and selection
  commands as well as Chromium's IME set.
- **Focus events are drained right after the press, not in the epilogue.**
  `dom` queues focus events only from `Document::focus`/`blur`, which only the
  press default action calls, and HTML runs the focus update inside the
  `mousedown` default action, before the `mouseup` and `click` of the same
  burst; draining in the epilogue would deliver `focus` after them. The fixup
  rule (a removed or uneditable host) fires nothing, so nothing else can queue
  one. A future `focus()`/`blur()` element method (E5) drains its own.
- **Routing: an editing account in the painter's name set, by state.** The
  painter forwards an event only when its name is in the published routed
  set. Since #449 that set is derived from `dom`'s listener table and the
  components' observed names in one function, `DocumentSlot::routes_given`;
  the editing account (`EditingRoute`, a bit per route in
  `DocumentSlot::editing_routes`) is one more term of it. A route flipping
  goes out through `DocumentSlot::change_names` after the waiting last
  removals (`apply_pending_last_removals`), the path an observed name's
  change takes, so each name moves only where the union changes, in order
  with the table's own changes. The kernel is not a registrant in the
  listener table: a row of its would be an invitee the dispatch has no realm
  to call for. The routes:
  - `pointerdown` and `click` while `Document::has_editing_hosts` (a counter
    `dom` keeps in the `contenteditable` setters and `free_node`; a detached
    host counts until it is freed);
  - `keydown` while `Document::focused()` is some;
  - `pointermove`, `pointerup` and `pointercancel` only while a mouse drag
    that placed a caret is held, opened by the press and closed when the
    drag ends (release, cancellation, a move with the button up, the focus
    moved, the next press) — `MainThreadRuntime::set_pressed`.

  The first two are synced by the page's epilogue before its commit
  (`DocumentSlot::sync_editing_routes`); the drag route is written inside
  the press's own job. The replica is a `watch` write with no
  acknowledgement and no wakeup, and `Painter::dispatch_input` polls the
  link before it routes, so every event the painter decides after the write
  sees it; one decided before it was filtered against the old set and never
  crossed. For a drag that means the first moves can be lost (the first one
  that crosses extends to its own position, so the selection catches up), and
  a release decided that early leaves the drag held until the next move with
  the button up or the next press. Likewise keys typed before the epilogue of
  the tap that focused a host are not routed. No hosted-node table reaches
  the committed frame. An event routed only on this account invites no realm
  and builds no path (the dispatch's own `hears` gate); `deliver_input_event`
  gates the default action on the account itself (`editing_routes(name)`),
  so it runs whether or not anything heard the event.
- **Touch and pen focus on the tap, a mouse on the press.** A mouse's
  primary `pointerdown` focuses the host and places the caret, as a
  browser's `mousedown` does. A finger's or pen's does it on the `click` the
  release produces (`deliver_input_event`, after `dispatch_click`, at the
  release target and position), as touch
  browsers do, so a drag the scroller consumed — which produces no `click`
  — does not focus the host it began on.
- **Multi-click counts presses, not clicks, for a mouse.** A browser selects
  the word on the second `mousedown`, so the main thread counts primary
  presses on the same host within `DOUBLE_CLICK_MILLISECONDS` (500 ms)
  itself, rather than reading `dispatch_click`'s count, which lands after the
  release. Two presses select the word, three or more the line. For touch
  and pen the same count runs over taps.
- **Only a mouse drags a selection.** A held mouse button's `pointermove`
  extends; a finger or pen dragging over a host is the scroll gesture, as on a
  touch browser.
- **A press outside every editing host blurs the focused one**, as a press on
  an unfocusable area does in a browser.
- **Keyboard mapping.** As the brief lists, plus meta-ArrowUp/ArrowDown to the
  value's start and end (`Granularity::Document`, the macOS binding), and
  Backspace/Delete doing nothing with ctrl or meta held rather than deleting
  one character. No platform-specific bindings (ctrl-arrow on Windows,
  emacs keys on macOS).
- **`TextInputState` is compared every epilogue while a host is focused**,
  after the commit, so a geometry-only change (a relayout, a scroll the page
  adopted and committed) is published as well; nothing is computed while
  nothing is focused and nothing was.

### Limitations

- **No shift-click extension.** `InputKind::Pointer` carries no modifiers, so
  a press cannot say shift is held; extending by shift-click needs that field
  in `dom::input` and every host filling it in.
- **No listener stops an edit or an editing default action** (the ruling):
  a browser's `beforeinput`, `keydown`, `pointerdown` or `mousedown` listener
  could.
- **A touch drag that scrolls nothing still taps.** This engine's `click`
  has no movement slop (only a consumed scroll or a cancel suppresses it), so
  a finger dragged over a host outside any scroller focuses it on release.
- **`relatedTarget` is not retargeted.** An element of a UA shadow tree
  reports `null` rather than its host.
- **`LynxView::edit` before boot is refused** (`NotReady`), as the page
  updates are.
- **A typed key's text is its W3C `key` value.** A host that knows the text a
  key produced (winit's `KeyEvent.text`) sends it as `Command::Insert` and
  lets the `keydown` carry `"Process"`, or relies on `key` (E3's choice).
