# bobcat-core: The Canvas 2D API

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The HTML Standard's `OffscreenCanvas` and 2D context (bucket 1: neither
native Lynx nor web-core defines a canvas API of its own), drawn by
`crates/bobcat-canvas`, which records each draw and replays it on the GPU.
A `<canvas>` element shows what an `OffscreenCanvas` commits to it once the
MTS realm has called `transferControlToOffscreen()` on it ("The placeholder
canvas" below); the element has no `getContext()`. An `OffscreenCanvas` and an
`ImageBitmap` move to another realm in a `postMessage` transfer list
("Transfer" below), so the BTS can draw on a placeholder's canvas. The
painter's side of a placeholder is `painter.md`'s "Placeholder canvases".

## Where it is

- `packages/bobcat-element/src/canvas.ts` (`bobcat:canvas`): every
  interface, Web IDL's argument conversions, the overloads, and the
  exceptions. `src/dom-exception.ts` (`bobcat:dom-exception`) is
  `DOMException`; no realm had one before.
- `crates/bobcat-core/src/canvas/`: `mod.rs` the per-realm `CanvasTable`,
  `members.rs` the `bobcat-internal:host` members (`native.d.ts`'s
  `BobcatCanvasNative` lists them), `css.rs` the color and `font` parsing.
- `packages/bobcat-element/src/element-papi.ts`'s `invokeUIMethod`: a
  `canvas`'s `transferControlToOffscreen`.
- `packages/bobcat-element/src/transfer.ts` (`bobcat:transfer`): the
  transfer list of every `postMessage`; `docs/workers.md`, "Transfer lists",
  is the transport.

## Names per realm kind

`bobcat:canvas` is imported for its effect, as `bobcat:timers` is: it puts
`OffscreenCanvas`, `OffscreenCanvasRenderingContext2D`, `CanvasGradient`,
`CanvasPattern`, `TextMetrics`, `ImageData`, `ImageBitmap`, `Path2D`,
`DOMException` and `createImageBitmap` on the realm's global object. The MTS
realm gets them from `bobcat:runtime`'s import (so a frame's `ShadowRealm`
card does too), the BTS from `bobcat:bts`'s, and a plain `Worker` from its
script's own `import "bobcat:canvas"`. The module also exports every name.

## Host state and lifetime

The canvas members are part of the core every realm has
(`realm::install_realm_core`), each realm, `ShadowRealm` children included,
over a `CanvasTable` of its own in its `RealmCore`. What the table has of a
document is its `CanvasHost`: `Worker` for a worker realm, `Document` for
the MTS realm and its children, which adds the member
`canvasTransferControl`. The table maps ids to `Entry` values — a canvas
(size, its `bobcat_canvas::Canvas` made at the first call that needs a
bitmap, and the `CanvasPresenter` of a placeholder's), a `Path`, a
`Gradient`, a `Pattern`, an `ImageBitmap`'s `Image` — all owned `Send`
values (a compile-time assertion in `mod.rs`), which `CanvasTable::remove`
takes out of the table.
Ids start at 1 and are never reused. Each JavaScript object is registered
with a `FinalizationRegistry` whose cleanup calls `canvasDrop`;
`ImageBitmap.close()` drops at once. The table is freed with the realm.

A member that fails as the standard says answers the exception's name as a
string, and the module throws it. Arguments are numbers, strings, booleans,
and for pixels a fixed-length `ArrayBuffer` with the view's offset and
length (`ImageData`'s constructor refuses a resizable buffer with a
`TypeError`, as Web IDL does). Lists of numbers cross as trailing arguments.

`fillStyle`, `strokeStyle`, `shadowColor` and `font` are kept by the module
with a state stack beside the host's, so a getter answers the gradient
object or the serialization the setter received.

## The placeholder canvas

Card main-thread code reaches it through `__InvokeUIMethod(canvas,
'transferControlToOffscreen', params, callback)`, which ReactLynx's
`MainThread.Element.invoke('transferControlToOffscreen')` calls and whose
promise resolves to `data`. web-core answers this method by calling the DOM
element's own (`createInvokeUIMethod.ts`), and so does this engine, in the
same `{code, data}` shape:

- For a `canvas`, `invokeUIMethod` (`element-papi.ts`) calls the host member
  `canvasTransferControl(nodeId)` instead of `callElementMethod`, whose
  answer is a record and cannot carry an object. The member runs
  `paint::canvas_pictures::transfer_control_to_offscreen` through the
  `CanvasDocument` impl on the document slot (`main/runtime/lib.rs`), files
  a canvas entry holding the answered presenter, its bitmap made at once,
  and answers `[id, width, height]`; `bobcat:canvas`'s
  `__BobcatTransferredCanvas` makes the `OffscreenCanvas` over that entry.
  The answer is `{code: 0, data: <OffscreenCanvas>}`, its size the
  placeholder's `width` and `height` at the transfer.
- A canvas already transferred answers `{code: 7}`, the code a kind's method
  failing with `InvalidStateError` answers (`main/runtime/lib.rs`'s
  `method_answer`, a dialog's methods); web-core answers 4 for a method that
  throws. An element of another kind takes the generic path and answers 3.
- The background thread's `SelectorQuery.invoke` of
  `transferControlToOffscreen` answers `{code: 3, data: <message>}` without
  running the method (`__BobcatQueryNodes`): its reply is posted to the BTS
  with no transfer list, and an `OffscreenCanvas` crosses only in one (user
  ruling 2026-11-19: transfer is through `postMessage` only). The BTS gets
  one from main-thread script that sends it with
  `lynx.getJSContext().postMessage(message, [canvas])`.

Setting the `OffscreenCanvas`'s `width` or `height` resizes its bitmap, and
the next commit sends `ToMain::CanvasResized`, on which `bobcat-main`
applies the new natural size (`update_placeholders`). A size with no bitmap
(a zero side, or one over 8,192 px) commits nothing: the placeholder keeps
the last commit's picture and size. Once the element is removed, draws and
commits go on and show nowhere; once it is freed, a commit does nothing.

## Transfer

Every `OffscreenCanvas` and `ImageBitmap` is marked uncloneable and
transferable as it is constructed (`markTransferable`), so a message holding
one that its transfer list does not name throws `DataCloneError`, and only a
listed one crosses, as one object at every position the message held it.
HTML's StructuredSerializeWithTransfer is followed with one change of order:
every check is made before anything moves, where HTML detaches each listed
object as its turn comes. The checks and their exceptions:

- a list entry that is not an `OffscreenCanvas` or an `ImageBitmap` (an
  `ArrayBuffer` included), a duplicate, a detached object, and an unlisted
  transferable in the message: `DataCloneError`;
- an `OffscreenCanvas` whose context mode is not none (it has a 2D context):
  `InvalidStateError`, HTML's `OffscreenCanvas` transfer steps ("If value's
  context mode is not equal to none, then throw an InvalidStateError");
- a second argument Web IDL refuses: `TypeError`.

The serializer's own list check runs before the context-mode check, so a list
holding both a canvas with a context and a detached object throws
`DataCloneError` where HTML, which checks entry by entry, may throw
`InvalidStateError`.

A post that succeeds takes the listed entries out of the table and detaches
the objects. A detached `OffscreenCanvas` (context mode `detached`) reads as
0 × 0; setting its size does nothing; `getContext` with any valid id and
`transferToImageBitmap` throw `InvalidStateError`, and as an image source it
is `InvalidStateError` for its zero size. A detached `ImageBitmap` is a
closed one: 0 × 0 and `InvalidStateError` as an image source.
`ImageBitmap.close()` detaches too, so a closed bitmap is refused in a list.
The message (`canvas::Transfer`) carries the taken entries as a
`Box<dyn Moved>`, a private trait with `Send` as a supertrait, so a type that
holds a message — a worker's channel, an embedder's `ModuleCallback` — is
proven `Send` without the compiler walking an entry's GPU types, and no crate
raises its recursion limit.

The receiving realm's reviver files each entry in its own table and makes the
object over it: an `OffscreenCanvas` of the entry's size, still bound to its
placeholder if it had one, or an `ImageBitmap` with its pixels. A canvas whose
size changed in the entry that posted it, before its realm committed that
size, is listed as drawn in the receiving realm, which commits it.

**A canvas in a worker realm.** A transferred placeholder canvas commits from
the worker realm's epilogue on `bobcat-workers` through the same
`settle_drawn`: its `CanvasPresenter` copies the texture, wakes the view's
host when a wake is owed, and sends `ToMain::CanvasResized` through its weak
command sender when the size changed, from that thread. A draw in a timer, a
message listener or a `requestAnimationFrame` callback (`lynx`'s in the BTS,
`bobcat:animation-frame`'s in a plain worker) is an entry like any other and
commits at its end. Text in a worker realm shapes with the view's host
fonts and the system's ("Colors and fonts"), not the document's
`@font-face` faces.

## The per-entry step

Draws are recorded and replayed onto the canvas's GPU texture at a read
(`bobcat-canvas`'s "Recording and replay"). Every draw lists its canvas as
drawn; the shared epilogue (`realm/owner.rs`, step 5, after the due timers
and before the role's commit) calls `RealmCore::settle_drawn_canvases`,
which runs `CanvasTable::settle_drawn` for the realm and each child. Each
canvas drawn during the entry is flushed, except one with a presenter whose
bitmap changed (`Canvas::has_changes`), which is committed instead
(`CanvasPresenter::commit`, which replays first): HTML's update of an
`OffscreenCanvas`'s placeholder, done here at the end of each JavaScript
entry, once per canvas, and only for an entry that changed it. An entry that
drew nothing costs one empty-list test. A transferred canvas never drawn is
never committed, and its placeholder draws nothing and is not waited for.

## Reading pixels

`getImageData` is `Canvas::read_image_data`, which replays what is recorded
and issues the readback, then a wait on its answer (`ImageDataRead`), taken
after the table's borrow is released (`ReadWait`). Natively the calling
thread does the readback itself as it is issued, and the wait returns at
once. On wasm32 the Render Worker does it in its next turn, and the job
parks as a synchronous `require` does (`JsThread::wait`, the realm's token
the biased first arm, `ImageDataRead::wait_async` the other): the thread's
tasks go on running and no other job does. The wait answers the pixels, or
`InvalidStateError` when the GPU did not answer (no adapter, a lost device,
the Render Worker dropping the read for want of a device) or the realm
ended first. No automated test runs the wasm32 wait; the local browser
rendering check does (`docs/browser-wasm.md`, "Rendering check").

## Colors and fonts

- A `<color>` parses with stylo's `specified::Color::parse_and_compute` over
  an `about:blank` author context and no device, so it works on either
  thread. The fork's `lynx` grammar leaves out `currentcolor`, which
  `css.rs` matches itself, and `color-mix()`, `light-dark()` and
  `contrast-color()`, which are refused; system colors need a device and are
  refused too. The getter serializes as HTML says: `#rrggbb` when opaque,
  else `rgba(r, g, b, a)`.
- `font` parses with stylo's shorthand parser
  (`properties::shorthands::font::parse_value`); the getter is the
  shorthand's own serialization (`LonghandsToSerialize`) with the size in px
  and no line height. `small-caps` parses and is not drawn (`Font` has no
  variant).
- Text is shaped with the realm's fonts: the MTS realm and its children use
  the document's `TextContext` (`Document::text_context_mut`, through the
  `CanvasDocument` impl on the document slot), so `@font-face` faces draw; a
  worker realm makes a `TextContext` at its first text call from the view's
  `WorkerFonts` (`view/lib.rs`, carried by the worker's `HostOutbox`): the
  system fonts and the view's `ViewSources::fonts`, `fallback_fonts`,
  `default_font_family` and `locale`, staged as the document's context is.
  On wasm32, which has no system fonts, those host fonts are the only ones a
  worker realm has. `TextContext::fonts_for` fills the fallback lists a
  missing character needs first.

## Size limits

A bitmap side is at most 8,192 px (`MAX_SURFACE_SIDE`). An `OffscreenCanvas`
may be made larger; it then has no bitmap: draws do nothing, `getImageData`
answers transparent black (up to 2^28 px, `RangeError` above), and as an
image source (`drawImage`, `createPattern`, `createImageBitmap`) it is
`InvalidStateError`, as a canvas with a zero side is. `ImageData` has no
limit of its own; `createImageBitmap` of one with a side over 8,192 px is
`InvalidStateError`.

## Deviations

- `getContext('2d', options)` reads none of `options`: `willReadFrequently`
  changes nothing (every read is a GPU readback), and `alpha: false` leaves
  the bitmap's alpha as drawn.
- `ImageBitmap` is transferable and not serializable: posted without being
  listed it throws `DataCloneError`, where HTML copies its pixels.
- A relative font size resolves against 10px, which is HTML's rule for an
  `OffscreenCanvas` (its font style source object is not an element, so
  "the relative keywords must be interpreted relative to the normal-weight
  10px sans-serif default"), but `ex`/`ch` take half of it and `lh` 1.2
  times it rather than the default font's metrics. Viewport and container
  units, `rpx` and a `calc()` of a relative length are refused.

`currentcolor` is opaque black: an `OffscreenCanvas` context's style
setters pass HTML's color parsing no element ("Let context be this's canvas
attribute's value, if that is an element; otherwise null"), and without
one `currentcolor` is the initial `color`, opaque black.
