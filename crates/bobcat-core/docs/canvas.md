# bobcat-core: The Canvas 2D API

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The HTML Standard's 2D context and `OffscreenCanvas` (bucket 1: neither
native Lynx nor web-core defines a canvas API of its own), drawn by
`terminus`'s 2D context (`terminus::link::html::canvas`,
`crates/terminus/src/html/canvas/context2d/`), which records each draw and
replays it on the GPU. This crate exposes it as host members of
`bobcat-internal:host`, functions over ids. Their callers are the engine's
built-in JavaScript (the `bobcat:*` modules of `packages/bobcat-element`)
and an embedder's native-module ES module (`lynx:<name>`,
`NativeModule::esm` in `src/native_module.rs`). Of the built-in modules,
`bobcat:element` calls two, `canvasTransferControl` and `canvasDetach`, for
the `transferControlToOffscreen` UI method ("Cards" below).

A `<canvas>` element shows what a canvas commits to it once the MTS realm
has bound it as the canvas's placeholder with `canvasTransferControl` ("The
placeholder canvas" below); the element has no `getContext()`. A canvas or
image bitmap moves from one realm of a view to another by token ("Moving a
canvas between realms" below). The painter's side of a placeholder is
`painter.md`'s "Placeholder canvases".

## Cards

The engine gives cards no canvas API. No realm has `OffscreenCanvas`,
`OffscreenCanvasRenderingContext2D`, `CanvasGradient`, `CanvasPattern`,
`TextMetrics`, `ImageData`, `ImageBitmap`, `Path2D`, `DOMException` or
`createImageBitmap` as a global, no built-in module exports them, and
`bobcat:canvas` is no module (its import fails in the realm with a
`ReferenceError`, as any unregistered `bobcat:` name does).

The canvas members are called by the engine's built-in modules and by an
embedder's `lynx:<name>` module, which may define a card-facing API over
them. `bobcat-internal:host` is not closed to card code: an `import` of it
from a card links like any other, as it does for every other host member,
and nothing checks the caller.

### `transferControlToOffscreen` as a UI method

`__InvokeUIMethod(canvas, 'transferControlToOffscreen', params, callback)`
on a `canvas` element does not reach `callElementMethod`:
`element-papi.ts`'s `invokeUIMethod` calls `canvasTransferControl` on the
element and, when that answers an entry, `canvasDetach` on it in the same
call, and answers `{code: 0, data: {token, width, height}}`: the hand-over
token ("Moving a canvas between realms" below) and the placeholder's
`width` and `height` at the transfer. The entry is in no realm's table
until a `canvasAttach(token)` files it. A canvas already transferred
answers `{code: 7, data: undefined}` (7 is `INVALID_STATE_ERROR`, the code
a kind's method failing with `InvalidStateError` answers). `params` is not
read. Any other element kind, and any other method on a `canvas`, takes
the generic path, where no element kind and no base method answers that
name, so the answer is code 3 (`METHOD_NOT_FOUND`).

The background thread's `SelectorQuery.invoke` of the method runs the same
`invokeUIMethod` in the MTS realm, so it answers the same: the data is
three numbers, which the existing reply carries. A token nobody attaches
stays in the view's hand-over store until the view ends.

An embedder's module reaches a `<canvas>` from a worker realm this way:

1. `lynx.createSelectorQuery().select(selector).invoke({method:
   'transferControlToOffscreen', success, fail})` from `bobcat:bts-runtime`;
2. `success({token, width, height})` calls `canvasAttach(token)`, which
   answers `[id, width, height]` as a `Float64Array`'s bytes, or
   `"InvalidStateError"` for a token already attached;
3. the module draws on `id` with the canvas members; the end of each
   JavaScript entry that changed the canvas commits it to the placeholder
   from the worker's thread ("The per-entry step" below).

### What an embedder's module writes itself

For canvas, the engine gives such a module the members and the UI method
above and nothing else. A card-facing API over them writes:

- the class layer: Web IDL's argument conversions and overloads, since a
  member converts nothing ("The members" below);
- the exceptions whose names the members answer, thrown as objects: a
  `DOMException` of its own, since no realm has one;
- the values the members do not read back: `fillStyle`, `strokeStyle`,
  `shadowColor` and `font`, and a gradient or pattern set with
  `canvasSetPaint`;
- `canvasDrop` of an entry once its object is collected, through a
  `FinalizationRegistry`: no garbage collection drops an entry.

What the engine does not have for this route:

- An image source other than a canvas, an image bitmap or unpremultiplied
  RGBA8 pixels (`canvasBitmapFrom`, `canvasBitmapFromPixels`): nothing
  decodes an image from a URL or from encoded bytes.
- `lynx:` modules in the MTS realm: `esm.rs`'s `build_main_runtime`
  reserves the `lynx:` prefix on `bobcat-main`, and a `ShadowRealm` child's
  `lynx:` import is refused (`realm/owner.rs`), so the route covers the BTS
  and each `Worker`. Code in the MTS realm calls the members directly.
- An ES module from the browser embedder: no native module `bobcat-wasm`
  registers supplies an `esm()`, and `BobcatCanvas.create` has no option that
  takes one. `bobcat-core`'s `esm` path itself has no wasm32-specific code.

The member contract is declared in
`packages/bobcat-element/src/canvas-host.d.ts`, which an embedder's module
is written against. A change to a member's name, parameters or answer
changes what those modules are written against: update `canvas-host.d.ts`
and the member list below in the same change. A test in
`canvas/tests.rs` holds the names `canvas-host.d.ts` exports equal to
`MEMBERS` and `DOCUMENT_MEMBER`.

## Where it is

- `crates/bobcat-core/src/canvas/`: `mod.rs` the per-realm `CanvasTable`,
  `members.rs` the `bobcat-internal:host` members (`canvas-host.d.ts`'s
  `BobcatCanvasNative` declares each, with its answer). A context member runs on the canvas's `Context2d`
  (`CanvasTable::context`, or `CanvasTable::draw` for one that may change
  the bitmap and so lists the canvas as drawn); a canvas member runs on the
  `OffscreenCanvas` itself (`CanvasTable::draw_canvas`, the
  `CanvasEntry`).
- `crates/bobcat-core/src/paint/canvas_pictures.rs`: the placeholder's
  presenter (`painter.md`).

## The members

The canvas members are part of the core every realm has
(`realm::install_realm_core`): the MTS realm, each `ShadowRealm` child, the
BTS and each `Worker`, each over a `CanvasTable` of its own in its
`RealmCore`. What the table has of a document is its `CanvasHost`: `Worker`
for a worker realm, `Document` for the MTS realm and its children, which
adds the member `canvasTransferControl`.

They cover the 2D context's operations, state reads and writes, pixel
reads and writes, and the objects a context uses:

- objects: `canvasCreate`, `canvasSetSize`, `canvasDrop`; paths
  (`canvasPath`, `canvasPathFromSvg`, `canvasAddPath`), gradients
  (`canvasLinearGradient`, `canvasRadialGradient`, `canvasConicGradient`,
  `canvasAddColorStop`), patterns (`canvasCreatePattern`,
  `canvasPatternSetTransform`) and image bitmaps (`canvasTransferToBitmap`,
  `canvasBitmapFrom`, `canvasBitmapFromPixels`, `canvasBitmapWidth`,
  `canvasBitmapHeight`);
- the state stack, the transform, the numeric, keyword and boolean
  attributes (`canvasGetNumber`/`canvasSetNumber` and
  `canvasGetKeyword`/`canvasSetKeyword` with a `which` selector,
  `canvasGetSmoothing`/`canvasSetSmoothing`), the styles
  (`canvasSetColor`, `canvasSetPaint`), the line dash and the font;
- rectangles, path building on a canvas's default path or a path entry,
  fill, stroke, clip and the two hit tests, text (`canvasFillText`,
  `canvasStrokeText`, `canvasMeasureText`), `canvasDrawImage`,
  `canvasGetImageData` and `canvasPutImageData`;
- `canvasDetach` and `canvasAttach`, which move a canvas or image bitmap
  between the realms of a view;
- `canvasTransferControl`, in a realm with a document.

A member converts nothing. Its arguments are numbers, strings, booleans,
and for pixels a fixed-length `ArrayBuffer` with the offset and length of
the bytes to read; lists of numbers cross as trailing arguments. A value of
the wrong type, or an id naming nothing of the kind the member takes,
throws the bridge's `InternalError`, its message starting with the member's
full name (`members.rs`'s `named`). A call that fails as the standard says
throws nothing and answers the exception's name as a string
(`"IndexSizeError"`, `"InvalidStateError"`, `"SyntaxError"`,
`"TypeError"`, `"RangeError"`); the caller decides what to do with it. A
member that answers several numbers answers a `Float64Array`'s bytes as an
`ArrayBuffer`.

There is no state on the JavaScript side. `fillStyle`, `strokeStyle`,
`shadowColor` and `font` have no getter: `canvasSetColor` and
`canvasSetFont` answer the serialization of what they set (`undefined`,
changing nothing, for a string that does not parse), and nothing reads the
value back afterwards. A gradient or pattern set with `canvasSetPaint` is
not answered back either.

## Entries and lifetime

The table maps ids to `Entry` values — a canvas (its size, its
`terminus::link::html::canvas::OffscreenCanvas` made at the first call that
needs a bitmap, and the `CanvasPresenter` of a placeholder's), a `Path`, a
`Gradient`, a `Pattern`, an image bitmap's `Image` — all owned `Send`
values (a compile-time assertion in `mod.rs`). Ids start at 1 and are never
reused, so a stale id names nothing. An entry lives until `canvasDrop(id)`,
`canvasDetach(id)` or the realm's end: no JavaScript object stands for it, and a garbage
collection drops nothing. The table is freed with the realm.

## The placeholder canvas

`canvasTransferControl(nodeId)` is HTML's `transferControlToOffscreen()` on
the element: it runs `paint::canvas_pictures::transfer_control_to_offscreen`
through the `CanvasDocument` impl on the document slot
(`main/runtime/lib.rs`), files a canvas entry holding the answered
presenter, its bitmap made at once, and answers `[id, width, height]`, the
size the placeholder's `width` and `height` at the transfer. An element
that is not a canvas in context mode none — another kind, or a canvas
already transferred — answers `"InvalidStateError"`; an id that is not a
live element throws. `bobcat:element`'s `transferControlToOffscreen` UI
method calls it ("Cards" above).

`canvasSetSize` on that canvas resizes its bitmap, and the next commit
sends `ToMain::CanvasResized`, on which `bobcat-main` applies the new
natural size (`update_placeholders`). A size with no bitmap (a zero side,
or one over 8,192 px) commits nothing: the placeholder keeps the last
commit's picture and size. Once the element is removed, draws and commits
go on and show nowhere; once it is freed, a commit does nothing.

## Moving a canvas between realms

Every realm kind has `canvasDetach(id)` and `canvasAttach(token)`, over one
`CanvasHandOver` store per view (`canvas/mod.rs`). The view's `ViewOutbox`
makes the store, and every `HostOutbox` it hands out carries the same
`Arc`: the MTS realm's and its `ShadowRealm` children's, the BTS's and each
`Worker`'s. That outbox is the narrowest owner all of a view's realms
share; nothing narrower reaches both `bobcat-main` and `bobcat-workers`.

- `canvasDetach(id)` takes the canvas or image bitmap entry `id` out of the
  calling realm's table, stores it under a new token and answers the token,
  a number. Tokens start at 1 and are never reused. A canvas goes as it
  is: its bitmap, its context state, the draws it recorded and has not
  replayed, and its placeholder's presenter if it has one. `id` names
  nothing afterwards. An id that names a path, gradient or pattern, or
  nothing, throws as any wrong id does. The canvas leaves the table's drawn
  list, so the detaching entry's epilogue neither flushes nor commits it,
  and the table's `ReplayUser` is let go when its last canvas leaves.
- `canvasAttach(token)` takes the entry out of the store, files it in the
  calling realm's table under a new id and answers `[id, width, height]` as
  a `Float64Array`'s bytes, the encoding `canvasTransferControl` answers
  with. A token that names nothing in the store — never issued, already
  attached, another view's — answers `"InvalidStateError"` and throws
  nothing. A canvas that arrives with recorded draws, or with a bitmap
  changed since its last commit (a size or a draw in the entry that
  detached it), is listed as drawn, so the receiving entry's epilogue
  replays it or commits it to its placeholder. The receiving table takes a
  `ReplayUser` as it files a canvas.

The token is a plain number, so it travels in any message a realm can
already send — a `Worker`'s `postMessage`, the cross-thread context — and
no transport carries anything of the canvas itself. A placeholder canvas
attached on `bobcat-workers` commits from there: its presenter is `Send`,
and the commit's `ToMain::CanvasResized` reaches `bobcat-main` through the
presenter's weak handle on the view's command channel. An entry that is
never attached is dropped with the store, when the last outbox holding it
goes, which is after the view and its workers have ended.

HTML's transfer steps refuse an `OffscreenCanvas` whose context mode is not
none. There is no `getContext()` here, so `canvasDetach` makes no such
check: a canvas moves with its context state.

## The per-entry step

Draws are recorded and replayed onto the canvas's GPU texture at a read
(`docs/terminus-architecture.md`, "Canvas 2D context"). The replay into a
`vello_gpu` scene, the CPU half of the render, runs on the realm's own
thread at the flush; on wasm32 the Render Worker runs only the GPU half
("Canvas surfaces" there). That replay uses the thread's one glyph outline
cache, shared by every canvas of every realm on the thread and never by the
page painter; a realm's `CanvasTable` holds a `ReplayUser` while it holds a
canvas entry, and lets go of it when `canvasDrop` removes its last one, so
the cache is dropped once no realm on the thread holds a canvas (a group's
view realms share `bobcat-main`, its BTS and `Worker`s `bobcat-workers`).

Every member that may change a bitmap lists its canvas as drawn; the shared
epilogue (`realm/owner.rs`, step 5, after the due timers and before the
role's commit) calls `RealmCore::settle_drawn_canvases`, which runs
`CanvasTable::settle_drawn` for the realm and each child. Each canvas drawn
during the entry is flushed, except one with a presenter whose bitmap
changed (`OffscreenCanvas::has_changes`), which is committed instead
(`CanvasPresenter::commit`, which replays first): HTML's update of an
`OffscreenCanvas`'s placeholder, done here at the end of each JavaScript
entry, once per canvas, and only for an entry that changed it. An entry
that drew nothing costs one empty-list test. A transferred canvas never
drawn is never committed, and its placeholder draws nothing and is not
waited for. The text members, `canvasMeasureText` included, list the canvas
as drawn too.

## Reading pixels

`canvasGetImageData` is `Context2d::read_image_data`, which replays what is
recorded and issues the readback, then a wait on its answer
(`ImageDataRead`), taken after the table's borrow is released
(`ReadWait`). Natively the calling thread does the readback itself as it is
issued, and the wait returns at once. On wasm32 the Render Worker does it
in its next turn, and the job parks as a synchronous `require` does
(`JsThread::wait`, the realm's token the biased first arm,
`ImageDataRead::wait_async` the other): the thread's tasks go on running
and no other job does. The wait answers the pixels, unpremultiplied RGBA8,
or `"InvalidStateError"` when the GPU did not answer (no adapter, a lost
device, the Render Worker dropping the read for want of a device) or the
realm ended first. The wasm32 wait is compiled and not run by any test.

## Colors and fonts

- `canvasSetColor`, `canvasSetFont` and `canvasAddColorStop` hand the
  string, unparsed, to the 2D context's string setters in `terminus`
  (`Context2d::set_fill_style_css`, `set_stroke_style_css`,
  `set_shadow_color_css`, `set_font_css`, `Gradient::add_color_stop_css`),
  and answer what they answer: the serialization, `undefined` for a string
  that does not parse, or the exception's name. How a `<color>` and the
  `font` shorthand parse, compute with no document and serialize is
  `docs/terminus-architecture.md`, "Canvas 2D context". The parsed font is
  `terminus`'s `html::canvas::ResolvedFont` (`seldon`'s). `small-caps`
  parses and is not drawn (`ResolvedFont` has no variant).
- Text is shaped with the realm's fonts: the MTS realm and its children use
  the document's `TextContext` (`Document::text_context_mut`, through the
  `CanvasDocument` impl on the document slot), so `@font-face` faces draw; a
  worker realm makes a `TextContext` at its first text call from the view's
  `WorkerFonts` (`view/lib.rs`, carried by the worker's `HostOutbox`): the
  system fonts and the view's `ViewSources::fonts`, `fallback_fonts`,
  `default_font_family` and `locale`, staged as the document's context is.
  On wasm32, which has no system fonts, those host fonts are the only ones a
  worker realm has. The canvas text calls take that `TextContext` and shape
  through `seldon::text::shape_line`, which fills the fallback lists a
  missing character needs first and hands parley the context's locale.

## Size limits

A bitmap side is at most 8,192 px (`MAX_SURFACE_SIDE`). `canvasCreate` and
`canvasSetSize` take a larger size; the canvas then has no bitmap: draws do
nothing, `canvasGetImageData` answers transparent black (up to 2^28 px,
`"RangeError"` above), and as an image source (`canvasDrawImage`,
`canvasCreatePattern`, `canvasBitmapFrom`) it is `"InvalidStateError"`, as
a canvas with a zero side is. `canvasBitmapFromPixels` of pixels with a
side over 8,192 px is `"InvalidStateError"`, and so is a crop (`sw`, `sh`)
with a side over 8,192 px, from any source.

## Deviations

- A canvas has no context options: every read is a GPU readback, and the
  bitmap's alpha is kept as drawn.
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

## Tests

`canvas/tests.rs` runs one suite over the members in the MTS realm, a
`ShadowRealm` child, the BTS and a `Worker`, each importing
`bobcat-internal:host` from a test module, and checks that no realm kind
has a canvas global or a `bobcat:canvas` module; it also holds the names
`canvas-host.d.ts` exports and declares equal to `MEMBERS` and
`DOCUMENT_MEMBER`. `paint/canvas_tests.rs`
drives a placeholder end to end through `canvasTransferControl`, through
`__InvokeUIMethod` and `canvasAttach` in the MTS realm, through the BTS's
`SelectorQuery.invoke` and `canvasAttach` there, and through a native
module's `lynx:` ES module that gives the BTS entry a small canvas API over
the members;
`main/runtime/tests.rs` and `main/page_tests.rs` hold the table's lifetime,
the `ReplayUser` hold and the per-entry step. The hand-over is tested
between the MTS realm and a `Worker` both ways (`canvas/tests.rs`), and for
a placeholder canvas the MTS realm detaches and the BTS attaches
(`paint/canvas_tests.rs`).
