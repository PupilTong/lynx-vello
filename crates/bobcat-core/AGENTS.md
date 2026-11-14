# crates/bobcat-core

Part of the agent guide; the root [AGENTS.md](../../AGENTS.md) holds the
mission, the standards and dependency policies, the workspace map, the
toolchain and testing. Read this file before reading or changing code under
`crates/bobcat-core`. Paths in backticks are relative to the repository root.
Its topics are listed under Topics below; read the ones a task touches.

The unified native runtime core. Source layout follows the ownership
boundaries. `main/` is everything on the Lynx main thread: `page.rs`'s view
page and what it adds to the realm driver, `quickjs.rs`'s script engine,
`runtime/` for realm integration, `workers.rs` for the `Worker` class, `tree/`
for Lynx page policy.
`background/` is the `bobcat-workers` thread and its worker realms. `view/` is
the public view facade, `paint/` the `Painter` facade: its link to one view
(the publication watch, the seat's mailbox, the events it sends to main) and
Lynx's gesture policy (`LYNX_GESTURES`) with the provenance of the scroll
physics. The engine half it wraps — the input router, the scroll kinematics
and the motions run over the scroll intents (Lynx's `autoScroll` among them),
the image protocol and the GPU target — is `terminus`'s `painter/`, reached
through `terminus::link::painter`. `link.rs` is
the one channel set a view spans its two threads with, `jobs.rs` the engine
thread itself — its scheduler and its job queue — `lifetime.rs` the task set,
token and latches a view and a worker are both built from, `realm/` the one
constructor every realm is opened with (`mod.rs`'s `open_realm`, which
installs the `RealmCore` members and then the host modules its caller
passes), a realm's `ShadowRealm` children (`child.rs`), the one driver a
view's page and a worker share (`owner.rs`) and the
table per realm kind their failures are reported by (`policy.rs`), `timers.rs`
and `clock.rs`/`alarm.rs` the timer machinery both realm kinds share,
`future.rs` the per-realm table the `Future` class is written over, `fetch.rs`
the one member a realm fetches a URL through, `esm.rs` the built-in module
table both runtimes register (`BUILTIN_MODULES`), `build_runtime` that
registers it, and the engine's module names and reserved prefixes, `script.rs`
the sanitized error a failure is reported with, `style.rs` the
`PreparsedStyleSheet` vocabulary, `resource.rs` the host protocol,
`performance/` the public `performance` module — the Lynx-shaped
`PerformanceEntry` types, the per-view `PipelineRegistry` every thread of a
view writes its marks into, the host members both realm kinds' `lynx.performance`
is written over, the two job monitors in `monitors.rs` (the long task
monitor `run_job` times every job for, and the JS blocking monitor over the
enqueue instants `WorkerMessage`s carry), and the memory monitor in
`memory.rs` (each script runtime and the document's element arena read by
its own thread on the monitor's interval); all three off unless the
embedder turns them on, and reported to it alone; `resource.lazyBundle`
entries, which `fetch.rs`'s `fetchResource` times (one relaxed load while
nobody observes `resource`, a `timed` flag on `SourceRequest::Fetch` and the
fetcher's `FetchOutcome` otherwise), reported to the embedder and to the BTS
of the realm that fetched; and each `<frame>` card's completed load, which
`LynxView::pump` writes as JSON and sends the card's realm as the frame's
`loadmetrics` (`ToMain::FrameLoadMetrics`, `bobcat:frame-boot`'s
`__BobcatLoadMetrics`); see
`docs/performance-observation-design.md`, and `main/runtime/pipelines.rs` for
the main-thread flush binding — and
`threads.rs` the two engine threads.

## Topics

- [Public surface and the two engine threads](docs/public-surface.md) — the
  public surface and the two engine threads (`jobs.rs`, `lifetime.rs`,
  `realm/owner.rs`).
- [Resource protocol, stylesheets and ESM loading](docs/resource-protocol.md) —
  the resource protocol, stylesheets and ESM loading (`esm.rs`).
- [Realm, document and boot](docs/realm-boot.md) — the realm (`open_realm`), the
  document, boot and the code cache (`code_cache.rs`).
- [Painter, frames, pacing and images](docs/painter.md) — the `Painter`, frames,
  pacing and images.
- [Workers, the BTS and cross-thread messages](docs/workers.md) — Workers
  (`main/workers.rs`), the BTS and cross-thread messages.
- [The host module, realm construction and ShadowRealm](docs/host-module.md) —
  the host module, `open_realm` (`realm/`) and `ShadowRealm` children
  (`child.rs`).
- [The frame element](docs/frame.md) — the `tree::frame` component and the
  `bobcat:frame`/`bobcat:frame-boot` modules.
- [The Element PAPI runtime](docs/element-papi.md) — the Element PAPI members
  that answer with a list, `__SetInlineStyles`, `__CreateList`, `__SetCSSId` and
  `Document::drop_element`.
- [Events, observers, exposure and layout events](docs/events.md) — events, the
  IntersectionObserver primitive, Lynx exposure (`main/exposure.rs`) and layout
  events (`main/layout_change.rs`).
- [Page policy: tags, text and the UA sheet](docs/page-policy.md) — Lynx page
  policy in the `tree` module (`tree/lib.rs`, `ua_sheet.rs`): tags, text and the
  UA sheet.
