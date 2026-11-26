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

use smallvec::SmallVec;
pub use terminus::link::clock::ClockInstant as Instant;
pub use terminus::link::{NodeId, Rect};

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
pub use crate::clock::SystemTime;
pub use crate::jobs::{JobKind, JsThreadName};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub(crate) ClockInstant);

impl Timestamp {
    #[must_use]
    pub const fn instant(self) -> ClockInstant {
        self.0
    }

    #[must_use]
    pub fn unix_millis(&self, origin: &TimeOrigin) -> f64 {
        let offset = self
            .0
            .checked_duration_since(origin.instant)
            .map_or_else(|| -millis(origin.instant.duration_since(self.0)), millis);
        round_millis(origin.unix_millis + offset)
    }

    #[must_use]
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.0.saturating_duration_since(earlier.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimeOrigin {
    instant: ClockInstant,
    unix_millis: f64,
}

impl TimeOrigin {
    pub(crate) fn now() -> Self {
        let system = SystemTime::now();
        let instant = ClockInstant::now();
        Self {
            instant,
            unix_millis: round_millis(unix_millis_of(system)),
        }
    }

    #[must_use]
    pub const fn instant(&self) -> Timestamp {
        Timestamp(self.instant)
    }

    #[must_use]
    pub const fn unix_millis(&self) -> f64 {
        self.unix_millis
    }

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

fn round_millis(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn unix_millis_of(time: SystemTime) -> f64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => millis(after),
        Err(before) => -millis(before.duration()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metric {
    pub name: &'static str,
    pub start_name: &'static str,
    pub start: Timestamp,
    pub end_name: &'static str,
    pub end: Timestamp,
}

impl Metric {
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.end.duration_since(self.start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineOrigin {
    LoadBundle,
    ReloadBundleFromNative,
    ReloadBundleFromBts,
    UpdateTriggeredByBts,
    UpdateTriggeredByNative,
    UpdateGlobalProps,
    SetNativeProps,
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

    #[must_use]
    pub fn from_lynx_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|origin| origin.lynx_name() == name)
    }

    const fn is_reload(self) -> bool {
        matches!(
            self,
            Self::ReloadBundleFromNative | Self::ReloadBundleFromBts
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameworkTiming {
    pub dsl: Option<Arc<str>>,
    pub stage: Option<Arc<str>>,
    pub marks: Vec<(Arc<str>, Timestamp)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PainterTiming {
    pub compose: Range<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActualFmp {
    pub actual_fmp: Option<Metric>,
    pub lynx_actual_fmp: Metric,
    pub total_actual_fmp: Option<Metric>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineTiming {
    pub origin: PipelineOrigin,
    pub identifier: Option<Arc<str>>,
    pub pipeline_start: Timestamp,
    pub pipeline_end: Timestamp,
    pub mts_render: Option<Range<Timestamp>>,
    pub resolve: Range<Timestamp>,
    pub layout: Range<Timestamp>,
    pub painting_ui_operation_execute: Range<Timestamp>,
    pub layout_ui_operation_execute: Range<Timestamp>,
    pub paint_end: Timestamp,
    pub framework_rendering_timing: FrameworkTiming,
    pub host_platform_timing: PainterTiming,
    pub actual_fmp: Option<ActualFmp>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtraTiming {
    pub open_time: Option<SystemTime>,
    pub container_init_start: Option<SystemTime>,
    pub container_init_end: Option<SystemTime>,
    pub prepare_template_start: Option<SystemTime>,
    pub prepare_template_end: Option<SystemTime>,
}

impl ExtraTiming {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadBundleTiming {
    pub pipeline: PipelineTiming,
    pub url: Arc<str>,
    pub create_lynx: Range<Timestamp>,
    pub load_bundle: Range<Timestamp>,
    pub parse: Option<Range<Timestamp>>,
    pub vm_execute: Option<Range<Timestamp>>,
    pub load_core: Option<Range<Timestamp>>,
    pub load_background: Option<Range<Timestamp>>,
    pub extra: ExtraTiming,
    pub lynx_fcp: Metric,
    pub fcp: Option<Metric>,
    pub total_fcp: Option<Metric>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadBundleTiming {
    pub pipeline: PipelineTiming,
    pub url: Arc<str>,
    pub reload_bundle: Range<Timestamp>,
    pub reload_background: Option<Range<Timestamp>>,
    pub lynx_fcp: Metric,
    pub fcp: Option<Metric>,
    pub total_fcp: Option<Metric>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaintName {
    FirstPaint,
    FirstContentfulPaint,
}

impl PaintName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstPaint => "first-paint",
            Self::FirstContentfulPaint => "first-contentful-paint",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintEntry {
    pub name: PaintName,
    pub start: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LazyBundleEntry {
    pub component_url: Arc<str>,
    pub size_bytes: Option<u64>,
    pub load_success: bool,
    pub require: Range<Timestamp>,
    pub decode: Option<Range<Timestamp>>,
}

impl LazyBundleEntry {
    pub const MODE: &'static str = "normal";

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

#[derive(Clone, Debug, PartialEq)]
pub struct ContainerEntry {
    pub identifier: Arc<str>,
    pub root: NodeId,
    pub first_render: Timestamp,
    pub start: Timestamp,
    pub size: f32,
    pub intersection_rect: Rect<f32>,
    pub last_painted: NodeId,
}

impl ContainerEntry {
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

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PerformanceEntry {
    LoadBundle(Box<LoadBundleTiming>),
    ReloadBundle(Box<ReloadBundleTiming>),
    Pipeline(Box<PipelineTiming>),
    Paint(PaintEntry),
    LongTask(LongTaskEntry),
    JsBlocking(JsBlockingEntry),
    Memory(MemoryEntry),
    LazyBundle(LazyBundleEntry),
    Container(ContainerEntry),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EntryTypes(u8);

impl EntryTypes {
    pub const NONE: Self = Self(0);
    pub const PIPELINE: Self = Self(1);
    pub const LOAD_BUNDLE: Self = Self(1 << 1);
    pub const PAINT: Self = Self(1 << 2);
    pub const LONG_TASK: Self = Self(1 << 3);
    pub const JS_BLOCKING: Self = Self(1 << 4);
    pub const MEMORY: Self = Self(1 << 5);
    pub const RESOURCE: Self = Self(1 << 6);
    pub const CONTAINER: Self = Self(1 << 7);
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LynxValue<'a> {
    Number(f64),
    Double(f64),
    Boolean(bool),
    Text(&'a str),
}

pub(crate) const ACTUAL_FMP_FLAG: &str = "__lynx_timing_actual_fmp";

impl PerformanceEntry {
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

    pub fn write_lynx_json(&self, origin: &TimeOrigin, out: &mut String) {
        out.push('{');
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

fn member_separator(out: &mut String, has_member: &mut [bool]) {
    if let Some(innermost) = has_member.last_mut() {
        if *innermost {
            out.push(',');
        }
        *innermost = true;
    }
}

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

const RESOURCE_ENTRY_TYPE: &str = "resource";
const LAZY_BUNDLE_NAME: &str = "lazyBundle";

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

const fn lynx_task_type(kind: JobKind, thread: JsThreadName) -> Option<&'static str> {
    Some(match kind {
        JobKind::Start => match thread {
            JsThreadName::Main => "load_template_task",
            JsThreadName::Workers => "load_js_task",
        },
        JobKind::Entry => "load_template_task",
        JobKind::Module => "load_js_task",
        JobKind::Timer => "timer_task",
        JobKind::ContentVisibility
        | JobKind::Intersection
        | JobKind::ResizeObserver
        | JobKind::ComponentEvent
        | JobKind::Post
        | JobKind::ModuleCallback
        | JobKind::Performance => "js_func_task",
        JobKind::Command | JobKind::Future | JobKind::FontFace | JobKind::Memory => {
            "native_func_task"
        }
        JobKind::WorkerEvent => "update_data_by_js_task",
        JobKind::AnimationFrame => "animation_task",
        JobKind::Settle | JobKind::Release => return None,
    })
}

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
    if let Some(after) = report.time_after_load_bundle {
        fields.double("time_after_load_bundle", whole_millis(after));
    }
    fields.list
}

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

const CONTAINER_ENTRY_TYPE: &str = "container";

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

fn whole_millis(duration: Duration) -> f64 {
    u32::try_from(duration.as_millis()).map_or(f64::from(u32::MAX), f64::from)
}

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
