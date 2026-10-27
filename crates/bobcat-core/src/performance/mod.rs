//! Lynx's performance entries, as this engine measures them: the
//! `pipeline` entries Lynx native's `onPerformanceEvent` reports
//! (`loadBundle`, the reloads and the flagged updates), shaped as Rust
//! types, and the one table that spells them in Lynx's names; and, opt-in
//! beside them, Paint Timing's two `paint` entries (`first-paint`,
//! `first-contentful-paint`), which Lynx does not report.
//!
//! The measurements underneath are `terminus`'s, in the browser's vocabulary:
//! a commit's style, layout and frame-build marks
//! ([`terminus::link::CommitTiming`]) and a presented frame's compose and
//! present marks ([`terminus::link::painter::PresentedFrame`]). What this
//! module adds is Lynx's model on top of them: which pipeline a flush
//! belongs to, the `__lynx_timing_flag`s it carried, the load's own
//! milestones, and the derived metrics (`lynxFcp`, `actualFmp`, …), with
//! Lynx's formulas.
//!
//! An embedder subscribes with [`crate::LynxView::observe_performance`] and
//! receives each entry from [`crate::LynxView::pump`] as
//! [`crate::EngineEvent::Performance`]. Nothing is assembled for a type
//! nobody subscribed to, and nothing at all on the view's main thread: a
//! flush records instants and a commit id, the painter records when the
//! commit was presented, and the entry is built by whichever thread takes
//! it — the embedder's in `pump`, or the BTS worker's own job for a
//! `lynx.performance` observer there.
//!
//! The Lynx names exist in two places only: [`PipelineOrigin`]'s, which are
//! the entries' `name`s, and [`PerformanceEntry::lynx_fields`], the key
//! table both [`PerformanceEntry::write_lynx_json`] and the BTS's entries are
//! written from.
//!
//! Beside the entries sit the two monitors over the engine threads' jobs
//! (`monitors`), the long task and the JS blocking monitor, and the memory
//! monitor (`memory`), which reads the two script runtimes and the
//! document's elements on its interval: off unless the Rust embedder turns
//! them on, and reported to it alone.
//!
//! And Lynx's `resource.lazyBundle` entry, one per `lynx.fetchBundle` that
//! made a request ([`LazyBundleEntry`]), recorded only while someone
//! observes `resource`.
//!
//! And W3C Container Timing's `container` entry ([`ContainerEntry`]), which
//! Lynx does not have: one per presented frame that grew the painted region
//! of an element carrying a `containertiming` attribute, from `terminus`'s
//! primitive ([`terminus::link::ContainerPaint`]).

mod members;
mod memory;
mod monitors;
mod registry;

use std::borrow::Cow;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
/// The engine's monotonic instant, which a [`Timestamp`] holds and a fetcher
/// reports a [`crate::resource::FetchOutcome`] in: `std::time::Instant`
/// natively and `web_time::Instant` on wasm32, where `std`'s panics.
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

use smallvec::SmallVec;
/// The id of an element, as a [`ContainerEntry`] names its root and its last
/// painted element: the number a script sees as the element's unique id.
pub use terminus::link::NodeId;
/// The rectangle a [`ContainerEntry`]'s `intersection_rect` is, in viewport
/// CSS px.
pub use terminus::link::Rect;
#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

pub(crate) use self::members::{
    install_pipeline_members, install_worker_members, pipeline_argument,
};
pub use self::memory::{MemoryCategory, MemoryConfig, MemoryEntry, MemoryItem};
pub(crate) use self::monitors::MonitorTask;
pub use self::monitors::{
    JsBlockingConfig, JsBlockingEntry, JsBlockingStage, LongTaskConfig, LongTaskEntry,
};
pub(crate) use self::registry::{
    FlushBinding, LoadMark, Opener, OpenerId, PipelineRegistry, WorkerAudience, WorkerLoadMark,
};
use crate::clock::ClockInstant;
/// The wall-clock type [`ExtraTiming`]'s marks are: `std::time::SystemTime`
/// natively and `web_time::SystemTime` on wasm32, so an embedder builds an
/// `ExtraTiming` without depending on `web-time` itself.
pub use crate::clock::SystemTime;
pub use crate::jobs::{JobKind, JsThreadName};

/// One instant on the engine's monotonic clock, comparable across every
/// thread of the process: a mark taken on the main thread, one taken on a
/// worker and one taken by the painter subtract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub(crate) ClockInstant);

impl Timestamp {
    /// The monotonic instant itself.
    #[must_use]
    pub const fn instant(self) -> ClockInstant {
        self.0
    }

    /// This instant as Lynx reports one: milliseconds since the Unix epoch,
    /// rounded to the microsecond (`1739594612307.429`), placed on that
    /// timeline by the view's [`TimeOrigin`].
    #[must_use]
    pub fn unix_millis(&self, origin: &TimeOrigin) -> f64 {
        let offset = self
            .0
            .checked_duration_since(origin.instant)
            .map_or_else(|| -millis(origin.instant.duration_since(self.0)), millis);
        round_millis(origin.unix_millis + offset)
    }

    /// How long after `earlier` this instant is; zero when it is not after
    /// it.
    #[must_use]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_duration_since(earlier.0)
    }
}

/// Where a view's monotonic clock sits on the Unix timeline: one reading of
/// each clock, taken together as the first statement of
/// [`crate::LynxGroup::create_lynx_view`].
///
/// Every timestamp an entry reports is converted through it, once, which is
/// what makes Lynx's `totalFcp = paintEnd − openTime` meaningful: `openTime`
/// is a host's wall-clock reading from before the view existed.
#[derive(Clone, Copy, Debug)]
pub struct TimeOrigin {
    instant: ClockInstant,
    /// Already rounded the way [`Timestamp::unix_millis`] rounds, so the
    /// origin's own instant converts to exactly this.
    unix_millis: f64,
}

impl TimeOrigin {
    /// The two clocks read now.
    pub(crate) fn now() -> Self {
        let system = SystemTime::now();
        let instant = ClockInstant::now();
        Self {
            instant,
            unix_millis: round_millis(unix_millis_of(system)),
        }
    }

    /// The origin's monotonic reading.
    #[must_use]
    pub const fn instant(&self) -> Timestamp {
        Timestamp(self.instant)
    }

    /// The origin's wall-clock reading, in Unix milliseconds rounded to the
    /// microsecond.
    #[must_use]
    pub const fn unix_millis(&self) -> f64 {
        self.unix_millis
    }

    /// A wall-clock reading moved onto the monotonic clock. `None` for one
    /// the platform's monotonic clock cannot represent, which is a reading
    /// from before the machine started.
    pub(crate) fn timestamp_of(&self, time: SystemTime) -> Option<Timestamp> {
        let offset = unix_millis_of(time) - self.unix_millis;
        let distance = Duration::try_from_secs_f64(offset.abs() / 1000.0).ok()?;
        if offset >= 0.0 {
            self.instant.checked_add(distance)
        } else {
            self.instant.checked_sub(distance)
        }
        .map(Timestamp)
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Lynx's precision: milliseconds with three decimals.
fn round_millis(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn unix_millis_of(time: SystemTime) -> f64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => millis(after),
        Err(before) => -millis(before.duration()),
    }
}

/// Lynx's `PerformanceMetric`: a named span between two named marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metric {
    /// The metric's Lynx name: `lynxFcp`, `fcp`, `totalFcp`, `actualFmp`,
    /// `lynxActualFmp` or `totalActualFmp`.
    pub name: &'static str,
    /// The Lynx name of the mark it starts at.
    pub start_name: &'static str,
    pub start: Timestamp,
    /// The Lynx name of the mark it ends at.
    pub end_name: &'static str,
    pub end: Timestamp,
}

impl Metric {
    /// The span, zero when the end is not after the start.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.end.duration_since(self.start)
    }
}

/// What started a pipeline, which is the `name` of its entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineOrigin {
    /// The view's first screen, or a `<frame>` card's.
    LoadBundle,
    /// [`crate::LynxView::reload`].
    ReloadBundleFromNative,
    /// A BTS `lynx.reload`.
    ReloadBundleFromBts,
    /// A background-thread update (`ReactLynx`'s `setState`).
    UpdateTriggeredByBts,
    /// [`crate::LynxView::update_data`] or `reset_data`.
    UpdateTriggeredByNative,
    /// [`crate::LynxView::update_global_props`].
    UpdateGlobalProps,
    /// Lynx's `setNativeProps`.
    SetNativeProps,
    /// `ReactLynx`'s hydration of the first screen.
    ReactLynxHydrate,
}

impl PipelineOrigin {
    const ALL: [Self; 8] = [
        Self::LoadBundle,
        Self::ReloadBundleFromNative,
        Self::ReloadBundleFromBts,
        Self::UpdateTriggeredByBts,
        Self::UpdateTriggeredByNative,
        Self::UpdateGlobalProps,
        Self::SetNativeProps,
        Self::ReactLynxHydrate,
    ];

    /// Lynx's name for it: an entry's `name`, and what a framework passes
    /// as `pipelineOrigin`.
    #[must_use]
    pub const fn lynx_name(self) -> &'static str {
        match self {
            Self::LoadBundle => "loadBundle",
            Self::ReloadBundleFromNative => "reloadBundleFromNative",
            Self::ReloadBundleFromBts => "reloadBundleFromBts",
            Self::UpdateTriggeredByBts => "updateTriggeredByBts",
            Self::UpdateTriggeredByNative => "updateTriggeredByNative",
            Self::UpdateGlobalProps => "updateGlobalProps",
            Self::SetNativeProps => "setNativeProps",
            Self::ReactLynxHydrate => "reactLynxHydrate",
        }
    }

    /// The origin Lynx names `name`.
    #[must_use]
    pub fn from_lynx_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|origin| origin.lynx_name() == name)
    }

    /// Whether this is a reload, whose pipeline yields a
    /// [`PerformanceEntry::ReloadBundle`].
    const fn is_reload(self) -> bool {
        matches!(
            self,
            Self::ReloadBundleFromNative | Self::ReloadBundleFromBts
        )
    }
}

/// What the framework told the pipeline about itself: `ReactLynx`'s `dsl`
/// and `stage`, and the marks its `_markTiming` made (`diffVdomStart`,
/// `patchChangesEnd`, …) in the order they were made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameworkTiming {
    pub dsl: Option<Arc<str>>,
    pub stage: Option<Arc<str>>,
    pub marks: Vec<(Arc<str>, Timestamp)>,
}

/// This engine's painter stages, reported where Lynx reports its
/// platform's (`hostPlatformTiming`, with `hostPlatformType: "vello"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PainterTiming {
    /// The presented frame's composition through its GPU submit.
    pub compose: Range<Timestamp>,
}

/// The three `actualFmp` metrics of a pipeline flagged
/// `__lynx_timing_actual_fmp`: its `paintEnd` minus the load's start, the
/// host's `prepareTemplateStart` and its `openTime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActualFmp {
    /// From `prepareTemplateStart`; `None` until the host set it
    /// ([`crate::LynxView::set_extra_timing`]).
    pub actual_fmp: Option<Metric>,
    /// From the load's `loadBundleStart`.
    pub lynx_actual_fmp: Metric,
    /// From `openTime`; `None` until the host set it.
    pub total_actual_fmp: Option<Metric>,
}

/// Lynx's `PipelineEntry`: one pipeline from its start to the present of
/// the frame that carried it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineTiming {
    pub origin: PipelineOrigin,
    /// The `__lynx_timing_flag` this entry reports; `None` on a load's or a
    /// reload's own entry.
    pub identifier: Option<Arc<str>>,
    pub pipeline_start: Timestamp,
    /// The present of the first frame whose commit covers the pipeline's;
    /// the same instant as [`Self::paint_end`].
    pub pipeline_end: Timestamp,
    /// The framework's main-thread render, when it marked one.
    pub mts_render: Option<Range<Timestamp>>,
    /// The commit's style flush (`resolveStart`/`resolveEnd`).
    pub resolve: Range<Timestamp>,
    /// The commit's layout.
    pub layout: Range<Timestamp>,
    /// The commit's frame build: the paint-order build and scene encode.
    pub painting_ui_operation_execute: Range<Timestamp>,
    /// The painter's composition of the presented frame.
    pub layout_ui_operation_execute: Range<Timestamp>,
    /// When the present call returned.
    pub paint_end: Timestamp,
    pub framework_rendering_timing: FrameworkTiming,
    pub host_platform_timing: PainterTiming,
    /// Present only on the entry whose identifier is
    /// `__lynx_timing_actual_fmp`.
    pub actual_fmp: Option<ActualFmp>,
}

/// The five marks a Lynx host reports about the container around a view
/// (Lynx's `setExtraTiming`), as wall-clock readings. Each is set once:
/// a second report of a key changes nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtraTiming {
    pub open_time: Option<SystemTime>,
    pub container_init_start: Option<SystemTime>,
    pub container_init_end: Option<SystemTime>,
    pub prepare_template_start: Option<SystemTime>,
    pub prepare_template_end: Option<SystemTime>,
}

impl ExtraTiming {
    /// Takes each key `other` sets that this one does not, answering whether
    /// any was taken.
    pub(crate) fn merge_unset(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (mine, theirs) in [
            (&mut self.open_time, other.open_time),
            (&mut self.container_init_start, other.container_init_start),
            (&mut self.container_init_end, other.container_init_end),
            (
                &mut self.prepare_template_start,
                other.prepare_template_start,
            ),
            (&mut self.prepare_template_end, other.prepare_template_end),
        ] {
            if mine.is_none() && theirs.is_some() {
                *mine = theirs;
                changed = true;
            }
        }
        changed
    }

    /// The keys in Lynx's names and order.
    fn lynx_keys(&self) -> [(&'static str, Option<SystemTime>); 5] {
        [
            ("openTime", self.open_time),
            ("containerInitStart", self.container_init_start),
            ("containerInitEnd", self.container_init_end),
            ("prepareTemplateStart", self.prepare_template_start),
            ("prepareTemplateEnd", self.prepare_template_end),
        ]
    }
}

/// Lynx's `LoadBundleEntry`: the first screen's pipeline, and the load's own
/// milestones around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadBundleTiming {
    pub pipeline: PipelineTiming,
    /// The bundle that loaded: the view's entry URL, or a `<frame>` card's
    /// container URL. Lynx carries it beside the entry; it is how a
    /// nested card's load is told from the view's own.
    pub url: Arc<str>,
    /// [`crate::LynxGroup::create_lynx_view`], or a `<frame>` card's realm
    /// construction.
    pub create_lynx: Range<Timestamp>,
    /// From the boot's import of the entry to the return of the first
    /// flush after the first render.
    pub load_bundle: Range<Timestamp>,
    /// The bundle's decode. `None`: it runs inside the embedder's fetcher,
    /// which reports no marks.
    pub parse: Option<Range<Timestamp>>,
    /// The entry's evaluation, its top-level await included.
    pub vm_execute: Option<Range<Timestamp>>,
    /// The BTS realm opening, to its `initialize` handled.
    pub load_core: Option<Range<Timestamp>>,
    /// The BTS entry's import.
    pub load_background: Option<Range<Timestamp>>,
    pub extra: ExtraTiming,
    /// The first paint minus `loadBundleStart`.
    pub lynx_fcp: Metric,
    /// The first paint minus `prepareTemplateStart`, once the host set it.
    pub fcp: Option<Metric>,
    /// The first paint minus `openTime`, once the host set it.
    pub total_fcp: Option<Metric>,
}

/// Lynx's `ReloadBundleEntry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadBundleTiming {
    pub pipeline: PipelineTiming,
    /// The view's entry URL.
    pub url: Arc<str>,
    /// From the reload's arrival on the main thread to the return of the
    /// flush that rendered it.
    pub reload_bundle: Range<Timestamp>,
    /// The BTS's half of the reload. `None`: not marked yet.
    pub reload_background: Option<Range<Timestamp>>,
    /// The present minus `reloadBundleStart`.
    pub lynx_fcp: Metric,
    pub fcp: Option<Metric>,
    pub total_fcp: Option<Metric>,
}

/// Which of Paint Timing's two marks a [`PaintEntry`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaintName {
    /// The view's first presented frame, whatever it drew. The same instant
    /// Lynx's `fcp`, `lynxFcp` and `totalFcp` end at.
    FirstPaint,
    /// The view's first presented frame with contentful ink, as Paint
    /// Timing defines it: a text glyph that is not white space at a used
    /// opacity above zero, an image whose pixels are available, an SVG with
    /// something to draw — not a background colour or a border. Lynx has no
    /// counterpart.
    FirstContentfulPaint,
}

impl PaintName {
    /// The Performance Timeline name: `"first-paint"` or
    /// `"first-contentful-paint"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstPaint => "first-paint",
            Self::FirstContentfulPaint => "first-contentful-paint",
        }
    }
}

/// W3C Paint Timing's `PerformancePaintTiming`: one of the view's two first
/// marks, reported once each to an observer of [`EntryTypes::PAINT`] (or a
/// BTS `observe(['paint'])`) that was observing when the mark was
/// recorded. Its `duration` is zero, as the standard's is, so it is not
/// carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintEntry {
    pub name: PaintName,
    /// When the frame's present call returned (Paint Timing's
    /// presentation time).
    pub start: Timestamp,
}

/// Lynx's `LazyBundleEntry` (`entryType` `"resource"`, `name`
/// `"lazyBundle"`): one bundle `lynx.fetchBundle` fetched, from the call to
/// the fetcher's answer.
///
/// Native fills it in `LazyBundleLifecycleOption::GetLazyBundleEntry`
/// (`lynx/core/resource/lazy_bundle/lazy_bundle_lifecycle_option.cc:138-168`)
/// for a load that made a request, and makes none for one served from what
/// the engine already held (`mode == STATE_CACHE`, line 139); a bundle a
/// view has already fetched is that here — `fetchBundle` answers it at once
/// through the fetcher's probe, requests nothing, and reports nothing.
/// `mode` is a constant: `"normal"` for every entry native sends
/// ([`Self::MODE`]). `sync`, which `LazyBundleEntry.yml` lists, is not
/// carried, because native's writer never writes it (lines 154-165): it is
/// the old `perf_info` record's flag — the resource loader answered inside
/// the require call itself (`LazyBundleLoader::SyncRequiring`,
/// `lazy_bundle_loader.cc:66,312-316`) — and a fetch here always answers
/// later, through the realm's future, so it would be `false` every time.
///
/// A deviation in what produces it: native sends it for a load through the
/// engine's own lazy bundle path (`__QueryComponent`, a radon lazy
/// component: `TemplateAssembler::RequireTemplateEntry`,
/// `lynx/core/renderer/template_assembler.cc`) and none for `fetchBundle`
/// (`TemplateAssembler::FetchBundle` and `LazyBundleLoader::DidFetchBundle`
/// touch no lifecycle option). This engine has no `__QueryComponent`, so
/// `fetchBundle`, which is how `ReactLynx` loads a lazy bundle here, is what
/// makes it. A `<frame>`'s fetch of its card makes none, as native's frame
/// load (`LazyBundleLoader::LoadFrameBundle`, `kFrame`) makes none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LazyBundleEntry {
    /// The URL the script passed (`componentUrl`).
    pub component_url: Arc<str>,
    /// The body's length in bytes (`size`), when the fetch succeeded and the
    /// fetcher reported it. Native writes 0 where it has none; this leaves
    /// the key out.
    pub size_bytes: Option<u64>,
    /// Whether the fetch succeeded (`loadSuccess`).
    pub load_success: bool,
    /// From the `fetchBundle` call to the bytes having arrived at the
    /// fetcher — or, when the fetcher reported no arrival, to the outcome's
    /// delivery to the realm (`requireStart`/`requireEnd`). Native's starts
    /// at the request (`LazyBundleLoader::StartRecordRequireTime`,
    /// `lazy_bundle_loader.cc:88,318-324`) and ends when the response
    /// reaches the engine thread (`EndRecordRequireTime`, lines 75 and
    /// 326-337).
    pub require: Range<Timestamp>,
    /// The fetcher's install of the bundle — the reference fetcher's
    /// container decode and the registration of its sections
    /// (`decodeStart`/`decodeEnd`), native's template entry build
    /// (`template_assembler.cc:1444-1455`). `None` when nothing was
    /// installed or the fetcher did not say.
    pub decode: Option<Range<Timestamp>>,
}

impl LazyBundleEntry {
    /// Lynx's `mode` for every entry it sends: `GenerateModeInfo`
    /// (`lynx/core/resource/lazy_bundle/lazy_bundle_utils.cc:110-120`) names
    /// `preload`, `cache` and `normal`, but a cached load sends no entry and
    /// a preloaded one sets its mode on an option that is never dispatched
    /// (`template_assembler.cc:2785-2786`), so what reaches an observer is
    /// `normal`. This engine has no preloading of lazy bundles either.
    pub const MODE: &'static str = "normal";

    /// The entry of one timed `fetchResource` of `url`: called at `started`,
    /// its outcome delivered to the realm at `delivered`, with the fetcher's
    /// readings when it succeeded (`outcome`), `None` when it failed.
    pub(crate) fn from_fetch(
        url: Arc<str>,
        started: ClockInstant,
        delivered: ClockInstant,
        outcome: Option<&crate::resource::FetchOutcome>,
    ) -> Self {
        let received = outcome
            .and_then(|outcome| outcome.received)
            .unwrap_or(delivered)
            .max(started);
        Self {
            component_url: url,
            size_bytes: outcome.and_then(|outcome| outcome.size_bytes),
            load_success: outcome.is_some(),
            require: Timestamp(started)..Timestamp(received),
            decode: outcome
                .and_then(|outcome| outcome.installed.clone())
                .map(|installed| Timestamp(installed.start)..Timestamp(installed.end)),
        }
    }
}

/// W3C Container Timing's `PerformanceContainerTiming` (WICG draft,
/// `https://wicg.github.io/container-timing/`): a presented frame grew the
/// painted region of the element carrying a `containertiming` attribute,
/// reported to the Rust embedder when it observes [`EntryTypes::CONTAINER`].
/// Lynx has no counterpart, and a background thread's `lynx.performance`
/// does not receive it.
///
/// The region is everything contentful painted under the root since its
/// first paint — text with a glyph that is not white space and images whose
/// pixels are available, as Paint Timing counts content — cut by its clips
/// and the viewport, except what an element carrying `containertimingignore`
/// cuts out; a shadow tree paints inside its host's roots
/// (`docs/tracking/deviations.md`). Recording stops at the view's first
/// pointer down, wheel or key down, which is what the draft's "has
/// dispatched input event" and "has dispatched scroll event" are for a
/// view: a later paint under the root reports nothing.
///
/// `rootElement` and `lastPaintedElement` are elements in the standard;
/// this entry carries their [`NodeId`]s.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerEntry {
    /// The `containertiming` attribute's value; `""` for an empty one.
    pub identifier: Arc<str>,
    /// The element carrying the attribute (`rootElement`).
    pub root: NodeId,
    /// When the first frame that painted anything under the root was
    /// presented (`firstRenderTime`).
    pub first_render: Timestamp,
    /// When the frame that grew the region was presented (`startTime`).
    pub start: Timestamp,
    /// The painted region's area in square CSS px (`size`): the area of the
    /// union of what was painted, not of its bounding box.
    pub size: f32,
    /// The painted region's bounding box in viewport CSS px
    /// (`intersectionRect`).
    pub intersection_rect: Rect<f32>,
    /// Of the elements the frame painted under the root, the one whose paint
    /// added the most new area (`lastPaintedElement`), retargeted to the
    /// root's tree: an element inside a shadow tree the root is not in is
    /// reported as that tree's host.
    pub last_painted: NodeId,
}

impl ContainerEntry {
    /// The entry of one `terminus` report, for the root whose attribute
    /// value is `identifier`.
    pub(crate) const fn from_paint(
        paint: &terminus::link::ContainerPaint,
        identifier: Arc<str>,
    ) -> Self {
        Self {
            identifier,
            root: paint.root,
            first_render: Timestamp(paint.first_render),
            start: Timestamp(paint.latest),
            size: paint.size,
            intersection_rect: paint.painted,
            last_painted: paint.last_painted,
        }
    }
}

/// One performance entry, as [`crate::EngineEvent::Performance`] reports it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PerformanceEntry {
    LoadBundle(Box<LoadBundleTiming>),
    ReloadBundle(Box<ReloadBundleTiming>),
    Pipeline(Box<PipelineTiming>),
    /// W3C Paint Timing, behind [`EntryTypes::PAINT`].
    Paint(PaintEntry),
    /// A job that ran long ([`crate::LynxView::set_long_task_monitor`]):
    /// W3C Long Tasks' `longtask`, attributed as Lynx's
    /// `lynxsdk_long_task_timing` event is, and like that event reported to
    /// the embedder alone.
    LongTask(LongTaskEntry),
    /// Lynx's `js_blocking` entry
    /// ([`crate::LynxView::set_js_blocking_monitor`]), reported to the
    /// embedder alone as Lynx reports it to the platform alone.
    JsBlocking(JsBlockingEntry),
    /// Lynx's `memory` entry ([`crate::LynxView::set_memory_monitor`]):
    /// every category's latest reading, reported when the total moved by
    /// the monitor's threshold, to the embedder alone as Lynx reports it to
    /// the platform alone.
    Memory(MemoryEntry),
    /// Lynx's `resource.lazyBundle` entry, behind [`EntryTypes::RESOURCE`]:
    /// one `lynx.fetchBundle` that made a request. Native sends it to every
    /// audience (`kEventTypeAll`,
    /// `lynx/core/resource/lazy_bundle/lazy_bundle_loader.cc:169`), its
    /// background thread included, so the BTS observer of `resource` of the
    /// realm that fetched receives it too.
    LazyBundle(LazyBundleEntry),
    /// W3C Container Timing, behind [`EntryTypes::CONTAINER`]: one root's
    /// painted region grew. The Rust embedder alone receives it.
    Container(ContainerEntry),
}

/// Which entries an observer receives. Empty by default: an embedder that
/// asks for nothing gets nothing assembled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EntryTypes(u8);

impl EntryTypes {
    /// None.
    pub const NONE: Self = Self(0);
    /// [`PerformanceEntry::Pipeline`]: one per `__lynx_timing_flag`.
    pub const PIPELINE: Self = Self(1);
    /// [`PerformanceEntry::LoadBundle`] and [`PerformanceEntry::ReloadBundle`].
    pub const LOAD_BUNDLE: Self = Self(1 << 1);
    /// [`PerformanceEntry::Paint`]: `first-paint` and
    /// `first-contentful-paint`. Asking for it is what turns on the paint
    /// walk's contentful test, until `first-contentful-paint` is recorded.
    pub const PAINT: Self = Self(1 << 2);
    /// [`PerformanceEntry::LongTask`]; asking for it turns no monitor on.
    pub const LONG_TASK: Self = Self(1 << 3);
    /// [`PerformanceEntry::JsBlocking`]; asking for it turns no monitor on.
    pub const JS_BLOCKING: Self = Self(1 << 4);
    /// [`PerformanceEntry::Memory`]; asking for it turns no monitor on.
    pub const MEMORY: Self = Self(1 << 5);
    /// [`PerformanceEntry::LazyBundle`]. Asking for it is what times a
    /// `fetchBundle`: two instants per fetch, and the fetcher's own
    /// readings.
    pub const RESOURCE: Self = Self(1 << 6);
    /// [`PerformanceEntry::Container`]. Asking for it turns nothing on in the
    /// document: an element's `containertiming` attribute is what makes the
    /// paint walk attribute paints, observed or not.
    pub const CONTAINER: Self = Self(1 << 7);
    /// Every type.
    pub const ALL: Self = Self(
        Self::PIPELINE.0
            | Self::LOAD_BUNDLE.0
            | Self::PAINT.0
            | Self::LONG_TASK.0
            | Self::JS_BLOCKING.0
            | Self::MEMORY.0
            | Self::RESOURCE.0
            | Self::CONTAINER.0,
    );

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether any type is in both.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for EntryTypes {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

/// One value of [`PerformanceEntry::lynx_fields`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LynxValue<'a> {
    /// A timestamp (Unix milliseconds) or a duration (milliseconds), each
    /// with three decimals.
    Number(f64),
    /// A number at full precision, where Lynx writes an `int64` or a
    /// `double`; one that is not finite is JSON's `null`.
    Double(f64),
    /// Where Lynx writes a `bool` (`PushBoolToMap`).
    Boolean(bool),
    Text(&'a str),
}

/// What [`ACTUAL_FMP_FLAG`] fills.
pub(crate) const ACTUAL_FMP_FLAG: &str = "__lynx_timing_actual_fmp";

impl PerformanceEntry {
    /// The entry's `entryType`: Lynx's `"pipeline"` for the three pipeline
    /// entries, Paint Timing's `"paint"`, Long Tasks' `"longtask"`, Lynx's
    /// `"js_blocking"`, `"memory"` and `"resource"`, and Container Timing's
    /// `"container"`.
    #[must_use]
    pub const fn entry_type(&self) -> &'static str {
        match self {
            Self::LoadBundle(_) | Self::ReloadBundle(_) | Self::Pipeline(_) => "pipeline",
            Self::Paint(_) => "paint",
            Self::LongTask(_) => "longtask",
            Self::JsBlocking(_) => "js_blocking",
            Self::Memory(_) => "memory",
            Self::LazyBundle(_) => RESOURCE_ENTRY_TYPE,
            Self::Container(_) => CONTAINER_ENTRY_TYPE,
        }
    }

    /// The entry's `name`: the pipeline's origin, the paint mark's
    /// Performance Timeline name, the long task's [`JobKind`], Lynx's
    /// `"js_blocking"` or `"memory"` again, Lynx's `"lazyBundle"`, or for a
    /// [`Self::Container`] the empty string, as the draft's `name` getter
    /// answers (the root's identifier is its `identifier`).
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Container(_) => "",
            Self::Paint(paint) => paint.name.as_str(),
            Self::LongTask(task) => task.kind.as_str(),
            Self::JsBlocking(_) => "js_blocking",
            Self::Memory(_) => "memory",
            Self::LazyBundle(_) => LAZY_BUNDLE_NAME,
            Self::LoadBundle(_) | Self::ReloadBundle(_) | Self::Pipeline(_) => self
                .pipeline()
                .map_or("", |pipeline| pipeline.origin.lynx_name()),
        }
    }

    /// The pipeline a pipeline entry carries; `None` for any other.
    #[must_use]
    pub fn pipeline(&self) -> Option<&PipelineTiming> {
        match self {
            Self::LoadBundle(load) => Some(&load.pipeline),
            Self::ReloadBundle(reload) => Some(&reload.pipeline),
            Self::Pipeline(pipeline) => Some(pipeline),
            Self::Paint(_)
            | Self::LongTask(_)
            | Self::JsBlocking(_)
            | Self::Memory(_)
            | Self::LazyBundle(_)
            | Self::Container(_) => None,
        }
    }

    /// The entry in Lynx's keys and Lynx's units, in a fixed order:
    /// `entryType`, `name` and `identifier` (`""` for none) first, then the
    /// pipeline's marks, the load's or reload's, the metrics, and the two
    /// nested objects. A key is a path: each `.` in it opens one nested
    /// object, so `frameworkRenderingTiming.diffVdomStart` is a member of
    /// `frameworkRenderingTiming` and `detail.lynxTasmElement.sizeBytes` one
    /// of `detail`'s `lynxTasmElement`. A `.` or `\` that is part of a name —
    /// a framework mark's own key — is written `\.` or `\\`. The members of
    /// each object are adjacent. A field whose value this engine does not
    /// have is left out, not zeroed.
    ///
    /// A [`Self::Paint`] is `entryType`, `name` and `startTime` alone.
    /// Its `startTime` is Unix milliseconds like every other timestamp here,
    /// not the standard's milliseconds after `timeOrigin`: the absolute
    /// reading is what compares with the pipeline entries' marks (a
    /// `first-paint`'s `startTime` is the page load's `lynxFcp` end).
    ///
    /// A [`Self::LongTask`] is Long Tasks' `startTime` and `duration`, then
    /// the keys of Lynx's `lynxsdk_long_task_timing` event
    /// (`long_task_monitor.cc`) but `task_info` and `duration_threshold_ms`.
    /// A [`Self::JsBlocking`] is the keys `ReportBlockingInfo` writes
    /// (`js_blocking_monitor.cc`), in its order.
    ///
    /// A [`Self::Memory`] is what Lynx's `MemoryMonitor::ReportMemory`
    /// writes, in its order: `entryType`, `name`, then under `detail` one
    /// object per category read, keyed by the category's name, with its
    /// `category`, `sizeBytes`, `instanceCount` and its own `detail`, a map
    /// of strings; then `sizeBytes`, the categories' sum.
    ///
    /// A [`Self::LazyBundle`] is what Lynx's
    /// `LazyBundleLifecycleOption::GetLazyBundleEntry` writes, in its order
    /// and types: `entryType`, `name`, `componentUrl`, the integer `size`
    /// (left out when the fetcher did not report one), the boolean
    /// `loadSuccess`, then `mode`, then `decodeStart`/`decodeEnd` (left out
    /// for a fetch nothing was installed from) and `requireStart`/
    /// `requireEnd`. No `sync`: native's writer leaves it out.
    ///
    /// A [`Self::Container`] is the draft's `PerformanceContainerTiming` in
    /// its own names and its `toJSON` order: `entryType`, `name` (`""`),
    /// `startTime` and `duration` (0), then `identifier`, the four members
    /// of `intersectionRect` (`x`, `y`, `width`, `height`, CSS px, at full
    /// precision), `size` (square CSS px, full precision) and
    /// `firstRenderTime`. Both times are Unix milliseconds, as a `paint`
    /// entry's are. `rootElement` and `lastPaintedElement` are elements
    /// and are not written.
    ///
    /// The one place a Lynx key is spelled: [`Self::write_lynx_json`] and
    /// the BTS's `lynx.performance` entries are both written from this.
    pub fn lynx_fields(
        &self,
        origin: &TimeOrigin,
    ) -> impl Iterator<Item = (Cow<'static, str>, LynxValue<'_>)> {
        let pipeline = match self {
            Self::Paint(paint) => return paint_fields(paint, origin).into_iter(),
            Self::LongTask(task) => return long_task_fields(task, origin).into_iter(),
            Self::JsBlocking(report) => return js_blocking_fields(report, origin).into_iter(),
            Self::Memory(memory) => return memory_fields(memory, origin).into_iter(),
            Self::LazyBundle(bundle) => return lazy_bundle_fields(bundle, origin).into_iter(),
            Self::Container(container) => return container_fields(container, origin).into_iter(),
            Self::LoadBundle(load) => &load.pipeline,
            Self::ReloadBundle(reload) => &reload.pipeline,
            Self::Pipeline(pipeline) => pipeline,
        };
        let mut fields = Fields {
            origin: *origin,
            list: Vec::with_capacity(48),
        };
        fields.text("entryType", self.entry_type());
        fields.text("name", self.name());
        fields.text(
            "identifier",
            pipeline.identifier.as_deref().unwrap_or_default(),
        );
        fields.time("pipelineStart", pipeline.pipeline_start);
        fields.time("pipelineEnd", pipeline.pipeline_end);
        if let Some(render) = &pipeline.mts_render {
            fields.range("mtsRenderStart", "mtsRenderEnd", render);
        }
        fields.range("resolveStart", "resolveEnd", &pipeline.resolve);
        fields.range("layoutStart", "layoutEnd", &pipeline.layout);
        fields.range(
            "paintingUiOperationExecuteStart",
            "paintingUiOperationExecuteEnd",
            &pipeline.painting_ui_operation_execute,
        );
        fields.range(
            "layoutUiOperationExecuteStart",
            "layoutUiOperationExecuteEnd",
            &pipeline.layout_ui_operation_execute,
        );
        fields.time("paintEnd", pipeline.paint_end);
        let mut vm_execute = None;
        match self {
            Self::LoadBundle(load) => {
                fields.range("loadBundleStart", "loadBundleEnd", &load.load_bundle);
                if let Some(parse) = &load.parse {
                    fields.range("parseStart", "parseEnd", parse);
                }
                if let Some(background) = &load.load_background {
                    fields.range("loadBackgroundStart", "loadBackgroundEnd", background);
                }
                fields.range("createLynxStart", "createLynxEnd", &load.create_lynx);
                if let Some(core) = &load.load_core {
                    fields.range("loadCoreStart", "loadCoreEnd", core);
                }
                for (key, value) in load.extra.lynx_keys() {
                    if let Some(value) = value {
                        fields.number(key, round_millis(unix_millis_of(value)));
                    }
                }
                fields.metric("lynxFcp", &load.lynx_fcp);
                fields.optional_metric("fcp", load.fcp.as_ref());
                fields.optional_metric("totalFcp", load.total_fcp.as_ref());
                vm_execute = load.vm_execute.as_ref();
            }
            Self::ReloadBundle(reload) => {
                fields.range(
                    "reloadBundleStart",
                    "reloadBundleEnd",
                    &reload.reload_bundle,
                );
                if let Some(background) = &reload.reload_background {
                    fields.range("reloadBackgroundStart", "reloadBackgroundEnd", background);
                }
                fields.metric("lynxFcp", &reload.lynx_fcp);
                fields.optional_metric("fcp", reload.fcp.as_ref());
                fields.optional_metric("totalFcp", reload.total_fcp.as_ref());
            }
            Self::Pipeline(_)
            | Self::Paint(_)
            | Self::LongTask(_)
            | Self::JsBlocking(_)
            | Self::Memory(_)
            | Self::LazyBundle(_)
            | Self::Container(_) => {}
        }
        fields.framework(&pipeline.framework_rendering_timing, vm_execute);
        fields.text("hostPlatformTiming.hostPlatformType", "vello");
        let compose = &pipeline.host_platform_timing.compose;
        fields.time("hostPlatformTiming.composeStart", compose.start);
        fields.time("hostPlatformTiming.composeEnd", compose.end);
        if let Some(fmp) = &pipeline.actual_fmp {
            fields.optional_metric("actualFmp", fmp.actual_fmp.as_ref());
            fields.metric("lynxActualFmp", &fmp.lynx_actual_fmp);
            fields.optional_metric("totalActualFmp", fmp.total_actual_fmp.as_ref());
        }
        fields.list.into_iter()
    }

    /// The entry as the JSON object Lynx's `onPerformanceEvent` hands a
    /// host, written from [`Self::lynx_fields`] into `out`: numbers with
    /// three decimals, each key's path as nested objects. For an embedder
    /// that reports or compares entries; the engine itself never calls it.
    pub fn write_lynx_json(&self, origin: &TimeOrigin, out: &mut String) {
        out.push('{');
        // The objects open around the member being written, outermost
        // first, and whether each has a member yet; the entry itself is the
        // first.
        let mut open: Vec<String> = Vec::new();
        let mut has_member: Vec<bool> = vec![false];
        for (key, value) in self.lynx_fields(origin) {
            let path = key_path(&key);
            let (leaf, objects) = path.split_last().expect("a key has at least one segment");
            let shared = open
                .iter()
                .zip(objects)
                .take_while(|(open, wanted)| open.as_str() == wanted.as_ref())
                .count();
            while open.len() > shared {
                open.pop();
                has_member.pop();
                out.push('}');
            }
            for object in &objects[shared..] {
                member_separator(out, &mut has_member);
                write_json_string(out, object);
                out.push_str(":{");
                open.push(object.clone().into_owned());
                has_member.push(false);
            }
            member_separator(out, &mut has_member);
            write_json_string(out, leaf);
            out.push(':');
            write_lynx_value(out, value);
        }
        for _ in open {
            out.push('}');
        }
        out.push('}');
    }
}

/// A comma before every member of the innermost open object but its first.
fn member_separator(out: &mut String, has_member: &mut [bool]) {
    if let Some(innermost) = has_member.last_mut() {
        if *innermost {
            out.push(',');
        }
        *innermost = true;
    }
}

/// A name as one segment of a [`PerformanceEntry::lynx_fields`] key: its
/// `\` and `.` escaped, so it nests nothing.
fn escape_segment(name: &str) -> Cow<'_, str> {
    if !name.contains(['.', '\\']) {
        return Cow::Borrowed(name);
    }
    let mut escaped = String::with_capacity(name.len() + 2);
    for character in name.chars() {
        if matches!(character, '.' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Cow::Owned(escaped)
}

/// A [`PerformanceEntry::lynx_fields`] key's segments, unescaped: split at
/// every `.` that no `\` escapes.
fn key_path(key: &str) -> SmallVec<[Cow<'_, str>; 4]> {
    let mut path = SmallVec::new();
    if !key.contains('\\') {
        path.extend(key.split('.').map(Cow::Borrowed));
        return path;
    }
    let mut segment = String::new();
    let mut characters = key.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => segment.extend(characters.next()),
            '.' => path.push(Cow::Owned(std::mem::take(&mut segment))),
            other => segment.push(other),
        }
    }
    path.push(Cow::Owned(segment));
    path
}

/// A key as the BTS's record carries it, which `bobcat:performance` splits
/// at its first `.` alone: the escapes removed, so a framework mark's own
/// `.` stays in its member's name. Every key a worker is delivered has a
/// first segment with no escape in it.
fn record_key(key: &str) -> Cow<'_, str> {
    if !key.contains('\\') {
        return Cow::Borrowed(key);
    }
    let mut unescaped = String::with_capacity(key.len());
    let mut characters = key.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => unescaped.extend(characters.next()),
            other => unescaped.push(other),
        }
    }
    Cow::Owned(unescaped)
}

/// A [`PerformanceEntry::Paint`]'s fields: `entryType`, `name`,
/// `startTime`.
fn paint_fields<'a>(
    paint: &PaintEntry,
    origin: &TimeOrigin,
) -> Vec<(Cow<'static, str>, LynxValue<'a>)> {
    let mut fields = Fields {
        origin: *origin,
        list: Vec::with_capacity(3),
    };
    fields.text("entryType", "paint");
    fields.text("name", paint.name.as_str());
    fields.time("startTime", paint.start);
    fields.list
}

/// Lynx's `kTypeResource` and `kNameLazyBundle`
/// (`lynx/core/resource/lazy_bundle/lazy_bundle_lifecycle_option.cc:143-144`).
const RESOURCE_ENTRY_TYPE: &str = "resource";
const LAZY_BUNDLE_NAME: &str = "lazyBundle";

/// A [`PerformanceEntry::LongTask`]'s fields.
fn long_task_fields<'a>(
    task: &LongTaskEntry,
    origin: &TimeOrigin,
) -> Vec<(Cow<'static, str>, LynxValue<'a>)> {
    let mut fields = Fields {
        origin: *origin,
        list: Vec::with_capacity(8),
    };
    let duration = round_millis(millis(task.duration()));
    fields.text("entryType", "longtask");
    fields.text("name", task.kind.as_str());
    fields.time("startTime", task.start);
    fields.number("duration", duration);
    fields.text("thread_name", task.thread.as_str());
    if let Some(task_type) = lynx_task_type(task.kind, task.thread) {
        fields.text("task_type", task_type);
    }
    fields.text("task_name", task.kind.as_str());
    fields.number("duration_ms", duration);
    fields.list
}

/// The category Lynx's `LongTaskMonitor` files a task of this kind under
/// (`task_type`, the `k…Task` constants of `long_task_monitor.h`), from
/// where Lynx opens a `LongTaskMonitor::Scope` for the same work: a
/// template's or a background bundle's load, a timer, a call into script,
/// a native operation, an animation frame, an update the background thread
/// sent. `None` where Lynx times no such task.
const fn lynx_task_type(kind: JobKind, thread: JsThreadName) -> Option<&'static str> {
    Some(match kind {
        // `LynxEngine::LoadTemplate` on the TASM thread, the background
        // bundle's evaluation (`BTSRuntime`, `kLoadJSTask`) on the JS one.
        JobKind::Start => match thread {
            JsThreadName::Main => "load_template_task",
            JsThreadName::Workers => "load_js_task",
        },
        JobKind::Entry => "load_template_task",
        JobKind::Module => "load_js_task",
        JobKind::Timer => "timer_task",
        // `JSClosureEventListener::Invoke`, `App::SendPageEvent`,
        // `JSProxy::CallJSFunction`: script called from outside it.
        JobKind::ContentVisibility
        | JobKind::Intersection
        | JobKind::ResizeObserver
        | JobKind::ComponentEvent
        | JobKind::Post
        | JobKind::ModuleCallback
        | JobKind::Performance => "js_func_task",
        // `LynxEngine::CallLepusMethod`, `DidLoadComponent`: a native
        // operation on the engine.
        // The memory monitor's sample: a native reading, no script.
        JobKind::Command | JobKind::Future | JobKind::FontFace | JobKind::Memory => {
            "native_func_task"
        }
        // `LynxEngine::UpdateDataByJS`: what the background thread sent.
        JobKind::WorkerEvent => "update_data_by_js_task",
        // `ElementVsyncProxy::TickAllElement`.
        JobKind::AnimationFrame => "animation_task",
        JobKind::Settle | JobKind::Release => return None,
    })
}

/// A [`PerformanceEntry::JsBlocking`]'s fields, in
/// `ReportBlockingInfo`'s order.
fn js_blocking_fields<'a>(
    report: &JsBlockingEntry,
    origin: &TimeOrigin,
) -> Vec<(Cow<'static, str>, LynxValue<'a>)> {
    let mut fields = Fields {
        origin: *origin,
        list: Vec::with_capacity(9),
    };
    fields.text("entryType", "js_blocking");
    fields.text("name", "js_blocking");
    fields.text("stage", report.stage.as_str());
    let count = f64::from(report.total_blocking_count);
    fields.double(
        "total_blocking_time",
        whole_millis(report.total_blocking_time),
    );
    fields.double("total_blocking_count", count);
    fields.double("total_duration", whole_millis(report.total_duration));
    fields.double("blocking_ratio", report.blocking_ratio);
    fields.double("avg_blocking_time", millis(report.avg_blocking_time));
    // Lynx's YAML (`JSBlockingEntry.yml`) names this `time_after_fcp`; its
    // monitor writes `time_after_load_bundle`, which is what is followed.
    if let Some(after) = report.time_after_load_bundle {
        fields.double("time_after_load_bundle", whole_millis(after));
    }
    fields.list
}

/// A [`PerformanceEntry::Memory`]'s fields, in `ReportMemory`'s order:
/// `detail` before the total `sizeBytes`, the sizes and counts written as
/// Lynx's `int64` and `int32` are.
fn memory_fields<'a>(
    memory: &'a MemoryEntry,
    origin: &TimeOrigin,
) -> Vec<(Cow<'static, str>, LynxValue<'a>)> {
    let mut fields = Fields {
        origin: *origin,
        list: Vec::with_capacity(3 + memory.items.len() * 12),
    };
    fields.text("entryType", "memory");
    fields.text("name", "memory");
    for item in &memory.items {
        let category = item.category.as_str();
        fields.text(format!("detail.{category}.category"), category);
        #[allow(
            clippy::cast_precision_loss,
            reason = "a byte count past 2^53 is not a reading this engine makes"
        )]
        let size = item.size_bytes as f64;
        fields.double(format!("detail.{category}.sizeBytes"), size);
        fields.double(
            format!("detail.{category}.instanceCount"),
            f64::from(item.instance_count),
        );
        for (key, value) in &item.detail {
            fields.text(
                format!("detail.{category}.detail.{}", escape_segment(key)),
                value,
            );
        }
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a byte count past 2^53 is not a reading this engine makes"
    )]
    let total = memory.size_bytes as f64;
    fields.double("sizeBytes", total);
    fields.list
}

/// A [`PerformanceEntry::LazyBundle`]'s fields, in `GetLazyBundleEntry`'s
/// order.
fn lazy_bundle_fields<'a>(
    bundle: &'a LazyBundleEntry,
    origin: &TimeOrigin,
) -> Vec<(Cow<'static, str>, LynxValue<'a>)> {
    let mut fields = Fields {
        origin: *origin,
        list: Vec::with_capacity(10),
    };
    fields.text("entryType", RESOURCE_ENTRY_TYPE);
    fields.text("name", LAZY_BUNDLE_NAME);
    fields.text("componentUrl", &bundle.component_url);
    if let Some(size) = bundle.size_bytes {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a body past 2^53 bytes is not a fetch this engine makes"
        )]
        fields.double("size", size as f64);
    }
    fields.boolean("loadSuccess", bundle.load_success);
    fields.text("mode", LazyBundleEntry::MODE);
    if let Some(decode) = &bundle.decode {
        fields.range("decodeStart", "decodeEnd", decode);
    }
    fields.range("requireStart", "requireEnd", &bundle.require);
    fields.list
}

/// Container Timing's `entryType`.
const CONTAINER_ENTRY_TYPE: &str = "container";

/// A [`PerformanceEntry::Container`]'s fields, in the draft's IDL order
/// after `PerformanceEntry`'s own.
fn container_fields<'a>(
    container: &'a ContainerEntry,
    origin: &TimeOrigin,
) -> Vec<(Cow<'static, str>, LynxValue<'a>)> {
    let mut fields = Fields {
        origin: *origin,
        list: Vec::with_capacity(11),
    };
    let rect = container.intersection_rect;
    fields.text("entryType", CONTAINER_ENTRY_TYPE);
    fields.text("name", "");
    fields.time("startTime", container.start);
    fields.double("duration", 0.0);
    fields.text("identifier", &container.identifier);
    fields.double("intersectionRect.x", f64::from(rect.origin.x));
    fields.double("intersectionRect.y", f64::from(rect.origin.y));
    fields.double("intersectionRect.width", f64::from(rect.size.width));
    fields.double("intersectionRect.height", f64::from(rect.size.height));
    fields.double("size", f64::from(container.size));
    fields.time("firstRenderTime", container.first_render);
    fields.list
}

/// A duration in whole milliseconds, as Lynx's millisecond clock reads one.
fn whole_millis(duration: Duration) -> f64 {
    u32::try_from(duration.as_millis()).map_or(f64::from(u32::MAX), f64::from)
}

/// A value as JSON: a number with three decimals, a number at full
/// precision (`null` when it is not finite), or a string.
fn write_lynx_value(out: &mut String, value: LynxValue<'_>) {
    match value {
        LynxValue::Number(number) => {
            write!(out, "{number:.3}").expect("writing to a String cannot fail");
        }
        LynxValue::Double(number) if number.is_finite() => {
            write!(out, "{number}").expect("writing to a String cannot fail");
        }
        LynxValue::Double(_) => out.push_str("null"),
        LynxValue::Boolean(value) => out.push_str(if value { "true" } else { "false" }),
        LynxValue::Text(text) => write_json_string(out, text),
    }
}

/// `text` as a JSON string literal.
fn write_json_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(control))
                    .expect("writing to a String cannot fail");
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The list [`PerformanceEntry::lynx_fields`] builds.
struct Fields<'a> {
    origin: TimeOrigin,
    list: Vec<(Cow<'static, str>, LynxValue<'a>)>,
}

impl<'a> Fields<'a> {
    fn text(&mut self, key: impl Into<Cow<'static, str>>, value: &'a str) {
        self.list.push((key.into(), LynxValue::Text(value)));
    }

    fn number(&mut self, key: impl Into<Cow<'static, str>>, value: f64) {
        self.list.push((key.into(), LynxValue::Number(value)));
    }

    fn double(&mut self, key: impl Into<Cow<'static, str>>, value: f64) {
        self.list.push((key.into(), LynxValue::Double(value)));
    }

    fn boolean(&mut self, key: &'static str, value: bool) {
        self.list.push((key.into(), LynxValue::Boolean(value)));
    }

    fn time(&mut self, key: impl Into<Cow<'static, str>>, at: Timestamp) {
        let value = at.unix_millis(&self.origin);
        self.number(key, value);
    }

    fn range(&mut self, start: &'static str, end: &'static str, range: &Range<Timestamp>) {
        self.time(start, range.start);
        self.time(end, range.end);
    }

    /// Lynx's `PerformanceMetric` under `key`, in its member order.
    fn metric(&mut self, key: &'static str, metric: &'a Metric) {
        let start = metric.start.unix_millis(&self.origin);
        let end = metric.end.unix_millis(&self.origin);
        self.text(format!("{key}.name"), metric.name);
        self.number(format!("{key}.duration"), round_millis(end - start));
        self.text(format!("{key}.startTimestampName"), metric.start_name);
        self.number(format!("{key}.startTimestamp"), start);
        self.text(format!("{key}.endTimestampName"), metric.end_name);
        self.number(format!("{key}.endTimestamp"), end);
    }

    /// `frameworkRenderingTiming`: the framework's `dsl` and `stage`, a
    /// load's `vmExecute*`, and the framework's marks, each mark's key one
    /// escaped segment.
    fn framework(&mut self, framework: &'a FrameworkTiming, vm_execute: Option<&Range<Timestamp>>) {
        if let Some(dsl) = &framework.dsl {
            self.text("frameworkRenderingTiming.dsl", dsl);
        }
        if let Some(stage) = &framework.stage {
            self.text("frameworkRenderingTiming.stage", stage);
        }
        if let Some(execute) = vm_execute {
            self.time("frameworkRenderingTiming.vmExecuteStart", execute.start);
            self.time("frameworkRenderingTiming.vmExecuteEnd", execute.end);
        }
        for (key, at) in &framework.marks {
            self.time(
                format!("frameworkRenderingTiming.{}", escape_segment(key)),
                *at,
            );
        }
    }

    fn optional_metric(&mut self, key: &'static str, metric: Option<&'a Metric>) {
        if let Some(metric) = metric {
            self.metric(key, metric);
        }
    }
}

/// Every entry in `entries` as one `<utf16Length>:<text>` record, for the
/// BTS's `takePerformanceEntries`: per entry, its pair count, then each of
/// [`PerformanceEntry::lynx_fields`]'s pairs as a key field and a value
/// field, the value prefixed `n` for a number (three decimals), `b` for a
/// boolean (`b1` or `b0`) and `s` for text. Each key is written by
/// [`record_key`], and `bobcat:performance` folds it back into the object
/// its first segment names.
pub(crate) fn encode_entries(entries: &[PerformanceEntry], origin: &TimeOrigin) -> String {
    use crate::main::record::write_record_field;
    let mut record = String::new();
    let mut scratch = String::new();
    for entry in entries {
        let fields: Vec<_> = entry.lynx_fields(origin).collect();
        scratch.clear();
        write!(scratch, "{}", fields.len()).expect("writing to a String cannot fail");
        write_record_field(&mut record, &scratch);
        for (key, value) in fields {
            write_record_field(&mut record, &record_key(&key));
            scratch.clear();
            match value {
                LynxValue::Number(number) => {
                    write!(scratch, "n{number:.3}").expect("writing to a String cannot fail");
                }
                LynxValue::Double(number) => {
                    write!(scratch, "n{number}").expect("writing to a String cannot fail");
                }
                LynxValue::Boolean(value) => scratch.push_str(if value { "b1" } else { "b0" }),
                LynxValue::Text(text) => {
                    scratch.push('s');
                    scratch.push_str(text);
                }
            }
            write_record_field(&mut record, &scratch);
        }
    }
    record
}

#[cfg(test)]
mod tests;
