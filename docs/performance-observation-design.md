# Performance observation: design

Design proposal, 2026-10-23, revised the same day under the rulings in §11.
Goals, in the order the user stated them:

1. The BTS realm offers the Lynx-compatible API (`lynx.performance`,
   `PerformanceObserver`, the pipeline hooks ReactLynx calls).
2. `terminus` offers behavior and interfaces as close to the browser
   standards as its scope allows, with no Lynx vocabulary.
3. The Rust API (`bobcat-core`) has the capability of Lynx native's platform
   API, shaped as idiomatic Rust rather than a port of the Java/ObjC classes.
4. No runtime cost on a page that observes nothing, and no work on the main
   thread for a consumer that is not the main thread.
5. The numbers are comparable with Lynx native's: same entry names, same
   key names, same formulas, same clock semantics.

The survey of what Lynx native has (engine services, platform APIs, JS API,
tracing) is the memory note `lynx-native-perf-observation-survey`; §1 repeats
only what this design builds on.

## 1. What is being matched

Lynx native's current metrics system is one callback,
`onPerformanceEvent(PerformanceEntry)`, fed by `TimingHandlerNg`
(`lynx/core/services/timing_handler/timing_handler_ng.cc`). An entry is
`{entryType, name, ...attributes}`; the attributes are camelCase keys holding
Unix timestamps in milliseconds with microsecond precision (`1739594612307.429`),
or `PerformanceMetric {name, duration, startTimestampName, startTimestamp,
endTimestampName, endTimestamp}` records. The truth for every entry's shape is
`lynx/tools/performance/performance_observer/definition_yaml_files/*.yml`.

The entries that are not deprecated:

| entryType.name | Produced by | Built here |
| --- | --- | --- |
| `pipeline.loadBundle` (`LoadBundleEntry`) | the first-screen pipeline | phase 1 |
| `pipeline.reloadBundleFromNative`, `pipeline.reloadBundleFromBts` (`ReloadBundleEntry`) | a reload | phase 1 |
| `pipeline.updateTriggeredByBts`, `.updateTriggeredByNative`, `.updateGlobalProps`, `.setNativeProps`, `.reactLynxHydrate` (`PipelineEntry`) | a flush bound to a `__lynx_timing_flag` | phase 1 |
| `resource.lazyBundle` | a lazy bundle load | phase 2 |
| `container` (W3C Container Timing, no Lynx equivalent) | a presented frame that grew a `containertiming` root's painted region | phase 2 |
| `memory.memory` | the memory monitor, when enabled | phase 2 |
| `js_blocking` | the JS blocking monitor, when enabled | phase 2 |
| `metric.fsp` | the First Stable Paint tracer, when enabled | phase 3 |

Deprecated in Lynx and **not built** (ruling Q2 covers the last three): the
standalone `init.container`, `init.lynxview`, `init.backgroundRuntime`,
`metric.fcp` and `metric.actualFmp` entries (their fields live on
`LoadBundleEntry` / `ReloadBundleEntry` / `PipelineEntry` since SDK 3.7, and
the C++ NG path emits none of them), `metric.tti` (gone since 3.3), the old
Timing API (`onTimingSetup`/`onTimingUpdate`,
`lynx.performance.addTimingListener`, snake_case keys; "no longer maintained
starting from Lynx 3.1"), and `nativeApp.markTiming(flag, key)`.

Lynx's long-task, fluency and JSB timing data are not performance entries:
they are event-reporter events (`lynxsdk_long_task_timing`, …). §7 says what
of that is built.

### 1.1 The browser standards each Lynx mechanism corresponds to

| Lynx | W3C | Relationship |
| --- | --- | --- |
| `PerformanceObserver`, `PerformanceEntry {name, entryType}` | Performance Timeline L2 | Lynx copied the shape; `observe()` takes names, not `{entryTypes}` |
| `PipelineEntry` stages (`mtsRender`, `resolve`, `layout`, `paint`) | Long Animation Frames (`renderStart`, `styleAndLayoutStart`) | the same "rendering update" phases, Lynx's with both ends and more cuts |
| `__lynx_timing_flag` → `PipelineEntry.identifier` | Element Timing (`elementtiming` attribute → `renderTime`) | both mark an element and report when the frame that includes it is presented |
| a `<frame>`'s card as the unit of timing (its own LynxView) | Container Timing (`containertiming` attribute → `container` entries per subtree root) | the standard's unit is a marked subtree, not a document or a shadow tree (§6.5) |
| `fcp`, `lynxFcp`, `totalFcp` | Paint Timing `first-paint` | Lynx's "fcp" is the first commit presented, regardless of content: the standard's `first-paint`, not its `first-contentful-paint` |
| `__lynx_timing_actual_fmp` | Largest Contentful Paint, loosely | author-declared instead of heuristic |
| `fsp` | none (closest is visual-completeness tooling) | — |
| long task (`≥ 50 ms`) | Long Tasks L1 (`longtask`, 50 ms) | same threshold |
| `js_blocking` | Long Animation Frames `blockingDuration` | queue delay rather than frame delay |
| `memory` | `measureUserAgentSpecificMemory()` | both are user-agent-specific breakdowns |
| `lynx.performance.profileStart/End/Mark/FlowId` | User Timing L3 (`mark`/`measure` with `detail`) | web-core already maps them onto User Timing |
| JS CPU profile in a trace | JS Self-Profiling API | Lynx drives it from DevTool, not from the page |

The design is layered so that `terminus` implements the W3C column and
`bobcat-core` derives the Lynx column from it (goal 2 and goal 1 at the same
time, with no second clock and no second set of marks).

## 2. The one data flow

```mermaid
flowchart LR
  subgraph BTS["bobcat-workers (BTS realm)"]
    RL["ReactLynx<br/>_generatePipelineOptions / _onPipelineStart /<br/>_markTiming / _bindPipelineIdWithTimingFlag"]
    OBS["lynx.performance.createObserver<br/>takePerformanceEntries() on wake"]
  end
  subgraph MAIN["bobcat-main (MTS realm + document)"]
    FL["__FlushElementTree(page, {pipelineOptions,<br/>__lynx_timing_flag})"]
    CT["terminus CommitTiming<br/>style / layout / frame build<br/>per commit_id"]
  end
  REG["PipelineRegistry<br/>(one per view, Arc&lt;Mutex&gt;,<br/>records keyed by pipeline id)"]
  subgraph PAINT["embedder thread (Painter + LynxView::pump)"]
    PR["adopt + compose + present<br/>writes presented(commit_id, at)"]
    PUMP["pump(): assemble PerformanceEntry<br/>for the Rust embedder"]
  end
  RL -- "host functions, Instant::now()" --> REG
  FL -- "binds pipeline → commit_id,<br/>copies CommitTiming (six Instants)" --> REG
  FL --> CT
  CT -- "Arc&lt;CommittedFrame&gt; (watch)" --> PR
  PR -- "present time" --> REG
  PR -- "one WorkerMessage, only while<br/>a BTS observer exists" --> OBS
  REG -- "completed records" --> PUMP
  REG -- "completed records,<br/>flat (key, value) list" --> OBS
```

**Nothing is assembled or serialized on the main thread** (ruling Q1). The
main thread's two writes are measurements: the binding of a flush to its
`commit_id` and a copy of six `Instant`s out of `terminus`. Every entry is
built on the thread that consumes it: the embedder's, inside `pump()`, and
the BTS worker's, inside the job its observer's wake runs. The painter,
which already runs on the embedder's thread and already holds the view's
seat, is what completes a record (it knows the present time) and what wakes
the BTS. The main-thread epilogue gains no step.

## 3. Clock and timestamps

**One monotonic clock, converted once.** Marks are `ClockInstant`
(`std::time::Instant` natively, `web_time::Instant` on Wasm — the pair
`crate::clock` already names). They are taken on whichever thread the event
happens on; `Instant` is comparable across threads of one process, which is
what makes a BTS-side `diffVdomStart` and a painter-side `paintEnd`
subtractable. A view records, at construction and beside the
`timeline_epoch` it already keeps, one `SystemTime` reading taken at the
same moment. Every entry handed out converts
`unix = epoch_unix + (instant - epoch_instant)`.

**Lynx shape at the boundary.** The Rust type is
`Timestamp(ClockInstant)` with `Timestamp::unix_millis(&self, epoch) -> f64`
(ms with three decimals, Lynx's `GetAllTimingInfoAsMillisecond`), and the
Lynx field list (§6.1) writes that `f64`. Lynx's `setExtraTiming` inputs are
Unix milliseconds from the host, so `ExtraTiming` takes `SystemTime` and is
converted into the view's instant space on the way in. Native's
`hostPlatformTiming` is Android-only; this engine reports its own painter
stages under the same field with `hostPlatformType: "vello"` (§5.2).

Why not `performance.now()`-style view-relative milliseconds: Lynx's
`totalFcp = paintEnd − openTime` subtracts a host timestamp taken before the
view existed; only an absolute clock makes that comparable.

## 4. Zero cost when nothing observes

Lynx's own rule is kept: a pipeline carries timestamps only when
`PipelineOptions::need_timestamps` is set, which `_bindPipelineIdWithTimingFlag`
or a `__lynx_timing_flag` attribute sets, plus the load and reload
pipelines, which always do. Everything else below is behind that bit.

- **`terminus`**: `Document::set_commit_timing(bool)` is one field.
  `render()` reads it once; when `false` it takes no instant and the
  `CommitTiming` slot keeps `None`. When `true` it takes six `Instant::now()`
  readings into a fixed-size `Copy` struct on the document, no allocation.
  Six readings are tens of nanoseconds natively and under a microsecond on
  the Wasm `performance.now()` path; a commit is milliseconds.
  `bobcat-core` turns the field on for a flush that binds a pipeline needing
  timestamps and off again after it, so an unflagged flush reads one `bool`.
- **The flag's attribute path**: `__SetAttribute(el, '__lynx_timing_flag', v)`
  already crosses into Rust per attribute; the name test joins the attribute
  setter's existing dispatch on the name, no new branch on the general path.
- **BTS hooks**: ReactLynx calls `_generatePipelineOptions` once per update
  and `_markTiming` only when `needTimestamps` (its own guard,
  `core/performance.js` `markTiming`). `_generatePipelineOptions` answers a
  fresh id from an `AtomicU64` and nothing else; the registry allocates a
  record only on `_onPipelineStart` of a pipeline that will carry timestamps.
  For an unflagged update that is one atomic increment and one host call
  returning a small object.
- **Painter**: after a present it compares the adopted `commit_id` with the
  registry's `oldest_waiting_commit` (one relaxed atomic load); only when a
  record waits does it lock, complete the record and, if a BTS observer
  exists (one `AtomicBool`), send one `WorkerMessage`. No new channel, no
  new wakeup on a page that flagged nothing.
- **Consumers**: `pump()` reads one generation counter; the BTS worker runs
  one job per wake, which drains every completed record its realm opened at
  once.
- **Container Timing**: one node bit per painted item on a page with no
  `containertiming` attribute; the region union and the ancestor walk exist
  only under a marked root.
- **Monitors** (§7) are off unless the Rust embedder turns them on, as
  Lynx's `enable_long_task_timing`, `enable_memory_monitor` and
  `enable_js_blocking_monitor` are.
- **Tracing** (§8) is a cargo feature, absent from a normal build.

The CodSpeed bench `crates/bobcat-core/benches/script_boundary.rs` gains two
cases: an update flush with no flag (must not move) and one with a flag
(measures the whole observed path). The unflagged case is the gate for goal 4.

## 5. `terminus`: the standard-shaped primitive

`terminus` knows nothing of pipelines, flags or Lynx names. It offers, on the
link (`docs/terminus-link-api.md` rows to add):

### 5.1 `CommitTiming`

```rust
pub struct CommitTiming {
    pub commit_id: u64,
    pub style_start: Instant, pub style_end: Instant,
    pub layout_start: Instant, pub layout_end: Instant,
    pub frame_build_start: Instant, pub frame_build_end: Instant,
}
impl<T> Document<T> {
    pub fn set_commit_timing(&mut self, enabled: bool);
    /// The timing of the last `render()` that built a frame, if timing was on.
    pub fn last_commit_timing(&self) -> Option<CommitTiming>;
}
```

`style_*` brackets `flush_styles_with_damage_sink` across the container
passes (`layout_pass` runs it per pass; the first pass's start and the last
pass's end are reported, matching Lynx's `resolveStart/End`, which bracket
the whole style resolution), `layout_*` brackets `run_layout_once` the same
way, `frame_build_*` brackets `build_frame_with_relevance` through
`painter.paint` (paint-order build and scene encode). The names are the
Long Animation Frames vocabulary (`styleAndLayout`, split because this
engine can) rather than Lynx's.

### 5.2 Presentation time and paint timing on the painter

`terminus::link::painter::Painter` reports, after `finish_draw` (window) or
`draw_offscreen` returns:

```rust
pub struct PresentedFrame { pub commit_id: u64, pub compose_start: Instant, pub compose_end: Instant, pub presented: Instant }
impl Painter {
    pub fn last_presented(&self) -> Option<PresentedFrame>;
    /// Paint Timing's two marks, as the painter saw them: the first frame it presented, and the first one that put ink on the viewport.
    pub fn first_paint(&self) -> Option<PresentedFrame>;
    pub fn first_contentful_paint(&self) -> Option<PresentedFrame>;
}
```

`presented` is the instant the present call returned (ruling Q5): after the
draw is submitted, not after the display shows it. Lynx marks `paintEnd` on
Android's draw callback and iOS's transaction commit, neither of which waits
for the display either, so the two numbers have the same meaning.
`compose_*` brackets adopt-and-compose through GPU encode; `bobcat-core`
places it under Lynx's `hostPlatformTiming`.

`first_paint` and `first_contentful_paint` carry the standard's names and
meanings (ruling Q6): the first presented frame, and the first presented
frame whose paint order holds contentful ink as Paint Timing defines it: a
non-empty text run at non-zero opacity, an image whose pixels are resolved
(an `<image>`, a background image, a vector image), an SVG with rendered
content — not a background color or a border, which count for
`first_paint` only (`CommittedFrame::paints_content`, decided during the
paint-order build, which already classifies every item, so no extra walk).
Lynx's `fcp` is `first_paint`; the renaming is `bobcat-core`'s (§6.3), and
`terminus` never spells `fcp`.

### 5.3 Container Timing: timing for one subtree

The standard's answer to "when did this part of the tree paint" is WICG
Container Timing (`https://wicg.github.io/container-timing/`; Chrome origin
trial 148–153, Firefox 156 behind a flag): a `containertiming` attribute
marks a subtree root, every paint inside the subtree is attributed to its
nearest marked ancestor, and a `container` entry
`{identifier, rootElement, firstRenderTime, startTime, size,
intersectionRect, lastPaintedElement}` is queued at a presentation whose
paints grew the root's painted region; `containertimingignore` cuts a
subtree out; recording stops at the first scroll or interaction. The draft
leaves shadow DOM and nested containers undefined.

`terminus` builds it as the primitive, in its own vocabulary:

```rust
pub struct ContainerPaint {
    pub root: NodeId,
    pub commit_id: u64,
    /// Device-space union of everything painted under `root` so far, and its area.
    pub painted: Rect, pub size: f32,
    pub first_render: Instant, pub latest: Instant,
    pub last_painted: NodeId,   // retargeted to `root`'s nearest shadow host when inside a shadow tree
}
impl<T> Document<T> {
    pub fn set_container_timing(&mut self, root: NodeId, enabled: bool);
    pub fn set_container_timing_ignored(&mut self, node: NodeId, ignored: bool);
}
impl Painter {
    /// The containers whose painted region grew in the last presented frame.
    pub fn take_container_paints(&mut self) -> Vec<ContainerPaint>;
}
```

Attribution runs in the paint-order build over the flat tree, the tree the
build already walks: a node bit `self_or_ancestor_has_container_timing`
is set when a root is marked and propagated at insertion (Blink's
`SelfOrAncestorHasContainerTiming`), so a page without a marked root pays
one bit test per painted item and nothing else; a painted item under a root
walks up to its nearest marked ancestor and adds its device rect to that
root's region (a `Region` union, as Blink's `cc::Region`). The painter
reports a root when a presented frame grew its region. Two deviations from
the draft, recorded in `docs/tracking/deviations.md`: nested roots are
defined (the nearest marked ancestor wins), and a shadow tree is inside
its host's container because the flat tree is what paints — the "shadowed"
model the Web Performance WG leaned to on 2025-09-11
(`w3c/element-timing#3`), with `last_painted` retargeted to the host.

### 5.4 Not in `terminus`

Element Timing (`elementtiming`) is not needed for Lynx's flag: Lynx binds a
flag to the *flush* it was set in, whether or not the element paints, so the
binding is a `bobcat-core` fact (flush → `commit_id`). A W3C
`PerformanceElementTiming` primitive can be added later over
`CommitTiming` and the paint-order build without touching this design.
Note that the standard does not expose an element inside a shadow tree at
all (`exposed for paint timing` requires the element's root to be the
document; WPT "Do not expose elements in Shadow Trees"), which is one more
reason the subtree primitive is Container Timing rather than Element Timing.

## 6. `bobcat-core`: the Lynx model and the Rust API

### 6.1 Types

```rust
// bobcat_core::performance (public module)
pub struct Timestamp(/* ClockInstant */);
impl Timestamp { pub fn unix_millis(&self, epoch: &TimeOrigin) -> f64; pub fn duration_since(&self, earlier: Timestamp) -> Duration; }

pub struct Metric { pub name: &'static str, pub start: (&'static str, Timestamp), pub end: (&'static str, Timestamp) }
impl Metric { pub fn duration(&self) -> Duration; }

pub enum PipelineOrigin { LoadBundle, ReloadBundleFromNative, ReloadBundleFromBts,
    UpdateTriggeredByBts, UpdateTriggeredByNative, UpdateGlobalProps, SetNativeProps, ReactLynxHydrate }

pub struct PipelineTiming {           // Lynx PipelineEntry's attributes
    pub origin: PipelineOrigin,
    pub identifier: Option<Arc<str>>, // the __lynx_timing_flag; None for load/reload
    pub pipeline_start: Timestamp, pub pipeline_end: Timestamp,
    pub mts_render: Option<Range<Timestamp>>,
    pub resolve: Range<Timestamp>, pub layout: Range<Timestamp>,
    pub painting_ui_operation_execute: Range<Timestamp>,   // frame build on main
    pub layout_ui_operation_execute: Range<Timestamp>,     // compose on the painter
    pub paint_end: Timestamp,
    pub framework_rendering_timing: FrameworkTiming,       // dsl, stage, marks
    pub host_platform_timing: PainterTiming,
    pub actual_fmp: Option<ActualFmp>,                     // when identifier == "__lynx_timing_actual_fmp"
}
pub struct FrameworkTiming { pub dsl: Option<Arc<str>>, pub stage: Option<Arc<str>>, pub marks: Vec<(Arc<str>, Timestamp)> }

pub struct LoadBundleTiming {         // Lynx LoadBundleEntry = PipelineTiming + these
    pub pipeline: PipelineTiming,
    pub url: Arc<str>,                                     // the bundle loaded; Lynx carries it in LynxPipelineInfo, not in the entry
    pub load_bundle: Range<Timestamp>, pub parse: Range<Timestamp>,
    pub vm_execute: Option<Range<Timestamp>>,
    pub load_background: Option<Range<Timestamp>>, pub load_core: Option<Range<Timestamp>>,
    pub create_lynx: Range<Timestamp>,
    pub extra: ExtraTiming,                                // from LynxView::set_extra_timing
    pub lynx_fcp: Metric, pub fcp: Option<Metric>, pub total_fcp: Option<Metric>,   // from terminus first_paint
    pub first_contentful_paint: Option<Timestamp>,         // terminus's, not in Lynx (Q6)
}
pub struct ReloadBundleTiming { /* the reload analogue */ }

#[non_exhaustive]
pub enum PerformanceEntry {
    LoadBundle(Box<LoadBundleTiming>), ReloadBundle(Box<ReloadBundleTiming>),
    Pipeline(Box<PipelineTiming>),
    /// W3C Container Timing, from `terminus`'s `ContainerPaint`; `identifier` is the root's `containertiming` attribute.
    Container(ContainerTiming),
    LazyBundle(LazyBundleTiming), Memory(MemoryUsage), JsBlocking(JsBlocking), LongTask(LongTask),
}
pub enum LynxValue<'a> { Number(f64), Text(&'a str), Metric(&'a Metric), Map(&'a [(&'a str, LynxValue<'a>)]) }
impl PerformanceEntry {
    pub fn entry_type(&self) -> &'static str;   // "pipeline", "resource", …
    pub fn name(&self) -> &str;                 // "loadBundle", "updateTriggeredByBts", …
    /// Lynx's attributes in Lynx's names, in a fixed order: the one place `paintingUiOperationExecuteStart` is spelled.
    pub fn lynx_fields(&self, epoch: &TimeOrigin) -> impl Iterator<Item = (&'static str, LynxValue<'_>)>;
    /// `{entryType, name, …}` from `lynx_fields`, for an embedder that reports or compares; never called by the engine.
    pub fn write_lynx_json(&self, epoch: &TimeOrigin, out: &mut String);
}
```

Fields instead of maps, `Range<Timestamp>` instead of `*Start`/`*End`
pairs, an enum instead of `entryType` strings, `Option` where Lynx leaves a
zero. The Lynx names survive in exactly two places: the `PipelineOrigin`
variants (they are the `name` values) and `lynx_fields`, which both the JSON
writer and the BTS binding (§9) iterate, so the key table exists once and
no engine thread serializes anything.

### 6.2 Registration and delivery

```rust
bitflags! { pub struct EntryTypes: u8 { const PIPELINE; const LOAD_BUNDLE; const CONTAINER; const RESOURCE; const MEMORY; const JS_BLOCKING; const LONG_TASK; } }
impl<F> LynxView<F> {
    /// Which entries `pump` reports as `EngineEvent::Performance`. Default: empty (ruling Q3): nothing is assembled until an embedder asks.
    pub fn observe_performance(&self, types: EntryTypes);
    pub fn set_extra_timing(&self, timing: ExtraTiming) -> Result<(), EngineError>; // Lynx setExtraTiming; write-once per key
    pub fn set_long_task_monitor(&self, config: Option<LongTaskConfig>);
    pub fn set_memory_monitor(&self, config: Option<MemoryMonitorConfig>);
    pub fn set_js_blocking_monitor(&self, config: Option<JsBlockingConfig>);
}
pub enum EngineEvent { /* … */ Performance(PerformanceEntry) }
```

Delivery is `pump()`, as every other engine fact is: the embedder polls on
its own thread and no engine thread calls into it. `pump()` assembles the
`PerformanceEntry` structs from the registry's completed records there, on
the embedder's thread, and only for the types subscribed; an unsubscribed
type's records are released at completion. `ExtraTiming` is Lynx's five
container keys as `Option<SystemTime>`; setting it after the load pipeline
completed re-emits the `LoadBundle` entry with `fcp`/`totalFcp` filled, as
Lynx does.

### 6.3 Where each Lynx key is marked

| Lynx key | This engine's point | Thread |
| --- | --- | --- |
| `createLynxStart/End` | enter/leave `LynxGroup::create_lynx_view` | embedder |
| `loadBundleStart` | the job that evaluates the entry begins (sources resolved) | main |
| `parseStart/End` | `.web.bundle` decode in the installer (`bobcat-source`), per load | fetcher's |
| `vmExecuteStart/End` | the entry module's evaluation (top-level await settled) | main |
| `mtsRenderStart/End` | `__RenderPage` dispatch begins → `__FlushElementTree` is called | main |
| `loadCoreStart/End` | BTS worker realm opened → `bts.ts` ready (`initialize` handled) | workers |
| `loadBackgroundStart/End` | BTS entry import begins → settles | workers |
| `pipelineStart` | BTS `_onPipelineStart`; for native-triggered pipelines the command's arrival on main | workers / main |
| `resolveStart/End`, `layoutStart/End` | `CommitTiming.style_*`, `.layout_*` | main |
| `paintingUiOperationExecuteStart/End` | `CommitTiming.frame_build_*` | main |
| `layoutUiOperationExecuteStart/End` | `PresentedFrame.compose_*` | painter |
| `paintEnd`, `pipelineEnd` | `PresentedFrame.presented` of the first presented frame whose `commit_id ≥` the pipeline's | painter |
| `loadBundleEnd` | the flush that committed the first frame returned | main |
| framework marks (`diffVdomStart`, `patchChangesStart`, …) | BTS/MTS `_markTiming(id, key)` | the caller's |
| `lynxFcp` | `first_paint.presented − loadBundleStart` | derived |
| `fcp`, `totalFcp` | `first_paint.presented − prepareTemplateStart`, `− openTime` | derived, needs `ExtraTiming` |
| `actualFmp`, `lynxActualFmp`, `totalActualFmp` | the flagged pipeline's `paintEnd` minus the three load origins | derived |

The gaps against native are recorded rather than faked: `loadBackground*`
and `loadCore*` are `None` on a view whose BTS failed or was never started;
there is no `verifyTasm*`/`ffi*` (no bytecode verification, no FFI);
`parseStart/End` is `None` in phase 1, because the `.web.bundle` decode runs
inside the embedder's fetcher (`bobcat-source`'s installer), which reports no
marks to the view, so `LoadBundleTiming::parse` stays empty until the fetcher
grows a way to report its decode span; and a reload's
`reloadBackgroundStart/End` is `None` in phase 1 (the BTS's half of a reload
is not marked yet), so a `ReloadBundle` entry completes at its present
without waiting for it, and reports no `fcp`/`totalFcp`.

As built in phase 1, a load completes only once its first frame is
presented and its realm's BTS has marked `loadBackgroundEnd` (Lynx's own
readiness rule, `timing_info_ng.cc` `GetPipelineEntry`), which is what lets
the observer a BTS entry creates at its top level receive the load entry; a
BTS that ends before that releases the wait.

### 6.4 The pipeline registry

One `PipelineRegistry` per view, `Arc<Mutex<Records>>` plus three atomics
(the id counter, `oldest_waiting_commit`, `bts_observing`), shared by the
view's MTS realm, its BTS worker, its painter seat and `LynxView` itself.
Writers take `Instant::now()` before the lock and hold it for one insert or
one field write. The map is bounded like Lynx's `timing_map_exceeded_size`
(1000): a pipeline that never reached a flush is dropped at the bound,
oldest first.

A record's life:

1. **Opened** by `_onPipelineStart` (BTS or MTS) or, for a native-triggered
   pipeline, by the command's arrival on main: `origin`, `dsl`, `stage`,
   `pipeline_start`, and the realm that opened it (a `WorkerKey` for a
   BTS worker, the MTS realm or one of its `ShadowRealm` children) — a fact
   about who ran the script, not about the tree.
2. **Bound** by the flush that commits it: pending flags and the
   `pipelineOptions` the flush was called with get the commit's id and the
   `CommitTiming` copy. A flush that produced no new frame (nothing dirty)
   binds to the frame already committed, as Lynx's
   `has_dispatched_timing_flags_` path does.
3. **Completed** by the painter after the first present whose `commit_id`
   covers the binding: `compose_*`, `presented`. The painter then sends one
   `WorkerMessage::PerformanceReady` to the BTS worker of the realm that
   opened the record, when that worker's `bts_observing` bit is set.
4. **Taken** by each audience: `pump()` for the Rust embedder, the BTS
   worker's job for its observers. A record is freed when every audience
   that existed at completion has taken it.

### 6.5 Nested cards (`<frame>`), as Lynx has them

In Lynx native a `<frame>`'s card is its own `LynxTemplateRender`
(`LynxFrameView.java:112`, `LynxFrameView.mm:49`) with its own
`PerformanceController` and its own BTS runtime. So:

- the child card's BTS `PerformanceObserver` receives only the child's
  entries, and the parent's only the parent's — neither runtime can observe
  the other;
- the host's `LynxViewClientV2` on the root view receives only the root
  card's entries; the child's render has one client, `FramePerformanceClient`
  (`platform/android/.../frame/FramePerformanceClient.java`, iOS
  `LynxFrameView (LynxViewLifecycleV2)`), which forwards exactly the child's
  `pipeline.loadBundle` entry to the parent card as a component event
  `loadmetrics` on the `<frame>` element, detail
  `{url, mode: "embedded" | "standard", entry: <the entry's raw map>}`
  (`LynxFrameView.java:456-477`).

This engine renders a child card under the parent's page in one commit, and
a `<frame>`'s shadow tree is a shadow tree like any other: no entry carries a
card, and nothing in `terminus` or the registry knows what a card is
(ruling Q4). What the browser platform offers instead, and what this design
uses:

- **Per-document timelines** are the browser's analogue of Lynx's
  per-LynxView scoping: each iframe has its own `performance` object and
  Paint Timing is per document, so a parent and a child frame never observe
  each other's paints. That is the model this engine does not have, and
  does not build.
- **Pipeline provenance** is a script fact: a record remembers the realm
  that opened it (§6.4 step 1). The BTS worker that takes records takes
  those its own realm opened, which reproduces Lynx's isolation between a
  child card's observers and the parent's without the tree knowing. The
  Rust embedder's stream carries every record of the view; a `LoadBundle`
  entry names its bundle `url`, which is how an embedder tells a child
  card's load from the root's.
- **Container Timing** (§5.3) is the standard's unit for "this part of the
  tree": an author who wants a child card's paint timing writes
  `<frame containertiming="child">` and observes `container` entries, in
  the BTS (`observe(['container'])`, W3C field names) or in Rust
  (`EntryTypes::CONTAINER`). The frame's shadow tree paints inside its
  host's container because the flat tree is what paints; the same attribute
  on a `<view>` times that view's subtree the same way.
- **Lynx's `loadmetrics`** stays as a component event the `<frame>`
  component fires, with the child realm's completed `LoadBundle` entry as
  its `entry` detail (`lynx_fields`), when the element carries that
  listener. It is Lynx compatibility at the component layer, not a timing
  concept.

Other platform attribution mechanisms were checked and do not apply: Long
Animation Frames attribute scripts to a `Window` (`windowAttribution`), not
to a subtree; Soft Navigations attribute paints to a `navigationId`;
Element Timing and Largest Contentful Paint expose no element inside a
shadow tree, and `w3c/element-timing#3` (open since 2019, last discussed
2025-09-11) has settled on nothing beyond preferring a container-style,
host-level view for component users.

## 7. Monitors (phase 2)

All off by default; each is a `LynxView` setter taking `Option<Config>`.

- **Long tasks**: `jobs.rs` runs every piece of JavaScript as one job. With
  the monitor on, the loop takes two instants per job and reports a job of
  `≥ threshold` (default 50 ms, Lynx's) as `LongTask { start, duration,
  kind: JobKind, thread: Main | Workers }`. `JobKind` is the job's own enum
  (entry, timer, command, event, posted, module callback), which is the
  attribution Lynx's `task_type`/`task_name` give; W3C's `longtask` has
  `name`/`startTime`/`duration` and is what `entry_type()` answers
  (`"longtask"`). The record is written into the registry by the job loop
  and taken by `pump()`; Lynx's `lynxsdk_long_batched_tasks_timing` (a 1 s
  window at 80 % busy) is not built; the per-job entries let an embedder
  compute it.
- **Memory**: on the interval Lynx uses (`memory_report_interval_sec`), the
  main thread reads `JS_ComputeMemoryUsage` for the MTS realm, the BTS worker
  its own, and the document sizes itself (`TreeArenas` slots, `TextStore`s,
  the committed frame's scenes) — the three Lynx categories
  `mainThreadScriptingEngine`, `backgroundThreadScriptingEngine`,
  `lynxTasmElement`, plus `committedFrame` as this engine's own. Each writer
  puts its number in the registry; `pump()` emits when the total moved by
  more than `change_threshold` since the last report. The readings are the
  measurement; nothing else runs on the measuring thread.
- **JS blocking**: with the monitor on, each command the BTS consumer queues
  carries its enqueue instant; the job that runs it adds
  `run_start − enqueued` above `threshold` (default 20 ms) to a running sum
  in the registry, which `pump()` emits per `report_interval` (default 5 s)
  as Lynx's `{stage: loadBundle | timer, total_blocking_time,
  total_blocking_count, total_duration, blocking_ratio, avg_blocking_time}`.
- **Lazy bundle**: `resource.rs`'s lazy bundle load already has request and
  decode points; the entry is `{componentUrl, size, sync, loadSuccess,
  requireStart/End, decodeStart/End}` and costs two instants per lazy load.
- **First Stable Paint** (phase 3): the painter has, per presented frame,
  the paint order and the viewport, so Lynx's algorithm (meaningful-content
  rects projected onto 256-bit X and Y masks, "valuable" when fill ≥ 30 %,
  "stable" when consecutive masks differ under the pixel and area rates for
  `min_diff_interval_ms`, `hard_timeout_ms` 10 s, cancelled by user
  interaction) runs on the painter thread over presented frames only while
  enabled. The entry is `metric.fsp` with Lynx's status and fill fields.
  Deferred until phase 1 and 2 land and an embedder asks for it.

## 8. Tracing and the `profile*` API

A cargo feature `trace` on `terminus` and `bobcat-core`, off by default, adds
`tracing` spans at the points Lynx's `TRACE_EVENT`s name where the names
transfer (`Timing::Mark.<key>`, the commit's style/layout/paint spans, job
runs). The subscriber is the embedder's: `tracing-chrome` or
`tracing-perfetto` writes a file the Perfetto UI opens next to a Lynx
`.pftrace`, which is the comparability goal 5 asks for at the trace level.
Without the feature the macros are not compiled in; with it and no
subscriber, each span is one dispatcher check.

`lynx.performance.profileStart(name, {flowId, args})`, `profileEnd()`,
`profileMark()`, `profileFlowId()`, `isProfileRecording()` on both realms
become host functions that emit `tracing` events under target `javascript`
(Lynx's category), with `flowId` as a span field; `isProfileRecording`
answers whether a subscriber is enabled for that target. In a build without
the feature they stay the no-ops `main-thread-runtime.ts` has today, and the
test `the performance shell must stay inert` keeps holding for that build.
`console.profile`/`profileEnd` map to the same. A sampling JS CPU profiler
(Lynx's `JSProfileInterval`) is out of scope: upstream QuickJS has none.

## 9. The BTS surface (goal 1)

`bts.ts` gains `lynx.performance` with native's current members:

| Member | Implementation |
| --- | --- |
| `createObserver(cb) → PerformanceObserver {observe(names), disconnect()}` | pure TS, filters `entryType` / `entryType.name`; entries are emitted as `lynx.performance.onPerformanceEvent` on the realm's `GlobalEventEmitter`, so `lynx.getJSModule('GlobalEventEmitter').addListener(...)` also works, as on native. `observe(['container'])` adds W3C Container Timing entries (`entryType: "container"`, the draft's field names), which native has no equivalent of |
| `_generatePipelineOptions()` → `{pipelineID, needTimestamps: false}` | host function (`PipelineRegistry` id) |
| `_onPipelineStart(id, options)` | host function; opens the record with `pipelineStart`, `pipelineOrigin`, `dsl`, `stage`, `needTimestamps` |
| `_markTiming(id, key)` | host function; one instant into the record's marks |
| `_bindPipelineIdWithTimingFlag(id, flag)` | host function; sets `needTimestamps` |
| `profileStart/End/Mark/FlowId/isProfileRecording` | §8 |
| `nativeApp.generatePipelineOptions/onPipelineStart/markPipelineTiming/bindPipelineIdWithTimingFlag` | the same four, under the `nativeApp` names |
| `addTimingListener`, `removeTimingListener`, `removeAllTimingListener`, `nativeApp.markTiming` | **not built** (ruling Q2): a call fails at the missing member |

Entry delivery to the BTS, with no main-thread part and no JSON (ruling
Q1): the first `observe()` with a non-empty list calls the host function
`observePerformance(true)`, which sets the registry's `bts_observing`
(the last `disconnect()` clears it). The painter, on completing a record of
that card, sends `WorkerMessage::PerformanceReady` to the worker; the job it
runs calls `takePerformanceEntries()`, a host function that, on the worker
thread, drains the card's completed records and answers one flat array per
entry — `[entryType, name, key1, value1, key2, value2, …]`, primitives only,
nested metrics and maps as nested arrays — from `lynx_fields`. `bts.ts`
folds each into the entry object and emits it. That follows AGENTS.md's
JavaScript-data-ownership rule (primitive binding arguments, object built in
JavaScript) while keeping the key table in Rust. Lynx sends to the BTS
whenever a background runtime exists; here nothing is sent until an observer
exists, which is what makes an unobserving card free.

The MTS realm's `lynx.performance` keeps `_generatePipelineOptions`,
`_onPipelineStart` and `_markTiming` (ReactLynx's MTS patch path marks
`mtsRenderStart/End` through them) backed by the same registry, and
`__FlushElementTree(page, options)` reads `options.pipelineOptions` and
`options.__lynx_timing_flag`, binding them to the commit it makes.
`__SetAttribute(el, '__lynx_timing_flag', v)` appends `v` to the registry's
pending flags, bound at the next flush. The `__lynx_timing_flag` key in
`updateData`/`reload` payloads from the host binds the native-triggered
pipeline the same way. The MTS realm gets no `createObserver`: Lynx's
open-source build delivers entries to the MTS only in Air mode.

## 10. Phasing

1. **Phase 1 — pipelines**: `CommitTiming`, `PresentedFrame`,
   `first_paint`/`first_contentful_paint` in `terminus`; `PipelineRegistry`,
   the painter's completion step, `PerformanceEntry::{Pipeline, LoadBundle,
   ReloadBundle}`, `EngineEvent::Performance`, `observe_performance`,
   `set_extra_timing`; BTS/MTS `lynx.performance` hooks,
   `PerformanceObserver`, `takePerformanceEntries`; the `<frame>`
   `loadmetrics` event; the two bench cases; a golden-style test that a
   flagged `<view>` yields one `pipeline.updateTriggeredByBts` entry whose
   `identifier` is the flag and whose stages are ordered.
2. **Phase 2 — monitors and Container Timing**: long task, memory, JS
   blocking, lazy bundle; the `containertiming` primitive and its
   `container` entries.
3. **Phase 3 — FSP and tracing feature**: `metric.fsp`; the `trace` cargo
   feature and the `profile*` host functions over `tracing`.

Each phase is one PR series; no stacking across phases.

## 11. Decisions

User rulings, 2026-10-23. All six questions are closed; nothing in this
design waits on a ruling.

1. **Q1, entry transport.** Neither JSON text nor primitive arguments built
   on the main thread: a consumer that is not the main thread gets no
   main-thread work. Built as §2/§6.4/§9: records complete on the painter's
   thread, assembly happens in `pump()` and in the BTS worker's own job,
   the BTS receives primitives in a flat array and builds the object in
   JavaScript.
2. **Q2, the legacy Timing API.** Not kept: no `addTimingListener`, no
   `onSetup`/`onUpdate`, no `nativeApp.markTiming`, no adapter.
3. **Q5, `paintEnd`.** The present call's return.
4. **Q6, paint timing.** `terminus` exposes the standard's `first_paint` and
   `first_contentful_paint`; Lynx's `fcp` is `first_paint` renamed in
   `bobcat-core` (or the element API's JavaScript), and
   `first_contentful_paint` stays on the Rust entry as this engine's extra
   field.

5. **Q3, default `EntryTypes`.** Opt-in: the default is empty, and an
   embedder that wants Lynx's data calls `observe_performance`. A
   comparison against native therefore starts with that one call.
6. **Q4, nested cards.** No `card` on any entry and no special treatment of
   a `<frame>`'s shadow tree: rendering does not distinguish cards. Timing
   for a part of the tree is W3C Container Timing (§5.3, §6.5), built as
   the `terminus` primitive and the `container` entry type; the BTS routing
   is pipeline provenance by realm, and the Rust stream carries every
   record of the view with `url` on `LoadBundle`. Ruled on the platform
   research in §6.5 (2026-10-23).
