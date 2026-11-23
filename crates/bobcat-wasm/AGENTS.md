# crates/bobcat-wasm

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/bobcat-wasm`. Paths in backticks are relative to the repository root.

The pure-Rust `wasm-bindgen` browser embedder and npm facade, built for
`wasm32-unknown-unknown` with shared memory. `loadTemplate` takes binary web
and source-based native bundles, delegating decoding, page configuration and
StyleInfo registration to `bobcat-source::PageSource`; the original response
URL remains the resource base. The Pages Canvas tab passes local ZIP bytes and
an entry URL through `loadZip` to `bobcat-source::ZipSource`. Each page gets a
separate resource scope, retaining archive assets after boot and isolating
image caches and completion queues while sharing the platform decoder. The
browser has no executor: each load is a local task on the Render Worker. The
service worker only provides cross-origin isolation headers; `loadLynxXml`
retains its XML-only, host-configured contract.

**The browser platform** (`src/platform/`, `bobcat_wasm::platform`): the
`vant::Platform` the Render Worker's resource system is built over.
`BrowserFetch` is the Worker's own `fetch`, which applies HTTP cache semantics
itself (`owns_http_cache`), so there is no byte tier. `BrowserDecoder`
(`decode.rs`) is the Render Worker's own decoders, and its frames are
platform frames: `createImageBitmap` of the bytes as a `Blob` decodes a
still image and an animation's first frame (`imageOrientation:
"from-image"`, `premultiplyAlpha: "premultiply"`, `colorSpaceConversion:
"default"`, a second `createImageBitmap` with `resizeWidth`/`resizeHeight`/
`resizeQuality: "high"` when the image is larger than asked for), and
WebCodecs' `ImageDecoder` says what an animation's frames are and decodes
frame *i*, one decoder per animation kept open until `forget_frames`; where
`ImageDecoder` is missing or refuses the type an image is one frame
(`src/image_frames.rs`, the pure half, natively tested). The answer is a
`BrowserFrame` (`frame.rs`), an `ImageBitmap` closed when its last copy is
dropped, which implements `bobcat_core::FrameWriter` with WebGPU's
`copyExternalImageToTexture` into the texture the painter created, in sRGB
and premultiplied. The pixels never pass through Wasm memory and the main
thread holds no image: every buffer handed to the browser is a fresh copy,
since a `Blob` or an `ImageDecoder` cannot be built over the shared memory.
`BobcatRenderer` builds its painter with `Painter::with_platform_frames`, a
`Painter<BrowserFrame>`. `BrowserMedia` (`media.rs`) is the video player
(`vant::media::Media<BrowserFrame>`), in two halves because an
`HTMLVideoElement` exists only where a `Document` does: the Render Worker's
half keeps each open video's sink under a number and posts the facade every
instruction as a `bobcat-media` message through the function
`BobcatRenderer.create` was handed (`postMediaRequest` in
`js/render-worker.ts`); the facade's half (`js/media.ts`, `MediaElements`)
owns a detached `<video>` per video, plays `http(s)` URLs itself and the
bytes the engine fetched for anything else from a `blob:` URL, takes each
presented frame with `requestVideoFrameCallback` (and the first at
`loadeddata`), and transfers it as a `VideoFrame` (`bobcat-media-frame`)
with its `loadeddata`, `ended` and `error` reports and a rejected `play()`
(`bobcat-media-event`). The Render Worker's script hands those to the free
exports `deliverMediaFrame` and `deliverMediaEvent`, outside the request
queue, since they touch only a video's sink. A video frame is a
`BrowserFrame` too, closed when its last copy goes, written with
`copyExternalImageToTexture` from the `VideoFrame`. Autoplay rules apply as
the browser has them: an unmuted `play` outside a user gesture is the
video's `error`. The code
cache's store is `CacheStore` (`cache.rs`), a `vant::AsyncStore` over one
cache of the web Cache API (`caches.open("bobcat-code-cache")`, 32 MiB,
`Volatility::EvictableAnytime`), handed over as `ArtifactStore::Async`, so
compiled scripts survive a page load and are shared by every tab of the
origin. What holds:

- `BrowserPlatform::new(code_cache)` is `async` because `caches.open` is,
  and never fails: a Worker with no `createImageBitmap` (`images will not
  decode: …`), a scope without `caches` (a context that is not secure) or an
  `open` that rejects (private browsing in some browsers) each become a note
  (`the code cache will not persist: …`) and the view runs without images
  or with the in-memory tier respectively. `caches` and
  `navigator.storage` are read from the global scope, so the store works in a
  Window or a Worker.
- A key is the request URL `https://bobcat.invalid/<key>` (each path segment
  percent-encoded; `Cache.put` accepts only `http(s)`, and nothing is fetched
  from it). An entry is a `Response` whose body is the entry bytes and whose
  headers carry the `Meta`: `x-bobcat-key`, `x-bobcat-length`,
  `x-bobcat-last-used` (Unix seconds) and `x-bobcat-record` (percent-encoded
  JSON). `scan` is `matchAll()` reading headers only; an entry whose headers
  do not parse is found through `keys()` and deleted. `touch` re-puts the
  stored body's stream under a new `last_used` header. Recency is that header
  alone, never the order `keys()` or `matchAll()` answer in. `estimate` is
  `navigator.storage.estimate()` when present.
- Every body is copied through a fresh non-shared `Uint8Array`: the Wasm
  memory is a `SharedArrayBuffer`, which a `Response` body cannot be built
  over, and a view over it is detached by a memory growth.
- All Cache API calls are stable `web-sys` bindings. The store's policy, and
  every overlap of its asynchronous calls, is `vant`'s and is tested natively;
  `CacheStore` itself has no automated test (the Wasm tests run in Node).
- The one check that renders in a browser is local:
  `node crates/bobcat-wasm/scripts/render-check.ts` after
  `pnpm --filter bobcat-wasm build` serves a Lynx XML card with a still PNG
  and three GIFs with the cross-origin isolation headers, drives Google
  Chrome headless over its DevTools protocol, and asserts on screenshot
  pixels: the still image's colour and alpha, a finite GIF resting on its
  last frame both as a CSS image (the file's count) and as an
  `<image loop-count="1">` (an `<image>` without `loop-count` loops
  forever), and a looping one showing both its frames. `--repeat <n>` runs
  it n times; a failed run prints the console of the page and its Workers. It needs a WebGPU
  adapter in headless Chrome; CI's Ubuntu runner has no GPU, so CI does not
  run it (`docs/browser-wasm.md`, "Rendering check").

The browser UI thread is a JavaScript-only host coordinator: it creates one
explicit embedder Worker and transfers an `OffscreenCanvas`, but never
instantiates Wasm or owns engine state. That Worker initializes the module,
constructs one opaque `LynxGroup` and one `LynxView` per page through
`BobcatRenderer::load`, keeps **one `Painter` for its canvas across page
loads** (rebuilt only when it is missing or its target has failed), permanently
owns every thread-affine GPU object — crates.io wgpu 30 Device,
Queue, Surface, the `vello_gpu` Renderer, and OffscreenCanvas — and uses `wasm_thread` to
create the two Workers each group is made of: its nested Lynx main/VM Worker
and the worker-realm Worker beside it. A `load` is `painter.detach()` → drop
the old view → drop its group (ending the Lynx-main Worker and then the
worker-realm one) → new group and view → `painter.attach(&view)`, and the
canvas is deliberately *not* resized along the way: it already carries the
right resolution, and setting a canvas's size clears its bitmap, blanking the
previous page's last frame. `BobcatRenderer::pump` stays one method and takes
both turns in order, the painter's first. That Worker also spawns its group's
Rayon style Workers with `wasm_thread`, leaving the vendored Stylo sources
unchanged. Core creates its owner-thread-bound QuickJS realm inside that
Worker; Element-PAPI batches, Stylo/Rayon, layout and render hand-off
synchronize through Rust channels, mutexes, atomics and the shared Wasm memory
exactly as natively. JavaScript `postMessage` is only the browser host boundary
(initial Canvas transfer, URL-based script requests/results,
resize/input/lifecycle) or a library's Worker bootstrap control plane; it is
not a DOM/render reconciliation protocol. URL requests are serialized, and a
lost-wake-safe `EventSignal` Promise wakes script completion independently of
Worker rAF, so a hidden page may pause drawing without stranding the `load`
Promise. The UI facade, nested VM Worker startup and built-in QuickJS
configuration impose no wall-clock deadline: QuickJS drains its owned pending
jobs and waits for the TLA boot module's evaluation Promise to settle at its
host checkpoint, and there is no browser microtask-completion protocol. The
bridge retains an opt-in execution timeout for its direct users and tests.

A Wasm instance owns nothing of Stylo's but the Worker bootstrap
`configure_wasm_workers` installs — one script URL, which every Worker a group
spawns is made of — while each `LynxGroup` owns its Lynx-main Worker,
worker-realm Worker, style Workers and both QuickJS runtimes, and each
`LynxView` its own realm, document and endpoints, as natively. Every public
`BobcatCanvas` gets a separate Render Worker and Wasm instance; a renderer
holds neither group nor view until `BobcatRenderer::load` builds both, and each
later load replaces them. A page gets a group of its own rather than reusing
the renderer's, because the script runtime is the group's: a page loaded twice
would otherwise register its entry module a second time under a name the
previous load already took. Dropping the view stops it and dropping its group
ends the Lynx-main Worker and then the worker-realm Worker, once the Lynx-main
Worker has released the document and thread-bound QuickJS realm; ending is all
it is on this target, since `panic=abort` leaves a trapped Worker never
signalling its join handle, so wasm teardown says the goodbye and does not
wait. Replacement construction starts only after that teardown. The transferred
OffscreenCanvas, module instance, configuration, latest metrics, resource
provider, registered font containers, selected default font family, and Stylo
worker *count* are the renderer's own, reapplied to each group it builds, while
the workers belong to the group and retire with it. Registered script and
stylesheet bytes remain available until the startup outcome arrives; cleanup
leaves ZIP assets and the next page's staged sources intact. The Render Worker
is not a pool member; the group's Lynx-main Worker is index zero of the pool it
builds, and the rest are managed Workers it spawns. `BobcatRenderer::create`
therefore takes a count of one to `MAX_STYLE_THREADS`, counted the way
`StyleThreads` counts everywhere — the Lynx-main Worker included — and the
facade asks for the machine's threads less the Render Worker. The UI never
blocks, while Worker-side Rust may block wherever the native runtime does. The
browser target enables `parking_lot_core/nightly` so transitive Stylo/wgpu
parking_lot locks use Wasm atomic wait/notify instead of the non-atomic backend
that panics on contention.

Release packaging pins Binaryen 132 through the JavaScript workspace and runs
`wasm-opt -Oz` after wasm-bindgen with an explicit mirror of every enabled
Rust/LLVM Wasm feature; the build rejects a different optimizer version instead
of accepting wasm-pack's older fallback. Package verification requires the
optimized module to omit its debugging `name` section while retaining
`target_features`. Browser builds disable Parley's `complex-scripts` feature to
avoid embedding ICU's multi-megabyte CJK and Southeast Asian dictionaries;
native targets retain it, so grapheme segmentation, shaping and ordinary
Unicode line breaking remain available while Thai, Khmer, Lao and Myanmar text
may use cluster-level emergency breaks and report a larger intrinsic minimum
width. `wasm_thread` is pinned to the upstream `spawn_from_worker` change,
whose crates.io release otherwise forwards nested spawns to a parent protocol
handler an explicit embedder Worker does not have; Chrome 135 supports the
resulting nested module Worker.

Page sources arrive through the Render Worker's own `fetch`: it registers the
raw stylesheet and entry-MTS bytes with the `bobcat-resources` system it owns
and calls `BobcatRenderer::load(entry_url, style_sheet_urls)`; the entry's
final response URL is the ESM specifier `bobcat:boot` imports and the base its
images resolve against. That raw entry is neither a Lynx nor a web-core
product, so it is registered verbatim, with no `MTS_CHUNK_PREAMBLE`: a raw
main-thread script imports what it uses from `bobcat:runtime` and
`bobcat:element` itself. Images are fetched by the resource system through the
same Worker `fetch` and decoded in the Render Worker too, by the browser's own
`createImageBitmap` and `ImageDecoder` (`BrowserDecoder` above); nothing about
an image crosses to the main thread. `loadLynxXml(url)` fetches an
XML envelope once, decodes it with the web loader's replacement-mode UTF-8
behavior, parses it with `bobcat-source::xml`, registers any raw stylesheet
and its main-thread body — prefixed with `MTS_CHUNK_PREAMBLE`, as `bobcat-source`
registers every card body — and hands them to the same `load`; both are
repeatable. The exported `LYNX_XML_PAGE_CONFIG` names the source format's
fixed page defaults, which a host may still deliberately override. The optional background body is
registered, as the `CommonJS` chunk web-core runs it as, at its section URL,
`<final-response-URL>#background-thread`, and named in
`ViewSources::background_entry` for the view's BTS Worker.

Transferring the canvas does not transfer its DOM event target, so the
`BobcatCanvas` facade retains that element and forwards active
`pointerdown`/`pointermove`/`pointerup`/`pointercancel` sequences. It claims
each accepted pointer, maps client coordinates through the canvas bounds into
viewport CSS px, and sends compact fire-and-forget records through the same
ordered Render-Worker queue as load/resize. The Worker stamps input with its
own `performance.now()` before `BobcatRenderer` writes the shared manual clock
and calls `LynxView::dispatch_input`, which keeps gesture time on the Worker
rAF timeline and prevents an idle frame clock from making `longpress` fire
immediately. Each load clears active captures, disposal removes all listeners
and restores the canvas's prior inline `touch-action`, and unexpected capture
loss becomes `pointercancel`.

`wheel` crosses too, through the same queue and the same
`BobcatRenderer::dispatchWheel` shape as a pointer. The facade normalizes the
delta to viewport CSS px: `deltaMode` 0 is page CSS px and takes the same
canvas-box scale the position does, `deltaMode` 1 is 40 CSS px per line (the
`WHEEL_LINE_CSS_PX` `bobcat-cli`'s macOS host uses), `deltaMode` 2 is the
viewport's own width and height. The sign is the browser's, which already
means "scroll offset increases" the way core's does — the CLI negates only
because winit's is the opposite. A ctrl-held wheel is the browser's zoom
gesture and is exempt: nothing is forwarded and nothing is prevented.
Everything else is forwarded and unconditionally `preventDefault()`ed, because
the Worker answers nothing and the facade cannot learn synchronously whether
the engine consumed the scroll — the canvas owns wheel scrolling outright, the
way `touch-action: none` makes it own touch panning. A browser mouse is
reported truthfully by the facade and fed to the engine as
`PointerKind::Mouse` by `dispatchPointer`, as the macOS host does, and a
primary-button drag still scrolls: the engine's drag recognizer latches every
pointer kind, a mouse included (user ruling 2026-10-13). Hover moves and secondary mouse buttons do not cross the boundary.

**Text input goes through a hidden `<textarea>`** (`js/text-input.ts`,
`docs/editable-text-design.md` E4). `BobcatRenderer::pump` hands the newest
`EngineEvent::TextInputStateChanged` of a turn to the Render Worker's
`postTextInputState` (the last `BobcatRenderer::create` argument) as one
plain object, posted as `bobcat-text-input`; a load that replaces a view with
a host focused posts one with `host: false`. The facade keeps one transparent,
`position: fixed` `<textarea>` at the engine's caret, focused while a host is
focused, holding its value and selection (never written during a
composition). Every `beforeinput` outside a composition is prevented and sent
as a `bobcat-edit` (`insertText`/`insertFromPaste` → `insert`, the line
breaks, the deletions, `deleteByCut` → `deleteBackward`); a composition is
`setComposition` per update, read at the `input` after it, and at its end a
`finishComposition` (the end's text is the last update's) or a `commitText`. `BobcatRenderer::edit` reads the flat call back into
`terminus::link::editing::Command` (`src/text_input.rs`, natively tested) and drops it
while the view cannot take edits. Every view is set to
`TextInputSource::Commands`, so a key carries no text and its `keydown` only
moves and selects all; the facade prevents the textarea's own handling of
those keys and sends a key the input method is composing with as
`'Process'`.

**Native modules are Rust; `globalProps` is the page's.** A view's
`NativeModules` are `impl bobcat_core::NativeModule`, which
`BobcatRenderer` hands every view it builds (`native_modules` in
`src/browser.rs`) and which run inside `LynxView::pump` on the Render Worker;
the facade takes no JavaScript handlers. The one module is `ExplorerModule`
(`src/explorer.rs`), compiled with the `explorer-module` cargo feature, off
by default and turned on by `scripts/build.ts` for the one package
`pnpm --filter bobcat-wasm build` makes, so the GitHub Pages demo and
`scripts/render-check.ts` both have it; a build without the feature gives a
view no module. It declares the Lynx Explorer homepage's methods
(`openSchema`, `openScan`, `setThreadMode`, `openDevtoolSwitchPage`,
`saveThemePreferences`, `saveToLocalStorage`, `readFromLocalStorage`,
`getSettingInfo`), reads each call's JSON arguments itself, logs
`setThreadMode`/`openDevtoolSwitchPage` on the Render Worker's console,
answers nothing, and sends the page three typed messages through
`postExplorerMessage` (a `BobcatRenderer.create` argument) for what needs the
page: `bobcat-explorer-open-schema` (`schema`, the card's string, never only
white space), `bobcat-explorer-store` (`key`, `value`, each ECMAScript's
`String()` of the argument) and `bobcat-explorer-notice` (`emptySchema` for an
`openSchema` without a URL, `noScanner` for `openScan`). Its logic is written
over an `ExplorerPage` trait so the native test build checks it; the
`BrowserExplorerPage` that posts the messages is browser-only. The facade
turns each message into an `ExplorerRequest` (`kind` `openSchema`, `store` or
`notice`) for `BobcatCanvas.create`'s `onExplorerRequest` callback, the shape
`onPerformanceEntry` has: a message whose fields are not the protocol's is
dropped with a warning, a callback that throws is `console.error`ed, and a
canvas without the callback drops them. `load`/`loadLynxXml`/`loadTemplate`/
`loadZip` each take an optional `{ globalProps }`, JSON-stringified on the
facade and written into `ViewSources::global_props` unread.
`packages/github-pages` passes `onExplorerRequest` for the
`@explorer/homepage` bundle it loads first: an `openSchema` request resolves
an absolute URL, a relative path, or `file://lynx?local://<path>[?query]`
against the document base and runs the page's own template load. Those local
paths have targets because the Pages build also publishes
`@explorer/showcase`'s menus as `showcase/menu/<name>.web.bundle` and each
`@lynx-example/<category>` it depends on as the package's whole `dist/` under
`showcase/<category>/`, both bundle flavours and the `static/` assets a demo
names relative to itself; the category list comes from the showcase's
dependencies and the packages are found through a `createRequire` rooted at
its manifest, pnpm's layout keeping them under its own `node_modules`. A
`.lynx.bundle` local path is loaded as the `.web.bundle` beside it, retrying
the named file once if that rejects — which is how the demos shipping only a
source-based `.lynx.bundle` open, while a bytecode one fails both. A `store`
request writes `localStorage` under a `bobcat-explorer:` prefix, and the two
notices set the upload status. The page passes **no `globalProps`**
deliberately: web-core's Explorer hands its view a `theme`, but the homepage
also reads `screenWidth`/`screenHeight` and flips to a two-column landscape
layout when the width exceeds the height, which is the wrong shape for this
preview canvas — with the field absent the page keeps its portrait
single-column layout and default theme.

The facade exposes no
create/append/drop/flush, document, tree, or engine API, and does not decode
`.web.bundle` containers; callers supply `PageConfig` and either executable
script URLs or a raw Lynx XML URL. Synchronous GPU capture is absent because
browser WebGPU completion is Promise-driven.
