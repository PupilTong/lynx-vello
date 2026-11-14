# bobcat-core: Painter, frames, pacing and images

Part of the agent guide for `crates/bobcat-core`; the index
[crates/bobcat-core/AGENTS.md](../AGENTS.md) lists the other topics, and the
root [AGENTS.md](../../../AGENTS.md) holds the mission, the standards and
dependency policies, the workspace map, the toolchain and testing. Paths in
backticks are relative to the repository root.

The core depends on `terminus` and re-exports one seam: the `input` module
republishes `terminus::link::Point2D` and
`terminus::link::input::{InputEvent, InputKind, PointerId, PointerKind, PointerPhase}`.
Wheel deltas there are viewport CSS pixels; converting physical-pixel, line or
page units is embedder policy. No document, node or hit-test result crosses.

**`Painter` is a standalone public object, not the view's**:
`Painter::new(DrawTarget, width, height, device_pixel_ratio)` builds one over a
target before any view exists, `attach(&view)` points it at a view, `detach()`
releases it. It holds only a watch receiver and a `Weak` on the view's seat
(the view's command sender plus its handle on the host's resource system, as
one releasable thing), so a view dropped under a painter leaves it showing and
capturing its last frame. One interactive painter per view, one *live* view per
painter: a second `attach` is `EngineError::PainterAttached`, refused by the
seat's weak count, which `Painter::attach` is the only place to downgrade; a
seat whose view is gone needs no `detach`. Attaching drops everything derived
from the previous view — adopted snapshot, scroll intents, gesture arena,
resolved pixels, the target's compose key, since commit ids restart at one
per document — rebases the frame clock onto the view's timeline
epoch, seeds the frame-post sequence past what has been serviced, and writes
its metrics into the seat's watch: **the painter owns device metrics**, and
that write is also what binds the view. Detaching resets the same minus the
target and leaves the watch alone, so the last frame stays up and capturable
while the next page loads and no later flush parks.

Every entry point first polls the link, adopting the newest `Published` with
the bitmaps it draws before noticing a view that has gone, so a commit published
in the release turn is still adopted. A commit whose bitmaps could not be
described in the same step — there was no store to ask — is not adopted, because
a frame indexes its store's bitmaps by draw order. The painter retains the newest frame and runs input routing,
gestures, compositor scrolling, composition and presentation inside the
embedder's own calls; vsync touches the OS only there. Commands needing the
live tree go to `bobcat-main`, which answers by publishing a later frame, so a
long JavaScript task cannot stop scrolling or re-presentation; only commits
publish, so a half-applied batch is unobservable. A scroll recomposes the
retained frame without a commit; the painter posts its offsets into the view's
`ScrollMailbox`, main adopts them at the marker with `scroll_to`, and recommits
only past a slot's `ScrollSlot::encode_window` or when the committed slot finds
the offset `recenter_due`. Embedders provide input, device metrics, OS initialization, a draw
target and IO primitives, and relay OS facts in
(`Painter::{dispatch_input, resize, set_occluded, refresh, pump, tick, capture}`
and `LynxView::pump`); they never start or steer the pipeline. Engine events are
enqueued and wake the host through the group's `EventRequester` for the next
`LynxView::pump`: `ScriptFinished` (entry-module boot), `StartupFailed`
(source/configuration/boot failure), `ScriptRunError` (main-thread app code
that threw, or a host call into the realm that failed — the entry's
evaluation, a module load, a `Future` settle, a page update, an animation
frame — during boot or after it), `ListenerFailed` (a listener that threw
during event delivery), `TimerFailed` (a `setTimeout` or `setInterval`
callback that threw when it came due), `WorkerThrew` (code in a worker's
realm threw, BTS included, and the worker still runs; a worker script or
imported module that cannot be loaded is reported here too), `WorkerEnded` (a
worker ended without being told to: its realm
could not be built, or the worker thread trapped), `Panicked` (the engine
panicked while it served the view), and the two diagnostics, `ScriptReported`
(a `lynx.reportError`, at level `"warn"`, `"error"` or `"fatal"`) and
`ConsoleMessage` (at the console method's name), each carrying the
`ScriptSource` of the realm that made it; a `"fatal"` report is a diagnostic
like the others and ends nothing. The three script failures are separate by
the kind of entry they happened in, also where the code that failed was a
continuation that entry resumed, and none of them is fatal: the realm goes
on, the walk continues, a repeating timer stays armed, and later events and
timers are delivered as normal. The two worker events are not fatal either;
each carries the worker's `ScriptSource` (`Worker { id, url }`, whose `url` is
`bobcat:bts` for the BTS),
is reported before the creating realm's `Worker` object dispatches its `error`
event, so no listener there suppresses it, and is reported only while the
script still holds that worker's key: never after `terminate()`, and never for
`close()`. `EngineEvent::is_fatal` names the events that end the view
(`StartupFailed` and `Panicked`); `LynxView::pump` ends a view on exactly
those, and an embedder asks it rather than matching variants. Every panic is
reported through one constructor, `EngineEvent::from_panic`, which the MTS
table's Panic row in `realm/policy.rs` names, from the driver's `trapped` (a
job or a task of the view, an input dispatch included), `finish_view` (the
group thread reaping the view's task) and the Wasm panic hook. A host member
that panics is among them: `ScriptEngine` catches the panic before the bridge
would turn it into the exception "the host function panicked", shows the
script that exception, and resumes the panic at the realm's next checkpoint,
normally the one that ends the entry that called the member, so catching the
exception does not stop it and the view ends as it does on Wasm. A worker's
host member that panics ends that worker the same way (`WorkerEnded`). A frame
the engine wants drawn rides the same wakeup, and the `Painter::pump`
answering it draws it.

**A host takes two turns per wakeup, and they are different calls.**
`LynxView::pump` alone advances the resource protocol past the startup sources
`create_lynx_view` already handed over: it hands each
`RequestSource` to the fetcher, gives it its `service_images` moment, names
every source the last paint walk discovered, drains the image inbox back to
`bobcat-main`, and returns the turn's lifecycle events — after a fatal event,
nothing further. `Painter::pump` draws the frame owed and asks the host for
nothing, so a host that wants an image to arrive takes the view's turn. Pacing
is the embedder's and the engine names no interval for it: after each turn
`owes_frame` answers whether a frame is still owed — a running animation, a
swap chain that had no image to give, a commit the turn did not draw — and the
host takes it at **its own next display frame**, whatever its display clock is
(a `CVDisplayLink` on the window's monitor, `requestAnimationFrame` in a
Worker). `is_animating` is the narrower fact, answered for any target, that an
offscreen host asks instead. **A realm timer is not the host's to wait out**:
every live realm, a view's and a worker's alike, has a `serve_clock` task
holding one pinned sleep on that realm's next deadline, re-armed only when the
deadline moves and fed by the watch that realm's epilogue publishes. Natively
that sleep is tokio's own time driver; on wasm32, where tokio's reads
`std::time::Instant` and would panic, `src/alarm.rs` serves it — one
process-wide `bobcat-alarm` Worker holding a heap of deadlines and the wakers
waiting on them, parked with `park_timeout`, started by the first
`LynxGroup::new` on the embedder's thread — and `crate::clock::sleep_until`
picks between the two by `cfg`. **One clock across threads**: every instant
is `terminus::link::clock::ClockInstant`, which `crate::clock` re-exports.
Natively it is `std::time::Instant` and nothing else; on wasm32 each Worker's
`web_time::Instant` starts from its own `performance.timeOrigin`, so
`bobcat-main`, `bobcat-workers` and `bobcat-alarm` each align their clock to
the embedder thread's as their first statement (one NTP-style exchange, which
`LynxGroup::new` answers with `SpawnerClock::answer`) and add that offset to
every later reading. A thread that must be aligned is spawned by the
embedder's thread: a Worker does not start until its creator returns to its
event loop, which an engine thread parked in its scheduler never does. A
draw that fails is the return value of
`Painter::{pump, tick, capture}`, reported once because there is no recovering
a lost surface; there is no `RenderFailed` event.

**The draw target is an argument to `Painter::new`, named once and kept for
that painter's whole life**: `DrawTarget::window(...)` takes anything
convertible into `WindowTarget` — a `'static` surface target, so a windowing
embedder passes a shared handle (`Arc<winit::Window>`) and a browser an owned
canvas — and `DrawTarget::Offscreen` asks for a windowless GPU target. Either
is built inside `Painter::new`, on the thread that will draw into it, the only
thread macOS lets a surface be created from. No target is attached later and no
painter has none; what a painter points at later is a view. An offscreen target
is refused on Wasm at construction rather than hanging, since building one
blocks the calling thread on a device request that thread's own event loop
would have to answer. `FrameSize::for_viewport` exposes the physical size
`Painter::new` and `Painter::resize` will compute, for a host that must size a
canvas's backing store first.

**Images are entirely the embedder's**; core fetches, decodes, caches and
retains no pixel of its own. A view's one resource system — its
`ResourceFetcher`, which is also its `terminus::link::FrameImages`, owned by the
`LynxView` as an `Rc` and read by an attached painter through the view's seat —
is asked for one image at a time by source string (the `url(…)` value CSS
produced, or a replaced element's source): named through `request_image`,
answered through `ImageReports` with the intrinsic size layout needs, given its
moment in every `LynxView::pump` through `service_images` (where a host whose
loads complete off-thread forwards them into the reports), and asked
synchronously when a painter adopts a commit, with a
`terminus::link::ImageSizeHint` — the largest device-pixel extent the frame
draws that source at, computed per draw under its transform and unioned per
source — so a host decodes to the draw rather than to the file. **A drawn
bitmap has one copy.** The painter first asks what each source draws as
(`FrameImages::describe`, which must not block on a decode): the bitmap's
`ImageDescriptor` — blob id, size, layout, what the rasterizer keys an upload
on — with its pixels when the store holds them. It keeps those pixels only
until the rasterizer holds its upload — one per bitmap, whatever the samplers
the frame draws it with; then it takes a lease on that upload, drops its
reference and tells the store it may drop its own (`FrameImages::release`). A leased upload
is not released under the GPU byte budget, so the frame — a dropped view's
last one included — stays drawable with no pixel on the CPU; the lease ends
when the painter adopts a commit that no longer draws the source, forgets the
view or drops. `FrameImages::read`, the call that may block on a restore, is
made only for a source whose pixels the store dropped and whose upload the
rasterizer does not hold: a source drawn again after its lease ended and its
upload was released, or a render that found neither an upload nor the pixels it was lent, after which
the next adoption describes the commit again. `describe` and `release` have
default bodies — `describe` is a `read`, `release` drops nothing — because
keeping every bitmap is a complete policy; a store with only `read` and
`retain` behaves as before. No container
sniffing, codec contract, cache policy or byte budget lives in `bobcat-core` or
`terminus`: the sizing vocabulary (`ImageSizeHint`, `Bitmap`, `target_size`) and
every cache policy are `vant`'s, which both re-export, and
`crates/bobcat-resources` is the reference orchestration over `vant` that every
shipped embedder uses. What the engine fixes about a host's image store is
`terminus::link::ImageResidency` (re-exported at the `bobcat-core` root):
`vant::image::residency::Residency` over `ImageReports`, which supplies
`MAX_RENDERABLE_DIMENSION` and sends every report itself, once per
transition, so a host runs the IO and cannot forget or reorder a report.
`FrameImages::retain` — the sources one resolve pass met, in
paint order — carries **no default body**, so every store writes its working
set where someone can see it rather than inheriting a silent no-op.
`LynxView::prefetch_images` warms sources ahead of the walk that would discover
them.

**An SVG document is parsed by the host, where it decodes bitmaps**
(`docs/svg-lynx-component-design.md`, revision 4.1, ruled 2026-10-10). A
fetched SVG is parsed by the fetcher with the engine's own parser,
`terminus::link::ImageEvent::parse_document` (re-exported as `ImageEvent`; the converter
in `terminus`'s `render/svg`, which shapes no text), and reported through
`ImageReports::parsed_document(source, Arc<VectorDocument>)`, or `failed`
when it does not parse; the report applies through
`MainThreadRuntime::apply_image_events` like every other, where `terminus` shapes
the document's text and encodes its scene, so the registry, the natural-size
relayout and the `load`/`error` follow the path every other report takes.
The markup an `<svg content>` hands the document (`Document::set_image_document`)
reaches the host the same way: the page's epilogue calls
`MainThreadRuntime::settle_entry`, whose first step, before its commit, drains
`Document::take_document_requests` into one `ViewNotice::RequestDocuments`
beside `RequestImages`, and `LynxView::pump` hands each
`(source, bytes, kind)` to `ResourceFetcher::request_document` after
`service_images` and the image requests (nothing for a failed view; the
trait's default does nothing, so on a host without document support an
`<svg content>` stays pending). Nothing on `bobcat-main` parses, on any
target, and `bobcat-core` has no SVG parser dependency of its own. The
`JsThread` runtime runs no blocking work and is dropped plainly at group
teardown (`DetachingRuntime` went with the engine-side parse).
