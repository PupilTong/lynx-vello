# crates/bobcat-cli

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/bobcat-cli`. Paths in backticks are relative to the repository root.

## The `cli` feature

The native `bobcat` product over `bobcat-core`, depending on it plus
`bobcat-resources` and `bobcat-source`. `bobcat -i file:///…` content-sniffs
and boots one web bundle or one raw Lynx XML source card; other URL schemes are
rejected at the boundary.

The CLI is an **embedder** of the opaque `bobcat_core::LynxGroup`, `LynxView`
and `Painter`: it owns argument parsing, local input IO, the `PageSource`
instance, the reference resource system with the extracted scripts/styles
registered, the winit window and event loop, device metrics, input translation,
the stdin prompt, and PNG writing — and nothing of the pipeline. It builds the
view from the group and the painter from the window on its own thread and
attaches them. Every event handler is a relay into the painter
(`dispatch_input`, `resize`, `set_occluded`, clock ticks in headless mode), and editing goes to the view
(`LynxView::edit`).

Text editing (`docs/editable-text-design.md`, E3): the newest
`EngineEvent::TextInputStateChanged` of a turn turns the window's input
method on (`set_ime_allowed`) exactly while an editing host is focused and
places its candidate window (`set_ime_cursor_area`, the caret or the first
composition character, as logical points). A key carries winit's
`KeyEvent.text` as its typed text: winit 0.30.13 queues no `KeyboardInput`
for a key its input method consumed, and no `Ime::Commit` for one it did not,
so no text is inserted twice. `cli/ime.rs` (`ImeBridge`) turns `WindowEvent::Ime`
into `SetComposition` (winit's byte cursor as UTF-16), `CommitText` and, on
`Disabled` mid-composition, `FinishComposition`; it holds winit's empty
`Preedit` back until the next event, since one comes right before every
`Commit` and only an empty `Preedit` that no `Commit` follows (sent before the
next other window event, or in `about_to_wait`) cancels the composition.

The window it hands `Painter::new` is the draw target and nothing else: frames
and lifecycle events wake the event loop through the injected `EventRequester`,
and that turn ends in `about_to_wait`, taking both turns in order —
`painter.pump()` draws the frame owed, then `view.pump()` services resources
and returns what the realm had to say. Winit's `RedrawRequested` is not
relayed. Drawing there coalesces a turn's events into one frame and keeps the
vsync wait out of winit's proxy-event drain, which iterates until empty. The
painter goes first deliberately: the frame a view committed before a fatal
event reaches the screen on the turn that reports it. The loop always waits —
a realm timer is not its deadline to keep — and what wakes it for a *frame* is
the window's own display: while `Painter::owes_frame` holds, a `CVDisplayLink`
on the window's monitor posts one wakeup per refresh and stops when nothing is
owed.

It renders one page: one group, one `create_lynx_view` given the author CSS and
entry MTS URL as a `ViewSources`, and the preserved `ScriptFinished` edge
consumed through `view.pump()`. Only an event for which `EngineEvent::is_fatal`
holds ends the run: `StartupFailed`, like a group or view construction
failure, as `CliError::StartView`, and any other fatal event (`Panicked`) as
`CliError::Script`. Every other event is printed to standard error and the run
goes on: `ScriptRunError`, `ListenerFailed`, `TimerFailed`, `WorkerThrew` and
`WorkerEnded`, and the realms' diagnostics as `[{source}] [{level}] {message}`.
A `ScriptSource` prints as `main` or as `worker <id> <url>`, so the BTS's lines
begin `[worker <id> bobcat:bts]`.
Headed mode builds its painter over the window; headless builds one over
`DrawTarget::Offscreen` and turns each synthetic vsync tick into one frame
(`Painter::post_frame`, the view's `pump` on every wakeup until
`Painter::frame_serviced`, then `Painter::draw_posted`), whether a frame
becomes GPU work being the engine's decision. Fields drop in the
order `vsync, painter, view, …, window`, so the display link stops before what
it wakes goes away and the surface is released before the last window handle.

Its resource system is `bobcat-resources`: the decoded input's scripts and
stylesheets registered under the input's own `file://` URL for a container
(`<input>/root.js`, `<input>/style-info.css`, …) and under
`bobcat-memory://lynx-xml/` URLs for XML, the input's own `file://` URL the
base every relative `url(…)` resolves against, and a disk tier under
the user's cache directory, so a page's images — beside the input, inline as
`data:`, or on the network — load and decode through the platform. A load
completing on the fetcher's driver thread wakes the event loop exactly as a
commit does.

Headed mode uses a native winit window with display-backed vsync and tracks
both logical viewport size and device-pixel ratio; headless mode uses a
configurable synthetic vsync rate, skips catch-up bursts after slow frames, and
retains its rasterizer, render texture and staging buffer across frames.
Both expose a GDB-like stdin command prompt (`continue`, `pause`, `frame`,
`screenshot`, `help`, `quit`; headless also `set/show vsync`), with editing
commands that drive a page without a keyboard: `tap X Y`, `key KEY [TEXT]`,
`compose TEXT [START END]`, `commit TEXT` and `show text-input` (the last
published `TextInputState` as one line);
`tests/fixtures/editable.lynx.xml` is a page with one logging editing host. Screenshots are
captured only through that live prompt — there is no one-shot startup flag —
and PNG readback happens only on a screenshot.

It must not duplicate runtime, DOM, layout, painting, or source-lowering
policy: missing MTS/PAPI support remains a precise `bobcat-core` QuickJS error.
`bobcat-source` lowers a decoded `StyleInfo` into
`bobcat_core::PreparsedStyleSheet`, flattening every `css_id` fragment in
reverse-topological order so imported fragments precede their importers and
guarding each scoped fragment's selectors with `:where([l-css-id="N"])`, and
each native embedder registers that sheet in `bobcat-resources` under the URL
it names in `ViewSources::style_sheets`. For XML, a present `<style>` body instead uses the fetcher's raw
CSS-text arm and the fixed page configuration is `false`/`false`/`true` for
default linear display, visible overflow, and selector support; a present
background section is registered under the native in-memory URL
`bobcat-memory://lynx-xml/app-service.js` and named in
`ViewSources::background_entry`, so the view's BTS Worker imports and runs it.

## The `server` feature

The `bobcat-server` HTTP screenshot **embedder** in the same crate, not runtime
infrastructure inside `bobcat-core`. It follows UI Judge's public capture
surface: `GET /health` and multipart `POST /screenshot/lynxml`,
`/screenshot/template`, `/screenshot/template/url`, `/screenshot/zip/upload`,
and `/screenshot/zip/url`. There is no JSON `/screenshot` route. All routes
require a safe `entry` path and route-specific `source`, `url`, or `file` part.
Viewports default to 800×600 at DPR 1, accept dimensions up to 8192 with at
most 2,621,440 pixels, and return JSON `{"url":"..."}` with
`Cache-Control: no-store` after uploading the BMP and original request parameters
to TOS as `{prefix}/{uuid}/screenshot.bmp` and `screenshot.json`. The server
requires `TOS_ACCESS_KEY`, `TOS_SECRET_KEY`, `TOS_BUCKET`, and `TOS_REGION` at
startup; optional endpoint, prefix, token, and internal V1 authentication settings
match UI Judge. Uploads run asynchronously after encoding, outside the capture
thread, with a 100-second deadline; upload failures return sanitized 502 errors.
Stored BMP output matches UI Judge's top-down 32-bit
BITMAPV4HEADER/BI_BITFIELDS layout, preserving alpha without a second white
composite. Multipart fields share a 10 MiB bound plus 64 KiB framing and a
10-second upload deadline; remote URLs are bounded to 8 KiB.
`/screenshot/template` and `/screenshot/lynxml` accept `screenshotSettleMs`
(default 16) and `timeoutMs` (default 60000); the other routes use 500 ms and
60000 ms and reject timing fields. JSON/query parameters, duplicate or unknown
fields, and snake_case aliases are rejected. `initData` and `globalProps` must
be objects; `.lynxml` entries reject `globalProps` even when empty. Non-empty
page-data objects remain explicit 422 errors: the server does not forward them
to its views yet, though `ViewSources` takes both as JSON text. See
`crates/bobcat-cli/SERVER.md` for examples.

Axum accepts HTTP requests concurrently, but a bounded FIFO of eight waiting
jobs feeds one dedicated capture thread — the embedder thread for each job. It
starts a fresh `LynxGroup`, constructs its non-`Send` `LynxView`, builds a
`Painter` over `DrawTarget::Offscreen` beside it and attaches the two; both
stay on that thread, the view because it owns the host's resource system and
the painter because it owns the GPU target. It settles on a plain frame
interval, one frame per step (`painter.post_frame()`, `view.pump()` on every
wakeup until the view has serviced the post, `painter.draw_posted(…)`), and
returns its RGBA capture. Dropping that view releases its group, including the Lynx
main thread, QuickJS runtime and Stylo pool; no runtime is shared across jobs.
BMP encoding runs on Tokio's blocking pool after the view is gone, so it cannot
retain the view or hold the GPU lane. Queue saturation and an unavailable
worker are 503, input/render failures 422, encoding failures 500, and
capture/multipart upload timeouts 408. Of the engine's events, only one for which
`EngineEvent::is_fatal` holds fails a capture with 422: `StartupFailed` as a
start failure, `Panicked` as a script failure. A `ScriptRunError`, a BTS or
`Worker` realm's `WorkerThrew` or `WorkerEnded`, a listener or timer failure
and the realms' diagnostics are written to standard error and the page is
still captured. A capture-thread panic makes `/health` unavailable and
initiates graceful server shutdown.

Remote template/ZIP downloads follow UI Judge's public HTTP(S), no-credentials,
no-redirect policy, pin DNS results, and enforce 10 MiB/10-second bounds. XML
bytes and downloaded templates enter `PageSource`; archives use the shared
`ZipSource`, registering members at `zip:///` URLs in each job's resource
system without filesystem extraction. ZIP validation errors are 400;
unsupported source/rendering errors remain 422. Source-based native bundles
require a `root` module; real bytecode remains unsupported. It listens on all
IPv4 and IPv6 interfaces and has no auth, TLS, or CORS. Captures still require
trusted JavaScript: fresh groups on a capture thread do not provide UI Judge's
process isolation, and page subresources use the ordinary resource transport.
`timeoutMs` cannot preempt synchronous QuickJS execution, GPU driver calls, or
synchronous view teardown. Source fetching, HTTP policy, BMP encoding,
queueing, and server lifecycle stay outside core.

`crates/bobcat-source/tests/web_core_e2e.rs` boots that corpus without this
server — the cards reach their own bitmaps and fonts through `file:` URLs the
build bakes in — and holds each card to a rendering **someone read the case
for and judged right**. Every card is in one of two lists: `verified.rs`, whose
cards have a golden beside the test, and `pending.rs`, whose cards have a
reason instead and deliberately no golden, because a picture of an unjudged
rendering reads as a decision. Two invariants keep that honest:
`no_pending_case_has_a_golden` and `unlisted_cases_are_an_error`. Comparing
against web-core's own screenshots was tried and dropped: the two stacks
rasterize text differently and several cases are about behaviour an image only
indirectly shows, so a pixel distance to chromium answers a question nobody
asked. Accepting a rendering is `PRIME_RADIANT_UPDATE_SNAPSHOTS=1` on one test,
after the reading, in the same change that moves the card to `verified!`.

A first screen is not the whole of most cases, so `web_core_e2e/driven.rs`
restates, per card, what upstream's Playwright spec drives — a tap, a drag or
a wheel turn at a viewport point, `updateData`, `updateGlobalProps`, `reload`,
`sendGlobalEvent`, a wait for a console line, a fresh start where upstream
opens one card in several tests — and the suite pins one more
golden per frame of that script (`<case>.<label>.png`). A script is not a
verdict: a pending card may carry one and still has no golden. The harness
boots every card the way upstream's shell does
(`web-core-e2e/shell-project/index.ts`: page data, the two native modules,
`/dist/` and `/resources/`), and a card that renders wrong is checked against
that file before the engine is blamed — four separate omissions there each
made a family of cards test nothing until they were found.
