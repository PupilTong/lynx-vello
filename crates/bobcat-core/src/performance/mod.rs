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
//! (`monitors`), the long task and the JS blocking monitor: off unless the
//! Rust embedder turns them on, and reported to it alone.

mod members;
mod monitors;
mod registry;

use std::borrow::Cow;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

pub(crate) use self::members::{
    install_pipeline_members, install_worker_members, pipeline_argument,
};
pub(crate) use self::monitors::MonitorTask;
pub use self::monitors::{
    JsBlockingConfig, JsBlockingEntry, JsBlockingStage, LongTaskConfig, LongTaskEntry,
};
pub(crate) use self::registry::{
    FlushBinding, LoadMark, Opener, OpenerId, PipelineRegistry, WorkerAudience, WorkerLoadMark,
};
use crate::clock::{ClockInstant, SystemTime};
pub use crate::jobs::{JobKind, JsThreadName};

/// One instant on the engine's monotonic clock, comparable across every
/// thread of the process: a mark taken on the main thread, one taken on a
/// worker and one taken by the painter subtract (on wasm32 because each
/// engine thread's clock is aligned to the embedder thread's, see
/// `crate::clock`).
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
///
/// Both readings are taken on the embedder's thread, the one every engine
/// thread's clock is aligned to (`crate::clock`), so the monotonic one
/// needs no correction of its own, and the wall clock is read here and
/// nowhere else: no sample is ever a wall-clock reading.
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
    /// Every type.
    pub const ALL: Self = Self(
        Self::PIPELINE.0
            | Self::LOAD_BUNDLE.0
            | Self::PAINT.0
            | Self::LONG_TASK.0
            | Self::JS_BLOCKING.0,
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
    Text(&'a str),
}

/// What [`ACTUAL_FMP_FLAG`] fills.
pub(crate) const ACTUAL_FMP_FLAG: &str = "__lynx_timing_actual_fmp";

impl PerformanceEntry {
    /// The entry's `entryType`: Lynx's `"pipeline"` for the three pipeline
    /// entries, Paint Timing's `"paint"`, Long Tasks' `"longtask"`, and
    /// Lynx's `"js_blocking"`.
    #[must_use]
    pub const fn entry_type(&self) -> &'static str {
        match self {
            Self::LoadBundle(_) | Self::ReloadBundle(_) | Self::Pipeline(_) => "pipeline",
            Self::Paint(_) => "paint",
            Self::LongTask(_) => "longtask",
            Self::JsBlocking(_) => "js_blocking",
        }
    }

    /// The entry's `name`: the pipeline's origin, the paint mark's
    /// Performance Timeline name, the long task's [`JobKind`], or Lynx's
    /// `"js_blocking"` again.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Paint(paint) => paint.name.as_str(),
            Self::LongTask(task) => task.kind.as_str(),
            Self::JsBlocking(_) => "js_blocking",
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
            Self::Paint(_) | Self::LongTask(_) | Self::JsBlocking(_) => None,
        }
    }

    /// The entry in Lynx's keys and Lynx's units, in a fixed order:
    /// `entryType`, `name` and `identifier` (`""` for none) first, then the
    /// pipeline's marks, the load's or reload's, the metrics, and the two
    /// nested objects. A key with a dot is a member of the object its first
    /// segment names (`frameworkRenderingTiming.diffVdomStart`,
    /// `lynxFcp.duration`), and each object's members are adjacent. A field
    /// whose value this engine does not have is left out, not zeroed.
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
            Self::Pipeline(_) | Self::Paint(_) | Self::LongTask(_) | Self::JsBlocking(_) => {}
        }
        let framework = &pipeline.framework_rendering_timing;
        if let Some(dsl) = &framework.dsl {
            fields.text("frameworkRenderingTiming.dsl", dsl);
        }
        if let Some(stage) = &framework.stage {
            fields.text("frameworkRenderingTiming.stage", stage);
        }
        if let Some(execute) = vm_execute {
            fields.time("frameworkRenderingTiming.vmExecuteStart", execute.start);
            fields.time("frameworkRenderingTiming.vmExecuteEnd", execute.end);
        }
        for (key, at) in &framework.marks {
            fields.time(format!("frameworkRenderingTiming.{key}"), *at);
        }
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
    /// three decimals, dotted keys as nested objects. For an embedder that
    /// reports or compares entries; the engine itself never calls it.
    pub fn write_lynx_json(&self, origin: &TimeOrigin, out: &mut String) {
        out.push('{');
        let mut first = true;
        let mut open: Option<Cow<'static, str>> = None;
        let mut first_member = true;
        for (key, value) in self.lynx_fields(origin) {
            let (outer, inner) = match key.split_once('.') {
                Some((outer, inner)) => (Some(outer), inner),
                None => (None, key.as_ref()),
            };
            if open.as_deref() != outer {
                if open.take().is_some() {
                    out.push('}');
                }
                if let Some(outer) = outer {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_json_string(out, outer);
                    out.push_str(":{");
                    open = Some(Cow::Owned(outer.to_owned()));
                    first_member = true;
                }
            }
            if outer.is_some() {
                if !first_member {
                    out.push(',');
                }
                first_member = false;
            } else {
                if !first {
                    out.push(',');
                }
                first = false;
            }
            write_json_string(out, inner);
            out.push(':');
            write_lynx_value(out, value);
        }
        if open.is_some() {
            out.push('}');
        }
        out.push('}');
    }
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
        JobKind::Command | JobKind::Future | JobKind::FontFace => "native_func_task",
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

    fn double(&mut self, key: &'static str, value: f64) {
        self.list.push((key.into(), LynxValue::Double(value)));
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

    fn optional_metric(&mut self, key: &'static str, metric: Option<&'a Metric>) {
        if let Some(metric) = metric {
            self.metric(key, metric);
        }
    }
}

/// Every entry in `entries` as one `<utf16Length>:<text>` record, for the
/// BTS's `takePerformanceEntries`: per entry, its pair count, then each of
/// [`PerformanceEntry::lynx_fields`]'s pairs as a key field and a value
/// field, the value prefixed `n` for a number (three decimals) and `s` for
/// text. `bobcat:performance` folds the dotted keys back into objects.
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
            write_record_field(&mut record, &key);
            scratch.clear();
            match value {
                LynxValue::Number(number) => {
                    write!(scratch, "n{number:.3}").expect("writing to a String cannot fail");
                }
                LynxValue::Double(number) => {
                    write!(scratch, "n{number}").expect("writing to a String cannot fail");
                }
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
