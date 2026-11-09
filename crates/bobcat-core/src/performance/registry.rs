//! One view's pipeline registry: the records every thread of the view
//! writes its marks into, and the one place a record is completed and
//! handed to whoever observes it.
//!
//! Shared as an `Arc` by the view's main-thread realms, its workers, its
//! painter seat and the [`crate::LynxView`]. Each writer takes its
//! instant before the lock and holds the lock for one insert or one field
//! write; the painter's per-present check, the flush's "is anything waiting"
//! check and the pump's "did anything complete" check are each one atomic
//! read that takes no lock when the answer is no.
//!
//! A record's life is Lynx's `TimingHandlerNg`'s, cut where this engine's
//! threads are:
//!
//! 1. **Opened** by a `_onPipelineStart` (BTS or MTS), a native update that carries a
//!    `__lynx_timing_flag`, a reload, or a realm's load.
//! 2. **Bound** by the flush that commits it: the flush's commit id and a copy of its
//!    [`CommitTiming`]. A flush is the only binding point.
//! 3. **Presented** by the painter once a frame whose commit covers the binding has been presented;
//!    a load also waits for its BTS's entry to have loaded (Lynx's `loadBackgroundEnd` readiness),
//!    so the BTS's observers exist by the time its entry is reported.
//! 4. **Completed**: its audiences are fixed — the Rust embedder when it subscribed to the record's
//!    types, the BTS worker of the realm that opened it when that worker observes — and each takes
//!    it once. A record no audience wants is freed at once.
//!
//! A `paint` record (Paint Timing's `first-paint`, `first-contentful-paint`)
//! skips the first three: it is completed when the painter first reports the
//! mark, for the Rust embedder when it subscribed to [`EntryTypes::PAINT`]
//! and for each BTS worker whose observers asked for `paint`, and is not
//! made at all when nobody did.
//!
//! The registry also answers the two switches that keep the painting side's
//! timing work off when nothing needs it, each a relaxed atomic read:
//! [`PipelineRegistry::wants_content_detection`], the document's contentful
//! test, and [`PipelineRegistry::wants_presentation_timing`], the painter's
//! presentation records.
//!
//! Beside the records sits one delivery path for every entry made whole
//! where it is produced, a monitor's among them
//! ([`PipelineRegistry::deliver`]): one queue for the Rust embedder and one
//! per worker, each bounded by [`MAX_RECORDS`], which the takes append to
//! the records' entries in the order they were made. And the two monitors'
//! switches, each one relaxed atomic, and the JS blocking monitor's state.
//!
//! The memory monitor's readings land here too
//! ([`PipelineRegistry::note_memory`]): each category's latest one, and the
//! `memory` entries reported from them through the same delivery path, for
//! the Rust embedder alone.
//!
//! So do the `resource.lazyBundle` entries a realm's `fetchResource`
//! completes ([`PipelineRegistry::note_lazy_bundle`]), made only while the
//! Rust embedder observes [`EntryTypes::RESOURCE`] or a BTS observer asked
//! for `resource` ([`PipelineRegistry::times_resources`], one relaxed load
//! per fetch).
//!
//! And the `<frame>` cards' loads: each nested card's completed load record
//! is owed to that card's realm as Lynx's `loadmetrics` component event
//! ([`PipelineRegistry::take_frame_loads`]), whoever observes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use terminus::link::CommitTiming;
use terminus::link::painter::PresentedFrame;
use tokio::sync::watch;

use super::memory::{MemoryCategory, MemoryConfig, MemoryEntry, MemoryItem};
use super::monitors::{decode_threshold, encode_threshold};
use super::{
    ACTUAL_FMP_FLAG, ActualFmp, EntryTypes, ExtraTiming, FrameworkTiming, JsBlockingConfig,
    JsBlockingEntry, JsBlockingStage, LazyBundleEntry, LoadBundleTiming, Metric, PaintEntry,
    PaintName, PainterTiming, PerformanceEntry, PipelineOrigin, PipelineTiming, ReloadBundleTiming,
    TimeOrigin, Timestamp,
};
use crate::background::WorkerKey;
use crate::clock::{ClockInstant, SystemTime};

/// Names one realm over a view's document — the page realm, or a `<frame>`
/// card's `ShadowRealm` child — for the records it opens and the BTS it
/// connects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OpenerId(u64);

impl OpenerId {
    /// The page realm's: the one realm every view has.
    pub(crate) const PAGE: Self = Self(0);
}

/// Which realm ran the script that opened a record: a fact about provenance,
/// which decides which BTS hears about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opener {
    Realm(OpenerId),
    Worker(WorkerKey),
}

/// A realm's load milestone after its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadMark {
    /// The entry's evaluation settled.
    VmExecuteEnd,
    /// The first render began.
    MtsRenderStart,
}

/// A BTS worker's load milestone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerLoadMark {
    CoreStart,
    CoreEnd,
    BackgroundStart,
    BackgroundEnd,
}

impl WorkerLoadMark {
    /// The mark a worker realm names `key`; `loadCoreStart` is the host's,
    /// taken before the realm exists, so a realm cannot name it.
    pub(crate) fn from_realm_key(key: &str) -> Option<Self> {
        match key {
            "loadCoreEnd" => Some(Self::CoreEnd),
            "loadBackgroundStart" => Some(Self::BackgroundStart),
            "loadBackgroundEnd" => Some(Self::BackgroundEnd),
            _ => None,
        }
    }
}

/// What one timed flush hands the registry.
pub(crate) struct FlushBinding {
    /// The realm whose `__FlushElementTree` this was.
    pub(crate) opener: OpenerId,
    /// The `pipelineOptions.pipelineID` it was called with; 0 for none.
    pub(crate) pipeline: u64,
    /// Its `__lynx_timing_flag` option and the flag attributes set since
    /// the last flush, in that order.
    pub(crate) flags: Vec<Arc<str>>,
    /// The native-triggered pipelines opened since the last flush.
    pub(crate) native: Vec<u64>,
    /// Whether this is the realm's load's first flush after its render.
    pub(crate) load: bool,
    /// The frame the flush committed or bound to.
    pub(crate) commit_id: u64,
    /// That frame's commit timing, when this flush built it.
    pub(crate) timing: Option<CommitTiming>,
    /// When `__FlushElementTree` was called.
    pub(crate) started: ClockInstant,
}

/// What a worker's mark completed.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Completion {
    /// Whether a record completed, which the embedder's next `pump` reports.
    pub(crate) completed: bool,
    /// Whether the worker that made the mark is owed entries now.
    pub(crate) owed_to_caller: bool,
}

/// Which workers an entry handed to [`PipelineRegistry::deliver`] may
/// reach besides the Rust embedder, each only when its observers asked for
/// the entry's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerAudience {
    /// The Rust embedder alone.
    None,
    /// The BTS of the realm that produced the entry.
    One(WorkerKey),
    /// Every worker of the view: an entry about the view as a whole.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the `container` entries of phase 2 carry it")
    )]
    Every,
}

/// Lynx's bound on its timing map (`timing_map_exceeded_size`), and on
/// each queue of [`PipelineRegistry::deliver`].
const MAX_RECORDS: usize = 1000;
/// `oldest_waiting_commit` when no record waits for a present.
const NO_COMMIT: u64 = u64::MAX;
/// Audience bits.
const RUST: u8 = 1;
const WORKER: u8 = 1 << 1;

pub(crate) struct PipelineRegistry {
    origin: TimeOrigin,
    /// The next pipeline id; ids start at 1, and 0 means "none" in every
    /// host call.
    next_id: AtomicU64,
    /// The next `<frame>` realm's [`OpenerId`]; the page's is 0.
    next_opener: AtomicU64,
    /// The smallest commit id a bound record waits to see presented, or
    /// [`NO_COMMIT`]: the painter's one read per present.
    oldest_waiting_commit: AtomicU64,
    /// The newest commit id the painter presented, 0 for none: what a flush
    /// that bound to an already presented frame reads.
    last_presented_commit: AtomicU64,
    /// Bumped at each completion: `pump`'s one read per turn.
    generation: AtomicU64,
    /// Whether anything observes — the embedder subscribed to a type, or a
    /// BTS observer exists. Without it, a pipeline that does not carry
    /// timestamps when it opens is not recorded at all.
    observed: AtomicBool,
    /// How many records exist: a flush or a mark with none takes no lock.
    live_records: AtomicUsize,
    /// [`Self::wants_content_detection`]: someone observes `paint` and
    /// `first-contentful-paint` has not been recorded.
    content_detection: AtomicBool,
    /// Whether a paint mark still needs the painter's presentation records:
    /// [`Self::content_detection`], or the view's first paint not recorded
    /// yet. Half of [`Self::wants_presentation_timing`].
    marks_wanted: AtomicBool,
    /// The long task monitor's threshold (`monitors::encode_threshold`),
    /// 0 while it is off: every job reads it once.
    long_task_threshold: AtomicU64,
    /// The JS blocking monitor's, likewise: every message to a worker
    /// reads it once.
    js_blocking_threshold: AtomicU64,
    /// Bumped by every monitor setter, the new value being the new
    /// configuration's epoch, and notified without a bump when a monitor's
    /// state changes in a way its task waits for.
    monitor_epochs: watch::Sender<u64>,
    /// [`Self::times_resources`]: the Rust embedder observes
    /// [`EntryTypes::RESOURCE`] or a BTS observer asked for `resource`.
    resource_observed: AtomicBool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    records: FxHashMap<u64, Record>,
    /// Each realm's load, by the realm that loads.
    loads: FxHashMap<OpenerId, LoadRecord>,
    /// Each BTS worker's own load marks.
    worker_loads: FxHashMap<WorkerKey, WorkerLoadMarks>,
    /// Which realm each BTS worker belongs to (`connectBackground`).
    bindings: FxHashMap<WorkerKey, OpenerId>,
    /// The workers with a `lynx.performance` observer, each with the types
    /// its observers opt into beyond its realm's pipeline entries, which
    /// every observing worker receives.
    worker_types: FxHashMap<WorkerKey, EntryTypes>,
    /// Completed records some audience has not taken yet, in completion
    /// order.
    completed: Vec<u64>,
    /// Workers owed a `PerformanceReady` for a completion no painter
    /// present made, handed to whichever embedder-thread call comes next.
    wakes: Vec<WorkerKey>,
    extra: ExtraTiming,
    rust_types: EntryTypes,
    /// Every flag bound so far: Lynx reports a timing flag once.
    dispatched_flags: FxHashSet<Arc<str>>,
    /// The view's Paint Timing marks, as the painter reported them.
    first_paint: Option<ClockInstant>,
    first_contentful_paint: Option<ClockInstant>,
    /// `create_lynx_view`'s span, which the page realm's load reports.
    root_create_lynx: Option<(ClockInstant, ClockInstant)>,
    /// The present the page realm's load waited for, which a JS blocking
    /// monitor turned on after it counts `time_after_load_bundle` from.
    page_loaded: Option<ClockInstant>,
    /// The JS blocking monitor's running state; `None` while it is off.
    js_blocking: Option<JsBlockingState>,
    /// The memory monitor's readings; `None` while it is off.
    memory: Option<MemoryState>,
    /// The delivered entries the Rust embedder has not taken, oldest first,
    /// at most [`MAX_RECORDS`].
    owed_rust: VecDeque<PerformanceEntry>,
    /// The delivered entries each worker has not taken, likewise.
    owed_workers: FxHashMap<WorkerKey, VecDeque<PerformanceEntry>>,
    /// The `<frame>` card realms whose load completed and whose `loadmetrics`
    /// the embedder's next `pump` has not sent yet, in completion order.
    frame_loads_owed: Vec<OpenerId>,
}

/// The memory monitor's state: Lynx's `MemoryMonitor` members.
struct MemoryState {
    /// The configuration this state belongs to: its monitor epoch.
    epoch: u64,
    /// `memory_records_`: each category's latest reading, by
    /// [`MemoryCategory::index`].
    records: [Option<MemoryItem>; MemoryCategory::ALL.len()],
    /// `last_reported_size_bytes_`: the total the last report carried.
    last_reported_total: u64,
    /// The smallest change of the total that is reported, in bytes.
    threshold: u64,
    /// Whether the next reading is reported whatever it changed: the first
    /// one after the monitor was turned on.
    force_next: bool,
}

/// The JS blocking monitor's state between two reports: Lynx's
/// `JSBlockingMonitor` members.
struct JsBlockingState {
    /// The configuration this state belongs to: its monitor epoch.
    epoch: u64,
    /// `total_blocking_time_`: the waits counted since the last report.
    total_time: Duration,
    /// `total_blocking_count_`.
    total_count: u32,
    /// `last_report_time_`: the last report made, or the monitor's start.
    last_report: ClockInstant,
    /// `load_bundle_time_`: the `loadBundle` report's instant, or the
    /// page's load for a monitor turned on after it.
    load_bundle: Option<ClockInstant>,
    /// What the `timer` reports are scheduled from: the `loadBundle`
    /// report's instant, or the configuration's for a monitor turned on or
    /// changed after the page's load; `None` while neither has happened.
    timer_anchor: Option<ClockInstant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Update,
    Load,
    Reload,
    /// A Paint Timing mark, completed when it is made; its instant is the
    /// record's `start`.
    Paint(PaintName),
}

#[derive(Clone, Copy)]
struct Commit {
    id: u64,
    timing: Option<CommitTiming>,
    /// When the flush returned: the stages of a flush that built no frame.
    at: ClockInstant,
}

#[derive(Clone)]
struct Record {
    kind: Kind,
    origin: PipelineOrigin,
    dsl: Option<Arc<str>>,
    stage: Option<Arc<str>>,
    /// The flags bound to it, one `Pipeline` entry each.
    flags: SmallVec<[Arc<str>; 1]>,
    need_timestamps: bool,
    start: ClockInstant,
    marks: Vec<(Arc<str>, ClockInstant)>,
    mts_render: (Option<ClockInstant>, Option<ClockInstant>),
    opener: Opener,
    commit: Option<Commit>,
    presented: Option<PresentedFrame>,
    /// A load's or reload's end: when its flush returned.
    end: Option<ClockInstant>,
    /// The BTS's load marks, copied at completion.
    background: Option<WorkerLoadMarks>,
    /// A load entry sent again after `set_extra_timing`: only that entry.
    load_only: bool,
    done: bool,
    audience: u8,
    worker: Option<WorkerKey>,
}

impl Record {
    fn new(kind: Kind, origin: PipelineOrigin, opener: Opener, start: ClockInstant) -> Self {
        Self {
            kind,
            origin,
            dsl: None,
            stage: None,
            flags: SmallVec::new(),
            need_timestamps: kind != Kind::Update,
            start,
            marks: Vec::new(),
            mts_render: (None, None),
            opener,
            commit: None,
            presented: None,
            end: None,
            background: None,
            load_only: false,
            done: false,
            audience: 0,
            worker: None,
        }
    }

    /// The entry types the record yields.
    fn types(&self) -> EntryTypes {
        if matches!(self.kind, Kind::Paint(_)) {
            return EntryTypes::PAINT;
        }
        let mut types = EntryTypes::NONE;
        if self.kind != Kind::Update {
            types = types | EntryTypes::LOAD_BUNDLE;
        }
        if !self.flags.is_empty() && !self.load_only {
            types = types | EntryTypes::PIPELINE;
        }
        types
    }

    const fn waits_for_present(&self) -> bool {
        !self.done && self.presented.is_none() && self.commit.is_some()
    }
}

struct LoadRecord {
    url: Arc<str>,
    pipeline: u64,
    create_lynx: Option<(ClockInstant, ClockInstant)>,
    vm_execute: (ClockInstant, Option<ClockInstant>),
    /// The completed load pipeline, kept so `set_extra_timing` can send its
    /// entry again.
    completed: Option<Record>,
}

#[derive(Clone, Copy, Debug, Default)]
struct WorkerLoadMarks {
    load_core: (Option<ClockInstant>, Option<ClockInstant>),
    load_background: (Option<ClockInstant>, Option<ClockInstant>),
}

const fn default_origin(opener: Opener) -> PipelineOrigin {
    match opener {
        Opener::Worker(_) => PipelineOrigin::UpdateTriggeredByBts,
        Opener::Realm(_) => PipelineOrigin::UpdateTriggeredByNative,
    }
}

fn non_empty(text: &str) -> Option<Arc<str>> {
    (!text.is_empty()).then(|| Arc::from(text))
}

impl PipelineRegistry {
    pub(crate) fn new(origin: TimeOrigin) -> Self {
        Self {
            origin,
            next_id: AtomicU64::new(1),
            next_opener: AtomicU64::new(1),
            oldest_waiting_commit: AtomicU64::new(NO_COMMIT),
            last_presented_commit: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            observed: AtomicBool::new(false),
            live_records: AtomicUsize::new(0),
            content_detection: AtomicBool::new(false),
            // The view's first paint is not recorded yet.
            marks_wanted: AtomicBool::new(true),
            long_task_threshold: AtomicU64::new(0),
            js_blocking_threshold: AtomicU64::new(0),
            monitor_epochs: watch::channel(0).0,
            resource_observed: AtomicBool::new(false),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// A registry with an origin of its own, for a view nobody builds
    /// through `create_lynx_view`: the crate's tests and benchmarks.
    pub(crate) fn detached() -> Arc<Self> {
        Arc::new(Self::new(TimeOrigin::now()))
    }

    pub(crate) const fn time_origin(&self) -> TimeOrigin {
        self.origin
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn count(&self, inner: &Inner) {
        self.live_records
            .store(inner.records.len(), Ordering::Relaxed);
    }

    fn update_observed(&self, inner: &Inner) {
        self.observed.store(
            !inner.rust_types.is_empty() || !inner.worker_types.is_empty(),
            Ordering::Relaxed,
        );
        self.resource_observed
            .store(Self::wanted(inner, EntryTypes::RESOURCE), Ordering::Relaxed);
        let detect =
            Self::wanted(inner, EntryTypes::PAINT) && inner.first_contentful_paint.is_none();
        self.content_detection.store(detect, Ordering::Relaxed);
        self.marks_wanted
            .store(detect || inner.first_paint.is_none(), Ordering::Relaxed);
    }

    /// Whether anyone asked for `kind`'s entries: the Rust embedder, or a
    /// worker's observers.
    fn wanted(inner: &Inner, kind: EntryTypes) -> bool {
        inner.rust_types.contains(kind)
            || inner
                .worker_types
                .values()
                .any(|types| types.contains(kind))
    }

    /// Whether the view's document should decide Paint Timing's contentful
    /// test as it builds frames (`terminus`'s
    /// `Document::set_content_detection`): someone observes `paint` — the
    /// Rust embedder through [`EntryTypes::PAINT`], or a BTS observer of
    /// `paint` — and `first-contentful-paint` has not been recorded. One
    /// relaxed load, which every commit reads.
    pub(crate) fn wants_content_detection(&self) -> bool {
        self.content_detection.load(Ordering::Relaxed)
    }

    /// Whether the view's painter should record its presentations
    /// (`terminus`'s `Painter::set_presentation_timing`): a bound record
    /// waits for a present, the document detects content, or the view's
    /// first paint — which every load entry's `lynxFcp` ends at, and the
    /// `first-paint` entry reports — has not been recorded yet. Two relaxed
    /// loads and no lock, which every draw reads; a view nobody observes
    /// answers `false` from its first recorded present on, except while a
    /// record waits.
    pub(crate) fn wants_presentation_timing(&self) -> bool {
        self.oldest_waiting_commit.load(Ordering::Relaxed) != NO_COMMIT
            || self.marks_wanted.load(Ordering::Relaxed)
    }

    /// A fresh pipeline id: one atomic increment and nothing else.
    pub(crate) fn generate_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// A fresh id for a `<frame>` card's realm.
    pub(crate) fn allocate_opener(&self) -> OpenerId {
        OpenerId(self.next_opener.fetch_add(1, Ordering::Relaxed))
    }

    /// How many completions there have been.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// `_onPipelineStart`. A pipeline that carries no timestamps is
    /// recorded only while something observes: it may still be bound to a
    /// flag later, and then its start and its marks are wanted.
    pub(crate) fn open(
        &self,
        id: u64,
        origin: Option<PipelineOrigin>,
        dsl: &str,
        stage: &str,
        need_timestamps: bool,
        opener: Opener,
    ) {
        if id == 0 || (!need_timestamps && !self.observed.load(Ordering::Relaxed)) {
            return;
        }
        let now = ClockInstant::now();
        let origin = origin.unwrap_or(default_origin(opener));
        let mut inner = self.lock();
        if let Some(record) = inner.records.get_mut(&id) {
            if record.commit.is_none() {
                record.origin = origin;
                record.dsl = non_empty(dsl);
                record.stage = non_empty(stage);
                record.need_timestamps |= need_timestamps;
            }
            return;
        }
        let mut record = Record::new(Kind::Update, origin, opener, now);
        record.dsl = non_empty(dsl);
        record.stage = non_empty(stage);
        record.need_timestamps = need_timestamps;
        self.insert(&mut inner, id, record);
    }

    /// `_markTiming`: `mtsRenderStart`/`mtsRenderEnd` are the record's own
    /// render; every other key is a framework mark, the last one per key
    /// kept.
    pub(crate) fn mark(&self, id: u64, key: &str) {
        if id == 0 || self.live_records.load(Ordering::Relaxed) == 0 {
            return;
        }
        let now = ClockInstant::now();
        let mut inner = self.lock();
        let Some(record) = inner.records.get_mut(&id) else {
            return;
        };
        if record.done {
            return;
        }
        match key {
            "mtsRenderStart" => record.mts_render.0 = Some(now),
            "mtsRenderEnd" => record.mts_render.1 = Some(now),
            _ => match record.marks.iter_mut().find(|(name, _)| **name == *key) {
                Some(mark) => mark.1 = now,
                None => record.marks.push((Arc::from(key), now)),
            },
        }
    }

    /// `_bindPipelineIdWithTimingFlag`: the pipeline carries timestamps, and
    /// reports `flag`.
    pub(crate) fn bind_flag(&self, id: u64, flag: &str, opener: Opener) {
        if id == 0 || flag.is_empty() {
            return;
        }
        let now = ClockInstant::now();
        let flag: Arc<str> = Arc::from(flag);
        let mut inner = self.lock();
        match inner.records.get_mut(&id) {
            Some(record) if !record.done => {
                record.need_timestamps = true;
                if !record.flags.contains(&flag) {
                    record.flags.push(flag);
                }
            }
            Some(_) => {}
            None => {
                let mut record = Record::new(Kind::Update, default_origin(opener), opener, now);
                record.need_timestamps = true;
                record.flags.push(flag);
                self.insert(&mut inner, id, record);
            }
        }
    }

    /// A native update or reload arriving with a flag, or any reload: opened
    /// now, bound by the realm's next flush. Answers its id.
    pub(crate) fn open_native(&self, origin: PipelineOrigin, flag: &str, opener: OpenerId) -> u64 {
        let now = ClockInstant::now();
        let id = self.generate_id();
        let kind = if origin.is_reload() {
            Kind::Reload
        } else {
            Kind::Update
        };
        let mut record = Record::new(kind, origin, Opener::Realm(opener), now);
        record.need_timestamps = true;
        if let Some(flag) = non_empty(flag) {
            record.flags.push(flag);
        }
        let mut inner = self.lock();
        self.insert(&mut inner, id, record);
        id
    }

    /// Whether the pipeline a flush names carries timestamps. One whose
    /// record carries none, in a flush nothing else times, is dropped here:
    /// it will never be bound.
    pub(crate) fn claim_for_flush(&self, id: u64, others: bool) -> bool {
        if id == 0 || self.live_records.load(Ordering::Relaxed) == 0 {
            return false;
        }
        let mut inner = self.lock();
        let Some(record) = inner.records.get(&id) else {
            return false;
        };
        if record.commit.is_some() {
            return false;
        }
        if record.need_timestamps || !record.flags.is_empty() {
            return true;
        }
        if !others {
            inner.records.remove(&id);
            self.count(&inner);
        }
        false
    }

    /// `create_lynx_view`'s span, for the page realm's load.
    pub(crate) fn set_root_create_lynx(&self, start: ClockInstant, end: ClockInstant) {
        self.lock().root_create_lynx = Some((start, end));
    }

    /// A realm's load begins: its `loadBundleStart` and `vmExecuteStart`.
    /// `created` is a `<frame>` card's realm construction; the page's is
    /// `create_lynx_view`'s. A realm loads once.
    pub(crate) fn open_load(
        &self,
        opener: OpenerId,
        url: &str,
        created: Option<(ClockInstant, ClockInstant)>,
    ) {
        let now = ClockInstant::now();
        let id = self.generate_id();
        let mut inner = self.lock();
        if inner.loads.contains_key(&opener) {
            return;
        }
        let create_lynx = created.or(if opener == OpenerId::PAGE {
            inner.root_create_lynx
        } else {
            None
        });
        inner.loads.insert(
            opener,
            LoadRecord {
                url: Arc::from(url),
                pipeline: id,
                create_lynx,
                vm_execute: (now, None),
                completed: None,
            },
        );
        let record = Record::new(
            Kind::Load,
            PipelineOrigin::LoadBundle,
            Opener::Realm(opener),
            now,
        );
        self.insert(&mut inner, id, record);
    }

    /// One of a realm's load milestones. Answers, for
    /// [`LoadMark::MtsRenderStart`], whether the load's render began now —
    /// the realm's next flush then ends the load.
    pub(crate) fn mark_load(&self, opener: OpenerId, mark: LoadMark) -> bool {
        let now = ClockInstant::now();
        let mut inner = self.lock();
        let inner = &mut *inner;
        let Some(load) = inner.loads.get_mut(&opener) else {
            return false;
        };
        match mark {
            LoadMark::VmExecuteEnd => {
                load.vm_execute.1.get_or_insert(now);
                false
            }
            LoadMark::MtsRenderStart => match inner.records.get_mut(&load.pipeline) {
                Some(record) if record.commit.is_none() && record.mts_render.0.is_none() => {
                    record.mts_render.0 = Some(now);
                    true
                }
                _ => false,
            },
        }
    }

    /// Binds a timed flush: the pipeline it names, the native pipelines
    /// and the load it closes, and its flags — each flag to the record a
    /// `_bindPipelineIdWithTimingFlag` gave it, or else to the flush's own
    /// pipeline, or else to a new record of its own. A record that reports
    /// nothing is dropped. When the flush bound to a frame already
    /// presented, the records complete here. Answers whether anything
    /// completed.
    pub(crate) fn bind_commit(&self, binding: FlushBinding) -> bool {
        let ended = ClockInstant::now();
        let FlushBinding {
            opener,
            pipeline,
            flags,
            native,
            load,
            commit_id,
            timing,
            started,
        } = binding;
        let generation = self.generation.load(Ordering::Relaxed);
        let mut guard = self.lock();
        let inner = &mut *guard;
        let flags: SmallVec<[Arc<str>; 2]> = flags
            .into_iter()
            .filter(|flag| !flag.is_empty() && inner.dispatched_flags.insert(Arc::clone(flag)))
            .collect();
        let mut targets: SmallVec<[u64; 4]> = SmallVec::new();
        let mut primary = None;
        if pipeline != 0 {
            match inner.records.get(&pipeline) {
                Some(record) if record.commit.is_none() => {
                    targets.push(pipeline);
                    primary = Some(pipeline);
                }
                // Not recorded when it opened, because nothing observed and
                // it carried no timestamps: its flags still report.
                None if !flags.is_empty() => {
                    let record = Record::new(
                        Kind::Update,
                        PipelineOrigin::UpdateTriggeredByBts,
                        Opener::Realm(opener),
                        started,
                    );
                    self.insert(inner, pipeline, record);
                    targets.push(pipeline);
                    primary = Some(pipeline);
                }
                _ => {}
            }
        }
        for id in native {
            if inner
                .records
                .get(&id)
                .is_some_and(|record| record.commit.is_none())
            {
                targets.push(id);
                primary.get_or_insert(id);
            }
        }
        if load
            && let Some(id) = inner.loads.get(&opener).map(|load| load.pipeline)
            && let Some(record) = inner.records.get_mut(&id)
            && record.commit.is_none()
        {
            record.mts_render.1 = Some(started);
            targets.push(id);
            primary.get_or_insert(id);
        }
        self.attach_flags(inner, flags, &mut targets, primary, opener, started);
        let bound = Self::bind_targets(
            inner,
            &targets,
            Commit {
                id: commit_id,
                timing,
                at: ended,
            },
        );
        self.count(inner);
        if !bound {
            return false;
        }
        // Sequentially consistent with the painter's two accesses in
        // `note_presented`, so of a present and this binding racing, at
        // least one sees the other: this one completes the records itself,
        // or the painter finds them waiting.
        self.oldest_waiting_commit
            .fetch_min(commit_id, Ordering::SeqCst);
        if self.last_presented_commit.load(Ordering::SeqCst) >= commit_id {
            // The frame was already presented; the binding is the closest
            // reading this side has of that present.
            self.present_through(
                inner,
                PresentedFrame {
                    commit_id,
                    compose_start: ended,
                    compose_end: ended,
                    presented: ended,
                },
            );
        }
        self.generation.load(Ordering::Relaxed) != generation
    }

    /// Gives each flag of a flush its record: the one a
    /// `_bindPipelineIdWithTimingFlag` bound it to, or else `primary`, the
    /// flush's own pipeline, or else a new record of its own, which then
    /// takes the flags after it too. Each record a flag lands on is added to
    /// `targets`.
    fn attach_flags(
        &self,
        inner: &mut Inner,
        flags: SmallVec<[Arc<str>; 2]>,
        targets: &mut SmallVec<[u64; 4]>,
        mut primary: Option<u64>,
        opener: OpenerId,
        started: ClockInstant,
    ) {
        for flag in flags {
            let owner = inner
                .records
                .iter()
                .find(|(_, record)| record.commit.is_none() && record.flags.contains(&flag))
                .map(|(id, _)| *id);
            if let Some(owner) = owner {
                if !targets.contains(&owner) {
                    targets.push(owner);
                }
                continue;
            }
            let target = if let Some(id) = primary {
                id
            } else {
                let id = self.generate_id();
                let record = Record::new(
                    Kind::Update,
                    PipelineOrigin::UpdateTriggeredByNative,
                    Opener::Realm(opener),
                    started,
                );
                self.insert(inner, id, record);
                targets.push(id);
                primary = Some(id);
                id
            };
            if let Some(record) = inner.records.get_mut(&target) {
                record.flags.push(flag);
            }
        }
    }

    /// Binds each target to `commit`, dropping an update that reports no
    /// flag. Answers whether any was bound.
    fn bind_targets(inner: &mut Inner, targets: &[u64], commit: Commit) -> bool {
        let mut bound = false;
        for id in targets {
            let Some(record) = inner.records.get_mut(id) else {
                continue;
            };
            if record.kind == Kind::Update && record.flags.is_empty() {
                inner.records.remove(id);
                continue;
            }
            record.commit = Some(commit);
            if record.kind != Kind::Update {
                record.end = Some(commit.at);
            }
            bound = true;
        }
        bound
    }

    /// The painter presented `frame`: completes every record waiting for a
    /// commit it covers, and answers the workers owed a `PerformanceReady`.
    /// Without a waiting record it is two atomic accesses and no lock.
    pub(crate) fn note_presented(&self, frame: PresentedFrame) -> Vec<WorkerKey> {
        self.last_presented_commit
            .fetch_max(frame.commit_id, Ordering::SeqCst);
        if self.oldest_waiting_commit.load(Ordering::SeqCst) > frame.commit_id {
            return Vec::new();
        }
        let mut inner = self.lock();
        self.present_through(&mut inner, frame);
        std::mem::take(&mut inner.wakes)
    }

    /// The painter drew the frame of `commit_id` while its presentation
    /// timing was off ([`Self::wants_presentation_timing`] said no record
    /// waited). A flush may have bound a record to that commit since the
    /// painter asked: then the record completes here, presented now, as a
    /// binding to an already presented frame does. Without a waiting record
    /// it is two atomic accesses and no lock, as [`Self::note_presented`]
    /// is.
    pub(crate) fn note_untimed_present(&self, commit_id: u64) -> Vec<WorkerKey> {
        self.last_presented_commit
            .fetch_max(commit_id, Ordering::SeqCst);
        if self.oldest_waiting_commit.load(Ordering::SeqCst) > commit_id {
            return Vec::new();
        }
        let now = ClockInstant::now();
        let mut inner = self.lock();
        self.present_through(
            &mut inner,
            PresentedFrame {
                commit_id,
                compose_start: now,
                compose_end: now,
                presented: now,
            },
        );
        std::mem::take(&mut inner.wakes)
    }

    /// The view's Paint Timing marks, as the painter first saw them. Each
    /// is kept once, and the first time each is seen it completes one
    /// `paint` record for every audience that asks for `paint` then.
    /// Answers the workers owed a `PerformanceReady` for them.
    pub(crate) fn note_paint(
        &self,
        first_paint: Option<PresentedFrame>,
        first_contentful_paint: Option<PresentedFrame>,
    ) -> Vec<WorkerKey> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        if inner.first_paint.is_none()
            && let Some(frame) = first_paint
        {
            inner.first_paint = Some(frame.presented);
            self.complete_paint(inner, PaintName::FirstPaint, frame.presented);
        }
        if inner.first_contentful_paint.is_none()
            && let Some(frame) = first_contentful_paint
        {
            inner.first_contentful_paint = Some(frame.presented);
            self.complete_paint(inner, PaintName::FirstContentfulPaint, frame.presented);
        }
        self.update_observed(inner);
        std::mem::take(&mut inner.wakes)
    }

    /// One completed `paint` record per audience: the Rust embedder's, when
    /// it subscribed to [`EntryTypes::PAINT`], and one per worker observing
    /// `paint` — the mark is the view's, so every such worker hears it. No
    /// record at all when nobody asks.
    fn complete_paint(&self, inner: &mut Inner, name: PaintName, at: ClockInstant) {
        let mut audiences: SmallVec<[(u8, Option<WorkerKey>); 2]> = SmallVec::new();
        if inner.rust_types.contains(EntryTypes::PAINT) {
            audiences.push((RUST, None));
        }
        let workers = Self::workers_asking(inner, EntryTypes::PAINT);
        audiences.extend(workers.into_iter().map(|key| (WORKER, Some(key))));
        for (audience, worker) in audiences {
            let mut record = Record::new(
                Kind::Paint(name),
                PipelineOrigin::LoadBundle,
                Opener::Realm(OpenerId::PAGE),
                at,
            );
            record.done = true;
            record.audience = audience;
            record.worker = worker;
            let id = self.generate_id();
            self.insert(inner, id, record);
            inner.completed.push(id);
            if let Some(worker) = worker
                && !inner.wakes.contains(&worker)
            {
                inner.wakes.push(worker);
            }
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// The workers owed a `PerformanceReady` for completions no present
    /// made: a binding to a frame already presented, `set_extra_timing`.
    pub(crate) fn take_wakes(&self) -> Vec<WorkerKey> {
        std::mem::take(&mut self.lock().wakes)
    }

    fn present_through(&self, inner: &mut Inner, frame: PresentedFrame) {
        let mut presented: SmallVec<[u64; 4]> = SmallVec::new();
        for (id, record) in &mut inner.records {
            if record.waits_for_present()
                && record
                    .commit
                    .is_some_and(|commit| commit.id <= frame.commit_id)
            {
                record.presented = Some(frame);
                presented.push(*id);
            }
        }
        // In opening order, so the entries of one present are reported in
        // the order their pipelines began.
        presented.sort_unstable();
        for id in presented {
            self.try_complete(inner, id);
        }
        let oldest = inner
            .records
            .values()
            .filter(|record| record.waits_for_present())
            .filter_map(|record| record.commit.map(|commit| commit.id))
            .min()
            .unwrap_or(NO_COMMIT);
        self.oldest_waiting_commit.store(oldest, Ordering::SeqCst);
    }

    /// The BTS worker connected to `opener`'s realm.
    fn background_of(inner: &Inner, opener: OpenerId) -> Option<WorkerKey> {
        inner
            .bindings
            .iter()
            .find(|(_, realm)| **realm == opener)
            .map(|(key, _)| *key)
    }

    /// The realm whose load a record's entries measure against.
    fn realm_of(inner: &Inner, opener: Opener) -> Option<OpenerId> {
        match opener {
            Opener::Realm(realm) => Some(realm),
            Opener::Worker(key) => inner.bindings.get(&key).copied(),
        }
    }

    /// Completes a presented record, unless it is a load whose BTS has not
    /// loaded its entry yet.
    fn try_complete(&self, inner: &mut Inner, id: u64) {
        let Some(record) = inner.records.get(&id) else {
            return;
        };
        if record.done || record.presented.is_none() {
            return;
        }
        let mut background = None;
        if record.kind == Kind::Load
            && let Some(realm) = Self::realm_of(inner, record.opener)
            && let Some(worker) = Self::background_of(inner, realm)
        {
            let marks = inner.worker_loads.get(&worker).copied().unwrap_or_default();
            if marks.load_background.1.is_none() {
                return;
            }
            background = Some(marks);
        }
        self.complete(inner, id, background);
    }

    /// Fixes a record's audiences, and frees it when it has none.
    fn complete(&self, inner: &mut Inner, id: u64, background: Option<WorkerLoadMarks>) {
        let Some(record) = inner.records.get(&id) else {
            return;
        };
        let realm = Self::realm_of(inner, record.opener);
        let worker = match record.opener {
            Opener::Worker(key) => Some(key),
            Opener::Realm(realm) => Self::background_of(inner, realm),
        }
        .filter(|key| inner.worker_types.contains_key(key));
        let rust_types = inner.rust_types;
        let Some(record) = inner.records.get_mut(&id) else {
            return;
        };
        record.done = true;
        if background.is_some() {
            record.background = background;
        }
        let types = record.types();
        let mut audience = 0;
        if rust_types.intersects(types) {
            audience |= RUST;
        }
        if worker.is_some() && !types.is_empty() {
            audience |= WORKER;
        }
        record.audience = audience;
        record.worker = worker;
        let mut page_loaded = None;
        let mut frame_load = None;
        if record.kind == Kind::Load && !record.load_only {
            let snapshot = record.clone();
            match realm {
                Some(OpenerId::PAGE) => {
                    page_loaded = record.presented.map(|frame| frame.presented);
                }
                // A `<frame>` card's own load: Lynx's `FramePerformanceClient`
                // forwards exactly this entry to the frame as `loadmetrics`
                // (`LynxFrameView.java:456-477`), whatever the host observes,
                // so it is owed whoever observes — one per card load.
                Some(card) => frame_load = Some(card),
                None => {}
            }
            if let Some(load) = realm.and_then(|realm| inner.loads.get_mut(&realm)) {
                load.completed = Some(snapshot);
            }
        }
        if let Some(card) = frame_load {
            inner.frame_loads_owed.push(card);
            if audience == 0 {
                // What the embedder's next `pump` reads to look at all.
                self.generation.fetch_add(1, Ordering::Release);
            }
        }
        if audience == 0 {
            inner.records.remove(&id);
        } else {
            inner.completed.push(id);
            if audience & WORKER != 0
                && let Some(worker) = worker
                && !inner.wakes.contains(&worker)
            {
                inner.wakes.push(worker);
            }
            self.generation.fetch_add(1, Ordering::Release);
        }
        self.count(inner);
        if let Some(presented) = page_loaded {
            self.page_loaded(inner, presented);
        }
    }

    /// The page realm's load completed, presented at `presented`: Lynx's
    /// `JSBlockingMonitor::OnPerformanceEvent`, which makes the `loadBundle`
    /// report and starts the timer from now. Once: a load entry sent again
    /// by `set_extra_timing` starts no second timer, as Lynx's would.
    fn page_loaded(&self, inner: &mut Inner, presented: ClockInstant) {
        if inner.page_loaded.is_some() {
            return;
        }
        inner.page_loaded = Some(presented);
        let now = ClockInstant::now();
        let Some(state) = inner.js_blocking.as_mut() else {
            return;
        };
        state.timer_anchor = Some(now);
        self.report_blocking(inner, JsBlockingStage::LoadBundle, now);
        // Wakes the report task, which waits for the anchor.
        self.monitor_epochs.send_modify(|_| {});
    }

    /// The workers whose observers asked for `kind`, in key order.
    fn workers_asking(inner: &Inner, kind: EntryTypes) -> SmallVec<[WorkerKey; 2]> {
        let mut workers: SmallVec<[WorkerKey; 2]> = inner
            .worker_types
            .iter()
            .filter(|(_, types)| types.contains(kind))
            .map(|(key, _)| *key)
            .collect();
        workers.sort_unstable();
        workers
    }

    /// The one delivery path for an entry made whole where it was produced:
    /// queued for the Rust embedder when it observes `kind`, and for each
    /// worker of `workers` that asked for `kind`, which is then owed a
    /// `PerformanceReady` ([`Self::take_wakes`]). Answers whether it was
    /// queued anywhere, which is when the caller wakes the embedder.
    fn deliver(
        &self,
        inner: &mut Inner,
        entry: PerformanceEntry,
        kind: EntryTypes,
        workers: WorkerAudience,
    ) -> bool {
        let workers = match workers {
            WorkerAudience::None => SmallVec::new(),
            WorkerAudience::One(key) => Self::workers_asking(inner, kind)
                .into_iter()
                .filter(|asking| *asking == key)
                .collect(),
            WorkerAudience::Every => Self::workers_asking(inner, kind),
        };
        let rust = inner.rust_types.contains(kind);
        if !rust && workers.is_empty() {
            return false;
        }
        for key in workers {
            push_bounded(inner.owed_workers.entry(key).or_default(), entry.clone());
            if !inner.wakes.contains(&key) {
                inner.wakes.push(key);
            }
        }
        if rust {
            push_bounded(&mut inner.owed_rust, entry);
        }
        self.generation.fetch_add(1, Ordering::Release);
        true
    }

    /// [`Self::deliver`] under a lock of its own.
    pub(crate) fn deliver_entry(
        &self,
        entry: PerformanceEntry,
        kind: EntryTypes,
        workers: WorkerAudience,
    ) -> bool {
        self.deliver(&mut self.lock(), entry, kind, workers)
    }

    /// A new monitor configuration's epoch; every monitor task wakes.
    fn next_monitor_epoch(&self) -> u64 {
        let mut epoch = 0;
        self.monitor_epochs.send_modify(|current| {
            *current += 1;
            epoch = *current;
        });
        epoch
    }

    /// A reader of the monitor epochs, which a monitor task waits on.
    pub(crate) fn monitor_epochs(&self) -> watch::Receiver<u64> {
        self.monitor_epochs.subscribe()
    }

    /// The long task monitor's threshold, `None` while it is off: one
    /// relaxed load, which every job of the view's realms reads.
    pub(crate) fn long_task_threshold(&self) -> Option<Duration> {
        decode_threshold(self.long_task_threshold.load(Ordering::Relaxed))
    }

    /// Turns the long task monitor on with `threshold`, or off.
    pub(crate) fn set_long_task_threshold(&self, threshold: Option<Duration>) {
        self.long_task_threshold
            .store(encode_threshold(threshold), Ordering::Relaxed);
        self.next_monitor_epoch();
    }

    /// The instant a message sent to a worker now carries for the JS
    /// blocking monitor, `None` while it is off: one relaxed load, and a
    /// clock reading only while it is on.
    pub(crate) fn js_blocking_stamp(&self) -> Option<ClockInstant> {
        (self.js_blocking_threshold.load(Ordering::Relaxed) != 0).then(ClockInstant::now)
    }

    /// A message's job waited `wait` for its worker's thread: Lynx's
    /// `AddJSBlockingTime`, which counts a wait of at least the threshold.
    pub(crate) fn note_js_blocking(&self, wait: Duration) {
        let Some(threshold) = decode_threshold(self.js_blocking_threshold.load(Ordering::Relaxed))
        else {
            return;
        };
        if wait < threshold {
            return;
        }
        if let Some(state) = self.lock().js_blocking.as_mut() {
            state.total_time = state.total_time.saturating_add(wait);
            state.total_count = state.total_count.saturating_add(1);
        }
    }

    /// Turns the JS blocking monitor on with `config` at `now`, or off, and
    /// answers the epoch a report task for the new configuration serves —
    /// `None` when it is off. Turning it on starts its totals and its first
    /// report's span; changing a monitor that is on keeps them.
    pub(crate) fn set_js_blocking_monitor(
        &self,
        config: Option<JsBlockingConfig>,
        now: ClockInstant,
    ) -> Option<u64> {
        let mut inner = self.lock();
        self.js_blocking_threshold.store(
            encode_threshold(config.map(|config| config.threshold)),
            Ordering::Relaxed,
        );
        // Under the lock, so a task that wakes for the bump reads the new
        // state.
        let epoch = self.next_monitor_epoch();
        if config.is_none() {
            inner.js_blocking = None;
            return None;
        }
        let page_loaded = inner.page_loaded;
        let state = inner.js_blocking.get_or_insert(JsBlockingState {
            epoch,
            total_time: Duration::ZERO,
            total_count: 0,
            last_report: now,
            load_bundle: page_loaded,
            timer_anchor: None,
        });
        state.epoch = epoch;
        // The timer reports start at the `loadBundle` report, which is still
        // to come while the page has not loaded. A monitor turned on, or
        // changed, after it — which Lynx, whose monitor exists from the
        // start, never has — starts them from now.
        state.timer_anchor = state.load_bundle.map(|_| now);
        Some(epoch)
    }

    /// Whether `epoch` is the JS blocking monitor's current configuration.
    pub(crate) fn js_blocking_is_current(&self, epoch: u64) -> bool {
        self.lock()
            .js_blocking
            .as_ref()
            .is_some_and(|state| state.epoch == epoch)
    }

    /// The instant the `timer` reports of the configuration of `epoch` are
    /// scheduled from, once there is one; `None` too when that
    /// configuration is not the current one.
    pub(crate) fn js_blocking_anchor(&self, epoch: u64) -> Option<ClockInstant> {
        self.lock()
            .js_blocking
            .as_ref()
            .filter(|state| state.epoch == epoch)
            .and_then(|state| state.timer_anchor)
    }

    /// The JS blocking monitor's report task, for the configuration of
    /// `epoch`, owes a `timer` report now. Answers whether one was kept
    /// for the Rust embedder, which is when it is owed a wake.
    pub(crate) fn report_js_blocking(&self, epoch: u64) -> bool {
        let mut inner = self.lock();
        let current = inner.js_blocking.as_ref().map(|state| state.epoch) == Some(epoch);
        current && self.report_blocking(&mut inner, JsBlockingStage::Timer, ClockInstant::now())
    }

    /// A report of `stage` at `now`, for a test that drives the monitor
    /// with instants of its own.
    #[cfg(test)]
    pub(crate) fn report_js_blocking_at(&self, stage: JsBlockingStage, now: ClockInstant) -> bool {
        let mut inner = self.lock();
        self.report_blocking(&mut inner, stage, now)
    }

    /// Lynx's `ReportBlockingInfo`: nothing, and the span not restarted,
    /// when nothing was counted; otherwise one report, for the Rust embedder
    /// alone, after which the totals and the span start again at `now`, as
    /// Lynx resets them. Answers [`Self::deliver`]'s answer.
    fn report_blocking(
        &self,
        inner: &mut Inner,
        stage: JsBlockingStage,
        now: ClockInstant,
    ) -> bool {
        let Some(monitor) = inner.js_blocking.as_mut() else {
            return false;
        };
        if stage == JsBlockingStage::LoadBundle {
            monitor.load_bundle = Some(now);
        }
        if monitor.total_count == 0 || monitor.total_time.is_zero() {
            return false;
        }
        let entry = JsBlockingEntry::from_totals(
            stage,
            monitor.total_time,
            monitor.total_count,
            now.saturating_duration_since(monitor.last_report),
            match stage {
                JsBlockingStage::LoadBundle => None,
                JsBlockingStage::Timer => monitor
                    .load_bundle
                    .map(|load| now.saturating_duration_since(load)),
            },
        );
        monitor.last_report = now;
        monitor.total_time = Duration::ZERO;
        monitor.total_count = 0;
        self.deliver(
            inner,
            PerformanceEntry::JsBlocking(entry),
            EntryTypes::JS_BLOCKING,
            WorkerAudience::None,
        )
    }

    /// Turns the memory monitor on with `config`, or off, and answers the
    /// epoch a sampling task for the new configuration serves — `None` when
    /// it is off. Each call starts the readings afresh: the categories read
    /// so far are dropped and the next reading is reported whatever it
    /// changed.
    pub(crate) fn set_memory_monitor(&self, config: Option<MemoryConfig>) -> Option<u64> {
        let mut inner = self.lock();
        // Under the lock, so a task that wakes for the bump reads the new
        // state.
        let epoch = self.next_monitor_epoch();
        inner.memory = config.map(|config| MemoryState {
            epoch,
            records: Default::default(),
            last_reported_total: 0,
            threshold: config.change_threshold,
            force_next: true,
        });
        config.map(|_| epoch)
    }

    /// Whether `epoch` is the memory monitor's current configuration: what
    /// its task and each sample check before they read anything.
    pub(crate) fn memory_is_current(&self, epoch: u64) -> bool {
        self.lock()
            .memory
            .as_ref()
            .is_some_and(|state| state.epoch == epoch)
    }

    /// The BTS worker of the view's page realm, which a memory sample asks
    /// for the background thread's reading.
    pub(crate) fn page_background(&self) -> Option<WorkerKey> {
        Self::background_of(&self.lock(), OpenerId::PAGE)
    }

    /// One category's reading: Lynx's `UpdateMemoryUsage` and then
    /// `ReportMemory`. A reading whose size is the one its category last
    /// had is dropped, as Lynx drops it, unless it is the first since the
    /// monitor was turned on; any other replaces the category's reading,
    /// and a report of every category's latest one is delivered, to the
    /// Rust embedder alone, when the total moved by at least the threshold
    /// since the last report (0 reports every change), or the total is 0,
    /// or the reading is the first. Answers [`Self::deliver`]'s answer;
    /// nothing at all while the monitor is off.
    pub(crate) fn note_memory(&self, item: MemoryItem) -> bool {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(state) = inner.memory.as_mut() else {
            return false;
        };
        let force = std::mem::take(&mut state.force_next);
        let slot = &mut state.records[item.category.index()];
        if !force
            && slot
                .as_ref()
                .is_some_and(|last| last.size_bytes == item.size_bytes)
        {
            return false;
        }
        *slot = Some(item);
        let total = state
            .records
            .iter()
            .flatten()
            .fold(0_u64, |total, item| total.saturating_add(item.size_bytes));
        if !force && total > 0 && total.abs_diff(state.last_reported_total) < state.threshold {
            return false;
        }
        state.last_reported_total = total;
        if !inner.rust_types.contains(EntryTypes::MEMORY) {
            return false;
        }
        let entry = MemoryEntry {
            size_bytes: total,
            items: state.records.iter().flatten().cloned().collect(),
        };
        self.deliver(
            inner,
            PerformanceEntry::Memory(entry),
            EntryTypes::MEMORY,
            WorkerAudience::None,
        )
    }

    /// One of a BTS worker's load marks. `loadBackgroundEnd` is the load's
    /// last readiness condition, so it may complete the load of the realm
    /// the worker belongs to.
    pub(crate) fn mark_worker_load(&self, key: WorkerKey, mark: WorkerLoadMark) -> Completion {
        let now = ClockInstant::now();
        let generation = self.generation.load(Ordering::Relaxed);
        let mut guard = self.lock();
        let inner = &mut *guard;
        let marks = inner.worker_loads.entry(key).or_default();
        let slot = match mark {
            WorkerLoadMark::CoreStart => &mut marks.load_core.0,
            WorkerLoadMark::CoreEnd => &mut marks.load_core.1,
            WorkerLoadMark::BackgroundStart => &mut marks.load_background.0,
            WorkerLoadMark::BackgroundEnd => &mut marks.load_background.1,
        };
        slot.get_or_insert(now);
        if mark != WorkerLoadMark::BackgroundEnd {
            return Completion::default();
        }
        if let Some(realm) = inner.bindings.get(&key).copied()
            && let Some(id) = inner.loads.get(&realm).map(|load| load.pipeline)
        {
            self.try_complete(inner, id);
        }
        let before = inner.wakes.len();
        inner.wakes.retain(|worker| *worker != key);
        Completion {
            completed: self.generation.load(Ordering::Relaxed) != generation,
            owed_to_caller: inner.wakes.len() != before,
        }
    }

    /// `connectBackground`: `key` is the BTS of `opener`'s realm.
    pub(crate) fn bind_worker(&self, key: WorkerKey, opener: OpenerId) {
        self.lock().bindings.insert(key, opener);
    }

    /// A worker ended. A load waiting for it waits no more, and reports
    /// without the BTS's marks.
    pub(crate) fn forget_worker(&self, key: WorkerKey) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        inner.worker_types.remove(&key);
        inner.owed_workers.remove(&key);
        inner.worker_loads.remove(&key);
        inner.wakes.retain(|worker| *worker != key);
        if let Some(realm) = inner.bindings.remove(&key)
            && let Some(id) = inner.loads.get(&realm).map(|load| load.pipeline)
        {
            self.try_complete(inner, id);
        }
        self.update_observed(inner);
    }

    /// Whether the worker `key` has a `lynx.performance` observer — `None`
    /// for none — and, when it has, the types its observers opt into
    /// beyond its realm's pipeline entries ([`EntryTypes::PAINT`] and
    /// [`EntryTypes::RESOURCE`]). A worker that stops observing drops the delivered
    /// entries it had not taken.
    pub(crate) fn observe_worker(&self, key: WorkerKey, asked: Option<EntryTypes>) {
        let mut inner = self.lock();
        if let Some(types) = asked {
            inner.worker_types.insert(key, types);
        } else {
            inner.worker_types.remove(&key);
            inner.owed_workers.remove(&key);
        }
        self.update_observed(&inner);
    }

    /// Whether a `fetchResource` is timed: the Rust embedder observes
    /// [`EntryTypes::RESOURCE`] or a BTS observer asked for `resource`. One
    /// relaxed load, which is all a fetch reads while nobody does.
    pub(crate) fn times_resources(&self) -> bool {
        self.resource_observed.load(Ordering::Relaxed)
    }

    /// One timed fetch completed, made by `opener`'s realm: its
    /// `resource.lazyBundle` entry, delivered to the Rust embedder and to
    /// the BTS of the realm that fetched — the worker itself, or the BTS
    /// its main-thread realm connected ([`Self::background_of`]) — as
    /// native sends it to every audience (`kEventTypeAll`,
    /// `lynx/core/resource/lazy_bundle/lazy_bundle_loader.cc:169`).
    /// Answers [`Self::deliver`]'s answer.
    pub(crate) fn note_lazy_bundle(&self, opener: Opener, entry: LazyBundleEntry) -> bool {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let background = match opener {
            Opener::Worker(key) => Some(key),
            Opener::Realm(realm) => Self::background_of(inner, realm),
        };
        self.deliver(
            inner,
            PerformanceEntry::LazyBundle(entry),
            EntryTypes::RESOURCE,
            background.map_or(WorkerAudience::None, WorkerAudience::One),
        )
    }

    /// Every `<frame>` card's load entry owed to its realm as `loadmetrics`
    /// since the last call, in completion order, assembled on the caller's
    /// thread — the embedder's, in `pump` — from the record its completion
    /// kept.
    pub(crate) fn take_frame_loads(&self) -> Vec<(OpenerId, LoadBundleTiming)> {
        let taken: Vec<(OpenerId, Taken)> = {
            let mut guard = self.lock();
            let inner = &mut *guard;
            if inner.frame_loads_owed.is_empty() {
                return Vec::new();
            }
            let owed = std::mem::take(&mut inner.frame_loads_owed);
            owed.into_iter()
                .filter_map(|card| {
                    let load = inner.loads.get(&card)?;
                    let record = load.completed.clone()?;
                    let snapshot = LoadSnapshot {
                        url: Arc::clone(&load.url),
                        create_lynx: load.create_lynx,
                        vm_execute: load.vm_execute,
                        page: false,
                    };
                    Some((
                        card,
                        Taken {
                            record,
                            load: Some(snapshot),
                            extra: inner.extra,
                            first_paint: inner.first_paint,
                        },
                    ))
                })
                .collect()
        };
        taken
            .into_iter()
            .filter_map(|(card, taken)| {
                taken
                    .assemble(EntryTypes::LOAD_BUNDLE, &self.origin)
                    .into_iter()
                    .find_map(|entry| match entry {
                        PerformanceEntry::LoadBundle(load) => Some((card, *load)),
                        _ => None,
                    })
            })
            .collect()
    }

    /// What the Rust embedder subscribes to.
    pub(crate) fn observe_rust(&self, types: EntryTypes) {
        let mut inner = self.lock();
        inner.rust_types = types;
        self.update_observed(&inner);
    }

    /// The host's container marks, each kept once. A load already reported
    /// is reported again, to every audience it has now, with the metrics
    /// the new marks fill. Answers whether anything was reported.
    pub(crate) fn set_extra_timing(&self, extra: &ExtraTiming) -> bool {
        let generation = self.generation.load(Ordering::Relaxed);
        let mut guard = self.lock();
        let inner = &mut *guard;
        if !inner.extra.merge_unset(extra) {
            return false;
        }
        let loads: Vec<Record> = inner
            .loads
            .values()
            .filter_map(|load| load.completed.clone())
            .collect();
        for mut record in loads {
            record.load_only = true;
            record.done = false;
            record.audience = 0;
            let id = self.generate_id();
            self.insert(inner, id, record);
            self.complete(inner, id, None);
        }
        self.generation.load(Ordering::Relaxed) != generation
    }

    /// Every completed entry the Rust embedder has not taken, of the types
    /// it subscribes to: the pipeline and paint records in completion
    /// order, assembled on the caller's thread, then the delivered entries
    /// in the order they were made.
    pub(crate) fn take_completed_for_rust(&self) -> Vec<PerformanceEntry> {
        let (taken, owed, types) = {
            let mut inner = self.lock();
            let types = inner.rust_types;
            let taken = Self::take_completed(&mut inner, RUST, None);
            (taken, std::mem::take(&mut inner.owed_rust), types)
        };
        taken
            .iter()
            .flat_map(|completed| completed.assemble(types, &self.origin))
            .chain(owed)
            .collect()
    }

    /// Every completed entry the worker `key` is owed, assembled on its
    /// own thread, then the entries delivered to it.
    pub(crate) fn take_completed_for_worker(&self, key: WorkerKey) -> Vec<PerformanceEntry> {
        let (taken, owed) = {
            let mut inner = self.lock();
            let taken = Self::take_completed(&mut inner, WORKER, Some(key));
            (taken, inner.owed_workers.remove(&key).unwrap_or_default())
        };
        taken
            .iter()
            .flat_map(|completed| completed.assemble(EntryTypes::ALL, &self.origin))
            .chain(owed)
            .collect()
    }

    fn take_completed(inner: &mut Inner, bit: u8, worker: Option<WorkerKey>) -> Vec<Taken> {
        let ids: SmallVec<[u64; 8]> = inner
            .completed
            .iter()
            .copied()
            .filter(|id| {
                inner.records.get(id).is_some_and(|record| {
                    record.audience & bit != 0 && (worker.is_none() || record.worker == worker)
                })
            })
            .collect();
        let mut taken = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(record) = inner.records.get(&id) else {
                continue;
            };
            let load = Self::realm_of(inner, record.opener).and_then(|realm| {
                inner.loads.get(&realm).map(|load| LoadSnapshot {
                    url: Arc::clone(&load.url),
                    create_lynx: load.create_lynx,
                    vm_execute: load.vm_execute,
                    page: realm == OpenerId::PAGE,
                })
            });
            taken.push(Taken {
                record: record.clone(),
                load,
                extra: inner.extra,
                first_paint: inner.first_paint,
            });
            if let Some(record) = inner.records.get_mut(&id) {
                record.audience &= !bit;
                if record.audience == 0 {
                    inner.records.remove(&id);
                }
            }
        }
        let records = &inner.records;
        inner.completed.retain(|id| records.contains_key(id));
        taken
    }

    fn insert(&self, inner: &mut Inner, id: u64, record: Record) {
        if inner.records.len() >= MAX_RECORDS && !inner.records.contains_key(&id) {
            // Lynx's `timing_map_exceeded_size`: the oldest pipeline that
            // never reached a flush goes first, and a load last of all.
            let victim = inner
                .records
                .iter()
                .filter(|(_, record)| record.commit.is_none() && record.kind != Kind::Load)
                .map(|(id, _)| *id)
                .min()
                .or_else(|| inner.records.keys().copied().min());
            if let Some(victim) = victim {
                inner.records.remove(&victim);
                inner.completed.retain(|id| *id != victim);
            }
        }
        inner.records.insert(id, record);
        self.count(inner);
    }

    /// The records as they stand, for the crate's tests.
    #[cfg(test)]
    pub(crate) fn records_for_test(&self) -> Vec<RecordProbe> {
        let inner = self.lock();
        let mut records: Vec<_> = inner
            .records
            .iter()
            .map(|(id, record)| RecordProbe {
                id: *id,
                origin: record.origin,
                load: record.kind == Kind::Load,
                flags: record.flags.iter().map(ToString::to_string).collect(),
                commit_id: record.commit.map(|commit| commit.id),
                timing: record.commit.and_then(|commit| commit.timing),
            })
            .collect();
        records.sort_by_key(|record| record.id);
        records
    }
}

/// One record as [`PipelineRegistry::records_for_test`] reports it.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RecordProbe {
    pub(crate) id: u64,
    pub(crate) origin: PipelineOrigin,
    pub(crate) load: bool,
    pub(crate) flags: Vec<String>,
    pub(crate) commit_id: Option<u64>,
    pub(crate) timing: Option<CommitTiming>,
}

/// What a realm's load contributes to an entry.
struct LoadSnapshot {
    url: Arc<str>,
    create_lynx: Option<(ClockInstant, ClockInstant)>,
    vm_execute: (ClockInstant, Option<ClockInstant>),
    /// The page realm's load, which the view's Paint Timing marks are
    /// about.
    page: bool,
}

/// One completed record and what its entries are assembled from, copied out
/// under the lock so the assembly runs outside it.
struct Taken {
    record: Record,
    load: Option<LoadSnapshot>,
    extra: ExtraTiming,
    first_paint: Option<ClockInstant>,
}

/// Queues `entry`, dropping the oldest beyond [`MAX_RECORDS`].
fn push_bounded(queue: &mut VecDeque<PerformanceEntry>, entry: PerformanceEntry) {
    if queue.len() >= MAX_RECORDS {
        queue.pop_front();
    }
    queue.push_back(entry);
}

fn range(span: (Option<ClockInstant>, Option<ClockInstant>)) -> Option<std::ops::Range<Timestamp>> {
    match span {
        (Some(start), Some(end)) => Some(Timestamp(start)..Timestamp(end)),
        _ => None,
    }
}

/// A metric ending at a present, from a mark that may not have been set.
fn paint_metric(
    name: &'static str,
    start_name: &'static str,
    start: Option<Timestamp>,
    end: Timestamp,
) -> Option<Metric> {
    start.map(|start| Metric {
        name,
        start_name,
        start,
        end_name: "paintEnd",
        end,
    })
}

impl Taken {
    /// The entries of the record that `types` asks for: the load's or the
    /// reload's own, then one `Pipeline` entry per flag.
    fn assemble(&self, types: EntryTypes, origin: &TimeOrigin) -> SmallVec<[PerformanceEntry; 2]> {
        let mut entries = SmallVec::new();
        let record = &self.record;
        if let Kind::Paint(name) = record.kind {
            if types.contains(EntryTypes::PAINT) {
                entries.push(PerformanceEntry::Paint(PaintEntry {
                    name,
                    start: Timestamp(record.start),
                }));
            }
            return entries;
        }
        let (Some(commit), Some(presented)) = (record.commit, record.presented) else {
            return entries;
        };
        let base = Self::pipeline(record, commit, presented);
        let paint_end = base.paint_end;
        let at = Timestamp(commit.at);
        let extra_at = |time: Option<SystemTime>| time.and_then(|time| origin.timestamp_of(time));
        let prepare = extra_at(self.extra.prepare_template_start);
        let open = extra_at(self.extra.open_time);
        let load_start = self.load.as_ref().map(|load| Timestamp(load.vm_execute.0));
        if types.contains(EntryTypes::LOAD_BUNDLE) {
            match record.kind {
                Kind::Load => {
                    if let Some(load) = &self.load {
                        entries.push(PerformanceEntry::LoadBundle(Box::new(self.load_entry(
                            load,
                            base.clone(),
                            at,
                            prepare,
                            open,
                        ))));
                    }
                }
                Kind::Reload => {
                    let start = Timestamp(record.start);
                    entries.push(PerformanceEntry::ReloadBundle(Box::new(
                        ReloadBundleTiming {
                            pipeline: base.clone(),
                            url: self
                                .load
                                .as_ref()
                                .map_or_else(|| Arc::from(""), |load| Arc::clone(&load.url)),
                            reload_bundle: start..record.end.map_or(at, Timestamp),
                            reload_background: None,
                            lynx_fcp: Metric {
                                name: "lynxFcp",
                                start_name: "reloadBundleStart",
                                start,
                                end_name: "paintEnd",
                                end: paint_end,
                            },
                            fcp: None,
                            total_fcp: None,
                        },
                    )));
                }
                Kind::Update | Kind::Paint(_) => {}
            }
        }
        if !record.load_only && types.contains(EntryTypes::PIPELINE) {
            for flag in &record.flags {
                let mut pipeline = base.clone();
                pipeline.identifier = Some(Arc::clone(flag));
                if **flag == *ACTUAL_FMP_FLAG
                    && let Some(start) = load_start
                {
                    pipeline.actual_fmp = Some(ActualFmp {
                        actual_fmp: paint_metric(
                            "actualFmp",
                            "prepareTemplateStart",
                            prepare,
                            paint_end,
                        ),
                        lynx_actual_fmp: Metric {
                            name: "lynxActualFmp",
                            start_name: "loadBundleStart",
                            start,
                            end_name: "paintEnd",
                            end: paint_end,
                        },
                        total_actual_fmp: paint_metric(
                            "totalActualFmp",
                            "openTime",
                            open,
                            paint_end,
                        ),
                    });
                }
                entries.push(PerformanceEntry::Pipeline(Box::new(pipeline)));
            }
        }
        entries
    }

    /// The pipeline every entry of the record carries, with no identifier:
    /// the commit's stages, or empty stages at the flush's end for a flush
    /// that built no frame, and the present's.
    fn pipeline(record: &Record, commit: Commit, presented: PresentedFrame) -> PipelineTiming {
        let at = Timestamp(commit.at);
        let (resolve, layout, build) = commit.timing.map_or_else(
            || (at..at, at..at, at..at),
            |timing| {
                (
                    Timestamp(timing.style_start)..Timestamp(timing.style_end),
                    Timestamp(timing.layout_start)..Timestamp(timing.layout_end),
                    Timestamp(timing.frame_build_start)..Timestamp(timing.frame_build_end),
                )
            },
        );
        let paint_end = Timestamp(presented.presented);
        let compose = Timestamp(presented.compose_start)..Timestamp(presented.compose_end);
        PipelineTiming {
            origin: record.origin,
            identifier: None,
            pipeline_start: Timestamp(record.start),
            pipeline_end: paint_end,
            mts_render: range(record.mts_render),
            resolve,
            layout,
            painting_ui_operation_execute: build,
            layout_ui_operation_execute: compose.clone(),
            paint_end,
            framework_rendering_timing: FrameworkTiming {
                dsl: record.dsl.clone(),
                stage: record.stage.clone(),
                marks: record
                    .marks
                    .iter()
                    .map(|(key, at)| (Arc::clone(key), Timestamp(*at)))
                    .collect(),
            },
            host_platform_timing: PainterTiming { compose },
            actual_fmp: None,
        }
    }

    /// A realm's load entry over its pipeline.
    fn load_entry(
        &self,
        load: &LoadSnapshot,
        pipeline: PipelineTiming,
        at: Timestamp,
        prepare: Option<Timestamp>,
        open: Option<Timestamp>,
    ) -> LoadBundleTiming {
        let record = &self.record;
        let start = Timestamp(load.vm_execute.0);
        // Lynx's `fcp` is the view's first paint (Paint Timing's
        // `first-paint`); a `<frame>` card's load, whose first paint the
        // view's is not, reports its own pipeline's present.
        let first_paint = load
            .page
            .then_some(self.first_paint)
            .flatten()
            .map(Timestamp)
            .filter(|paint| *paint >= start)
            .unwrap_or(pipeline.paint_end);
        let background = record.background.unwrap_or_default();
        LoadBundleTiming {
            url: Arc::clone(&load.url),
            create_lynx: load.create_lynx.map_or(start..start, |(begin, end)| {
                Timestamp(begin)..Timestamp(end)
            }),
            load_bundle: start..record.end.map_or(at, Timestamp),
            parse: None,
            vm_execute: load.vm_execute.1.map(|end| start..Timestamp(end)),
            load_core: range(background.load_core),
            load_background: range(background.load_background),
            extra: self.extra,
            lynx_fcp: Metric {
                name: "lynxFcp",
                start_name: "loadBundleStart",
                start,
                end_name: "paintEnd",
                end: first_paint,
            },
            fcp: paint_metric("fcp", "prepareTemplateStart", prepare, first_paint),
            total_fcp: paint_metric("totalFcp", "openTime", open, first_paint),
            pipeline,
        }
    }
}
