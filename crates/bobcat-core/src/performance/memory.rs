//! The memory monitor, as Lynx's `MemoryMonitor` has it: its configuration,
//! the three categories it reads, the `memory` entry it reports, and the
//! view's task that samples on the monitor's interval.
//!
//! Off unless the embedder turns it on
//! ([`crate::LynxView::set_memory_monitor`]), as Lynx's
//! `enable_memory_monitor` is. Off, nothing is read anywhere: no clock, no
//! heap walk, no footprint, no task.
//!
//! On, each category is read by the thread that holds what it measures and
//! replaces that category's last reading; each reading reports a `memory`
//! entry with every category's latest reading when the total moved by at
//! least the configured threshold, the way Lynx's `UpdateMemoryUsage` and
//! `ReportMemory` do. The entry is the Rust embedder's alone, as Lynx sends
//! it to the platform alone.

use std::sync::Arc;
use std::time::Duration;

use quickjs_rust_bridge::MemoryUsage;
use terminus::link::MemoryFootprint;

use super::PipelineRegistry;
use super::monitors::MonitorEpoch;
use crate::clock::ClockInstant;
use crate::link::HostOutbox;

/// The memory monitor's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryConfig {
    /// How often the monitor samples once it has started.
    pub report_interval: Duration,
    /// How long after the monitor is turned on the first sample is taken.
    pub acquisition_delay: Duration,
    /// The smallest change of the total, in bytes, that is reported: a
    /// reading that moves the total by less since the last report is kept
    /// and not reported. 0 reports every reading that changes a category.
    pub change_threshold: u64,
}

impl Default for MemoryConfig {
    /// Lynx's `memory_report_interval_sec` (20 minutes),
    /// `memory_acquisition_delay_second` (2 s) and
    /// `memory_change_threshold_mb` (0) defaults (`lynx_env.cc`).
    fn default() -> Self {
        Self {
            report_interval: Duration::from_mins(20),
            acquisition_delay: Duration::from_secs(2),
            change_threshold: 0,
        }
    }
}

/// What a [`MemoryItem`] measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryCategory {
    /// The `QuickJS` runtime the view's main-thread realm runs in, read on
    /// `bobcat-main`. Lynx's category is its own main-thread engine's heap,
    /// one per view or group; this engine's runtime is shared by every realm
    /// on the thread — every view of the group, and their `ShadowRealm`
    /// children — so the reading covers all of them, and its detail says so
    /// (`sharedRuntime: "thread"`).
    MainThreadScriptingEngine,
    /// The `QuickJS` runtime of `bobcat-workers`, read by the view's BTS
    /// worker. Shared the same way: every worker realm of the group runs on
    /// it, so the reading covers all of them.
    BackgroundThreadScriptingEngine,
    /// The document's elements: their count times the size of one arena
    /// node, as Lynx's `ReportElementMemoryInfo` sums `sizeof` of each
    /// element.
    LynxTasmElement,
}

impl MemoryCategory {
    /// Every category, in the order an entry lists them.
    pub(crate) const ALL: [Self; 3] = [
        Self::MainThreadScriptingEngine,
        Self::BackgroundThreadScriptingEngine,
        Self::LynxTasmElement,
    ];

    /// The category's name: Lynx's `kCategoryMTSEngine`,
    /// `kCategoryBTSEngine` and `kCategoryTasmElement`
    /// (`memory_record.h`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MainThreadScriptingEngine => "mainThreadScriptingEngine",
            Self::BackgroundThreadScriptingEngine => "backgroundThreadScriptingEngine",
            Self::LynxTasmElement => "lynxTasmElement",
        }
    }

    /// The category's position in [`Self::ALL`], which lists the variants
    /// in declaration order.
    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

/// One category's reading: Lynx's `MemoryRecord`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryItem {
    pub category: MemoryCategory,
    pub size_bytes: u64,
    /// How many instances the reading covers.
    pub instance_count: u32,
    /// What else the reading says, as Lynx's string map.
    pub detail: Vec<(Arc<str>, Arc<str>)>,
}

impl MemoryItem {
    /// A scripting engine's reading: `malloc_size` as the size, and the
    /// runtime's other counters as the detail.
    pub(crate) fn scripting_engine(category: MemoryCategory, usage: &MemoryUsage) -> Self {
        let counters = [
            ("memory_used_size", usage.memory_used_size),
            ("malloc_count", usage.malloc_count),
            ("obj_count", usage.obj_count),
            ("str_count", usage.str_count),
            ("atom_count", usage.atom_count),
            ("shape_count", usage.shape_count),
            ("js_func_count", usage.js_func_count),
            ("array_count", usage.array_count),
        ];
        let mut detail: Vec<(Arc<str>, Arc<str>)> = counters
            .into_iter()
            .map(|(key, value)| (Arc::from(key), Arc::from(value.to_string())))
            .collect();
        detail.push((Arc::from("sharedRuntime"), Arc::from("thread")));
        Self {
            category,
            size_bytes: u64::try_from(usage.malloc_size).unwrap_or(0),
            instance_count: 1,
            detail,
        }
    }

    /// The document's reading: Lynx's `ReportElementMemoryInfo` detail.
    pub(crate) fn elements(footprint: &MemoryFootprint) -> Self {
        Self {
            category: MemoryCategory::LynxTasmElement,
            size_bytes: u64::try_from(footprint.element_bytes()).unwrap_or(u64::MAX),
            instance_count: u32::try_from(footprint.live_elements).unwrap_or(u32::MAX),
            detail: vec![
                (
                    Arc::from("elementCount"),
                    Arc::from(footprint.live_elements.to_string()),
                ),
                (
                    Arc::from("singleElementSizeBytes"),
                    Arc::from(footprint.node_stride_bytes.to_string()),
                ),
            ],
        }
    }
}

/// Lynx's `memory` entry: every category's latest reading, and their sum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryEntry {
    /// The sum of the items' sizes.
    pub size_bytes: u64,
    /// One reading per category read so far, in [`MemoryCategory`] order.
    pub items: Vec<MemoryItem>,
}

impl MemoryEntry {
    /// The reading of `category`, if it has been read.
    #[must_use]
    pub fn item(&self, category: MemoryCategory) -> Option<&MemoryItem> {
        self.items.iter().find(|item| item.category == category)
    }
}

/// The memory monitor's samples, as a task of the view on `bobcat-main`
/// ([`super::MonitorTask::MemorySamples`]), until the view ends or the
/// monitor's configuration is no longer the one of `epoch`.
///
/// The first sample is taken `acquisition_delay` after the start, and one
/// more every `report_interval` after that; a deadline the task woke past is
/// followed one interval after the wake rather than at once. Each sample is
/// `sample`, which queues the page's job that reads the main thread's
/// categories and asks the view's BTS for its own.
pub(crate) async fn serve_memory_samples(
    host: HostOutbox,
    epoch: u64,
    config: MemoryConfig,
    mut sample: impl FnMut(),
) {
    let mut task = MonitorEpoch::new(host, epoch, PipelineRegistry::memory_is_current);
    if !task.is_current() {
        return;
    }
    let Some(mut deadline) = ClockInstant::now().checked_add(config.acquisition_delay) else {
        return;
    };
    loop {
        if task
            .run(crate::clock::sleep_until(deadline))
            .await
            .is_none()
        {
            return;
        }
        sample();
        let now = ClockInstant::now();
        let Some(next) = deadline
            .checked_add(config.report_interval)
            .filter(|next| *next > now)
            .or_else(|| now.checked_add(config.report_interval))
        else {
            return;
        };
        deadline = next;
    }
}
