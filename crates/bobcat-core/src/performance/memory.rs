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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryConfig {
    pub report_interval: Duration,
    pub acquisition_delay: Duration,
    pub change_threshold: u64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            report_interval: Duration::from_mins(20),
            acquisition_delay: Duration::from_secs(2),
            change_threshold: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryCategory {
    MainThreadScriptingEngine,
    BackgroundThreadScriptingEngine,
    LynxTasmElement,
}

impl MemoryCategory {
    pub(crate) const ALL: [Self; 3] = [
        Self::MainThreadScriptingEngine,
        Self::BackgroundThreadScriptingEngine,
        Self::LynxTasmElement,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MainThreadScriptingEngine => "mainThreadScriptingEngine",
            Self::BackgroundThreadScriptingEngine => "backgroundThreadScriptingEngine",
            Self::LynxTasmElement => "lynxTasmElement",
        }
    }

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryItem {
    pub category: MemoryCategory,
    pub size_bytes: u64,
    pub instance_count: u32,
    pub detail: Vec<(Arc<str>, Arc<str>)>,
}

impl MemoryItem {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryEntry {
    pub size_bytes: u64,
    pub items: Vec<MemoryItem>,
}

impl MemoryEntry {
    #[must_use]
    pub fn item(&self, category: MemoryCategory) -> Option<&MemoryItem> {
        self.items.iter().find(|item| item.category == category)
    }
}

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
