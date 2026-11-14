# crates/bobcat-resources

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/bobcat-resources`. Paths in backticks are relative to the repository root.

The cross-platform reference resource system: one `ResourceFetcher` for macOS,
Linux and the browser, which every shipped embedder uses. It is the worked
example of what the protocol expects, not part of the protocol. The policy it
applies is `vant`'s; what is here is the orchestration — `Resources<P:
vant::Platform>`, `ViewResources<P>`, the job executor, the image and source
pipelines, the background install holds — and the native reference adapters: `NativePlatform`
(`NativeFetch` over libcurl and `file:`, `FsStore` over a cache directory and
another over its `code` subdirectory, `NativeDecoder` over ImageIO/gdk-pixbuf).
The crate has no browser platform: it compiles for wasm32 as orchestration
alone (local tasks through `wasm-bindgen-futures`, the `Date` clock through
`js-sys`), and the browser's `Platform` is `bobcat-wasm`'s
(`crates/bobcat-wasm/AGENTS.md`, "The browser platform"). Natively `Resources::new`
builds `DefaultPlatform` (`NativePlatform`) with no persistent store, and
`Resources<P>` defaults `P` to it; on wasm32 there is no default and no
`Resources::new`, so an embedder names its platform. An embedder with a disk
cache, its own HTTP stack or its own storage builds a `Platform` and passes
it to `Resources::with_platform`. Five things live here and nowhere else in
the workspace.

**Transports**: contents the embedder registers under any URL, kept in a
`vant::registry::Registry<PreparsedStyleSheet>` per scope
(`Resources::register` and `register_style_sheet`, which are also its
`vant::registry::Register` implementation and add the stylesheet-cache
invalidation a registration change needs, or a `ContainerInstaller`'s
own `Registrar` for the sections of a container it recognized in a plain
`SourceRequest::Fetch`'s bytes — a decoded bundle's scripts, named sheets
and `StyleInfo` sheet, a browser-fetched script's bytes, a test's PNG), `data:`
URLs, `file:` URLs natively, and `http(s)` through the platform's own client:
libcurl loaded at runtime with `libloading` on macOS and Linux (no build-time
link, no bundled HTTP or TLS stack; a host without it gets a precise
`Unavailable`), and the Render Worker's `fetch` in the browser.

**Background installs**: `Resources::register_in_background(container,
install)` runs `install` with a `Registrar` off the embedder's thread (the
blocking pool natively, a local task in the browser) and holds every source
request for a URL whose path is the container's or under it until it has run;
a held load is then answered from the registry, a pre-parsed sheet directly,
or fails with the install's message, which is also noted. The running
installs are `Installs` (`registry.rs`), kept beside the registry in the
scope's `Transports` because they wait on a tokio `watch` channel, and
resolution accepts a held URL whatever its scheme. `Resources` implements
`vant::registry::RegisterInBackground` with this method, which is how
`bobcat-source`'s `DeferredPage` reaches it.

**The `FetchIndex`**: the base URL relative stylesheet, fetch and image URLs
resolve against by URL rules (a font URL arrives absolute, and so does every
script URL: an entry or a synchronous load, which is how a lazy container's
section is loaded, resolved against the view's `ViewSources::base_url`, which
this must therefore equal; a worker script against `__Card__` — a container
page's container URL, otherwise the entry's response URL; an import against
its importer's), and the
set of URLs a plain `SourceRequest::Fetch` has **completed successfully** for,
written after the container installer ran. It is behind an `Arc` of its own
rather than inside the shared state, because `ViewResources::fetch_probe()`
hands a reader of it to `bobcat-main` and `bobcat-workers` while everything
else here stays on the embedder's thread — on wasm32 the shared handle is an
`Rc`, and two mutexes and a set of URLs are `Send + Sync` on every target. A
new scope gets a fresh one, like the fresh registry beside it; a failed fetch
and a failed install are never in it.

**A MIME-keyed preprocessing pipeline**: every payload is sniffed (image magic
beats the label, a label beats a byte scan, a BOM names a charset), classified,
and treated by class — text transcoded to UTF-8 with its BOM removed so the
engine's strict validation sees what a browser's decoder would have produced,
JSON validated, images container-sniffed and header-probed for their intrinsic
size without decoding a pixel, the rest passed through.

**Tiered caching, applied from `vant`**: decoded bitmaps in the scope's
`ImageResidency` (`terminus::link::ImageResidency`, the engine's store type
over `vant::image::residency::Residency`) under a byte budget with every
view's working set pinned against eviction — a `ViewResources` carries its
`ViewKey` and releases its set when it drops; a bitmap whose upload a
painter's rasterizer holds leaves the tier when the painter says so
(`FrameImages::release`), is described by its descriptor from then on, and is
decoded again, off the painter's thread, only when a frame wants it
(`FrameImages::want`) because the rasterizer does not hold it either
— and fetched bytes in `vant::byte_cache::ByteCache`
over the platform's `Store` (`FsStore`: `<fnv64>.bin` beside `<fnv64>.json`
holding the key, length, last use and the HTTP record, temp-and-rename
writes, strays and broken halves swept at open) with RFC 9111 freshness,
`ETag`/`Last-Modified` revalidation, and the fetch cache modes mapped from
`CachePolicy`. A `Fetch` whose `owns_http_cache` is true — the browser's —
gets no byte tier in front of it; its HTTP cache plays that role, and
nothing it holds can be read back synchronously, so the image pipeline keeps
the encoded bytes of what it cannot restore. Stylesheet responses, including
pending loads and failures, are shared by resolved URL within a resource
scope. Preload hints populate that same cache; registration changes
invalidate the affected URLs.

**The code cache's Store side.** The engine reads and fills the process-wide
`vant::artifact::Artifacts` (`crates/bobcat-core/docs/realm-boot.md`, "The code cache"); this
crate puts persistence behind it when the platform's `Parts::artifacts` is
present, in a `CodeCache` shared by every scope of one system that drives a
`vant::artifact::ArtifactCache` over a `Store` or a
`vant::artifact::AsyncArtifactCache` over an `AsyncStore`. Two things happen.
A script is **preloaded** before it is delivered: every stored variant of
exactly that text is admitted to the memory tier, so the realm's synchronous
lookup hits; a text under `bobcat_core::MIN_CACHED_SOURCE_BYTES` is not
hashed. Over a `Store`, `sources::prepare` does it on the pool thread that
prepared the text; over an `AsyncStore`, the load's task awaits it after
`prepare` answered and before the completion is delivered, natively and in
the browser. `ResourceFetcher::service_images` **flushes** once per embedder
turn: when the tier has artifacts queued and no flush is in flight (one
atomic load otherwise), over a `Store` one blocking-pool job natively —
inline in the browser — writes them, and the last scope's drop writes what is
still queued; over an `AsyncStore` the flush is a task (the executor
natively, `spawn_local` in the browser) and nothing is written on drop, so
what was compiled after the last serviced turn is compiled again next time.
An `AsyncStore` is opened by a task started with the system; the first
preload or flush awaits that open. Natively the
Store is an `FsStore` at `<DiskCacheConfig::dir>/code` under
`DiskCacheConfig::code_budget_bytes` (64 MiB at the default location); it
opens and fails separately from the byte tier, each failure a note, and
`FsStore::scan` leaves directories alone so the outer Store never sweeps the
inner one. A system with no disk cache — `Resources::new` — has no Store, and
its realms still share bytecode through the memory tier for the life of the
process. Several systems or processes over one directory are safe: entries
are content-addressed and each file is written by temp-and-rename. A body
lands before its record, so another Store's scan that runs between the two
sweeps the entry as a broken half; that costs one later recompile and nothing
else, because the index is a hint and every hit is verified.

**Platform image decoding** (raster images; SVG is parsed, below): no codec
is compiled in — `ImageIO` on macOS
(`CGImageSourceCreateThumbnailAtIndex` with a maximum pixel size, so a photo
shown small is decoded small), gdk-pixbuf on Linux (loaded at runtime;
`gdk_pixbuf_loader_set_size` from the header probe), and the main thread's
`Image` element in the browser (the Render Worker fetches the bytes and hands
them over as a Blob), each asked to downsample during decode. Natively a load
is one task on the crate's own `current_thread` tokio runtime, built by
`Resources::new` and moved to a `bobcat-resources-driver` thread that drives
and shuts it down; its blocking pool (`max_blocking_threads = worker_threads`)
runs the transport read, the preprocessing and the decode, and a `Semaphore`
sized by `decode_parallelism` is acquired *before* a decode closure is
submitted, so a decode that has to wait holds no pool thread. A panic inside a
closure becomes that image's or source's reported failure. In the browser a
load is a local task instead. Either way completions are delivered through the
wakeup the embedder supplies and applied in the next `LynxView::pump` through
`service_images`.

**SVG documents are parsed here, in the decode's place**, on every target
(`docs/svg-lynx-component-design.md`, revision 4.1): once preprocessing
settles `ImageFormat::Svg`, the load runs the engine's parser
(`ImageEvent::parse_document`) through `images::parse_job`: natively under a
decode permit in a blocking closure of its own, in the browser inline in the
load's local task on the Render Worker, whose executor has no permits; the
platform decoder and the browser's `Image` element never see an
SVG. It completes as `Completion::ParsedDocument` and servicing reports it
through `ImageReports::parsed_document`; a document that does not parse is
an ordinary `Completion::Failed` with a note. `ViewResources::request_document`
(`images::request_document`) takes the markup of an `<svg content>` under the
synthetic `svg-content:` source the engine named and runs the same parse with
no resolution, transport or preprocessing; its completion
(`Completion::RequestedDocument`) is routed to the view that asked through
the residency's in-flight list of document requests. **Documents are parsed and
reported, never retained** (owner ruling 2026-10-11): the engine owns the
scene it encodes. A document request files no entry before, during or after
its parse, so two requests for one source are two parses and two reports
and `knows_image` stays false; a fetched document leaves only the
residency's parsed marker, so the URL stays known while a later request fetches
and parses it again. `is_resident` is false, `read` answers `None` (so the
memory tier, refinement and restore never see a document), and
`memory_used_bytes` counts no document. A document labelled with a specific
non-SVG type such as `text/plain` is trusted as that type and is not an
image (`mime::sniff`).

`images.rs` is the pipeline's IO alone: what a request, a describe or a want
means is the residency's answer, every report is sent by `ImageResidency`, and
this module starts the load, the refinement or the restore it is told to and
feeds the completion back.

The frame describes each image at the size it draws it, and nothing it asks
blocks: a resident bitmap far larger than its draw is re-decoded at the drawn
size in the background and replaced; one drawn larger than it was decoded is
refined back up while the image has more to give; and one that was evicted and
whose upload the rasterizer does not hold either draws nothing, is wanted
(`FrameImages::want`), and is restored from the retained bytes or the disk
tier as a refinement job — off the embedder's thread, under a decode permit —
whose completion wakes the host, so the next adoption draws it with no commit.
A restore that fails leaves the source with nothing to draw and is not
retried. The store answers URL pictures only (`PictureKey::Url`): every
generation it answers is zero, it has no `PictureKey::Id` picture and so no
frame to take, and it hands the residency the URLs of the drawn set it is told
(`FrameImages::retain`). The browser's decode runs on the main thread's `Image` element over a
mailbox whose two sides only signal through `postMessage`
(`crates/bobcat-wasm/js/image-decoder.ts` is the main thread's half); nothing
waits on it. See `docs/picture-source-design.md`.

Shape: `Resources` is the shared system (registry, caches, executor, decoder;
cheaply cloned, bound to the embedder's thread) and the only holder of the
executor, so the runtime shuts down — without waiting for work already picked
up — when the last clone of the last scope drops on that thread.
`Resources::builder` yields the per-view `ViewResources` that
`LynxGroup::create_lynx_view` takes, carrying that view's `ImageReports`.

**Animated images** (`animation.rs`, `docs/picture-source-design.md`
"Animated images"): a load of a container that may animate asks the decoder
for its frames (`ImageDecoder::frames_blocking`) inside the decode's closure,
under its permit; one with more than one frame keeps its encoded bytes and
gets a playhead under `vant`'s `PlayPolicy::stated`, one per URL per scope,
shared by every draw in every view. `describe` advances it and answers the
frame it is on (frame 0 is the residency's bitmap; frame *i* is decoded by
index at frame 0's decoded size, one decode in flight per source, under a
decode permit) at the playhead's step count as its generation, with the
instant the next frame is due as `next_change`. A describe is a draw; a
playhead stops when no view's `retain` set includes its URL and continues
from the same frame when one does again. While drawn a URL holds the frame
shown and the two after it (`AHEAD`), drops a frame passed, and drops the
frame shown on `release`; an animation of at most three frames keeps all of
them while drawn; held frames count in `memory_used_bytes`, not against the
bitmap budget. While a playhead runs the store arms a timer for its next
frame on its own executor — tokio's timer natively (the runtime is built
with `enable_time`), the Render Worker's `setTimeout` in the browser
(`executor::sleep`) — which completes as `Completion::Due` and wakes the
host; a frame due before it is decoded arms no timer, and its decode's
completion wakes the host. A frame decode that fails stops the playhead and
is noted. `ImageIO` answers every frame, from a source kept open per playing
animation (`decode/imageio.rs`, `Opened`: at most sixteen, keyed on the
`Bytes` allocation, closed by `ImageDecoder::forget_frames` when the store's
playhead stops), because a GIF frame decoded from a fresh source costs time in
proportion to its index; gdk-pixbuf and the browser's `Image` decoder answer
one, explicitly. The fixtures in
`tests/fixtures/animated/` are written by `tests/animated_fixtures.rs`, which
checks them byte for byte; the store's tests (`animation_tests.rs`) run over a
test platform whose decoder reads APNG with the `png` crate and a test clock
(`Shared::set_clock_hook`), and include a golden through a presenting painter
(`tests/screenshots/animated/`).

Recorded limits: an animated image animates only where the platform decoder
decodes frames by index (macOS today); the `<image>` element's own playback
surface (`loop-count`, `autoplay`, its methods and events) is not built, so
an `<image>` plays under the CSS policy; no `region-to-decode`, no `blur-radius` post-processing, and none of the
`<image>` element surface past `src` — the pipeline serves whatever source
string the paint walk names, today `url(…)` layers and the source an
`<image>` presents through `Document::set_picture`. The macOS
decoder is type-checked against the Apple target but exercised only where
ImageIO exists; the Linux decoder and libcurl transport are tested for real
against the system libraries, and the browser path is linted for wasm32 and
exercised only in a browser.
