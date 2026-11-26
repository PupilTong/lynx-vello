//! The two monitors over the engine threads' jobs, as Lynx's
//! `LongTaskMonitor` and `JSBlockingMonitor` have them: their
//! configurations, their entries (made whole where they are produced and
//! handed to [`PipelineRegistry::deliver_entry`]), and the tasks a monitor
//! runs on the view's `bobcat-main`, the memory monitor's among them. Both
//! are off unless the embedder turns them on; off, each costs one relaxed
//! atomic load where it would measure and reads no clock.

use std::future::Future;
use std::pin::pin;
use std::time::Duration;

use tokio::sync::watch;

use super::memory::{MemoryConfig, serve_memory_samples};
use super::{PipelineRegistry, Timestamp};
use crate::jobs::{JobKind, JsThreadName};
use crate::link::HostOutbox;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongTaskConfig {
    pub threshold: Duration,
}

impl Default for LongTaskConfig {
    fn default() -> Self {
        Self {
            threshold: Duration::from_millis(50),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsBlockingConfig {
    pub threshold: Duration,
    pub report_interval: Duration,
}

impl Default for JsBlockingConfig {
    fn default() -> Self {
        Self {
            threshold: Duration::from_millis(20),
            report_interval: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongTaskEntry {
    pub start: Timestamp,
    pub end: Timestamp,
    pub thread: JsThreadName,
    pub kind: JobKind,
}

impl LongTaskEntry {
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.end.duration_since(self.start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsBlockingStage {
    LoadBundle,
    Timer,
}

impl JsBlockingStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoadBundle => "loadBundle",
            Self::Timer => "timer",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JsBlockingEntry {
    pub stage: JsBlockingStage,
    pub total_blocking_time: Duration,
    pub total_blocking_count: u32,
    pub total_duration: Duration,
    pub blocking_ratio: f64,
    pub avg_blocking_time: Duration,
    pub time_after_load_bundle: Option<Duration>,
}

impl JsBlockingEntry {
    pub(crate) fn from_totals(
        stage: JsBlockingStage,
        total_blocking_time: Duration,
        total_blocking_count: u32,
        total_duration: Duration,
        time_after_load_bundle: Option<Duration>,
    ) -> Self {
        Self {
            stage,
            total_blocking_time,
            total_blocking_count,
            total_duration,
            blocking_ratio: total_blocking_time.as_secs_f64() / total_duration.as_secs_f64(),
            avg_blocking_time: total_blocking_time
                .checked_div(total_blocking_count)
                .unwrap_or_default(),
            time_after_load_bundle,
        }
    }
}

pub(crate) fn encode_threshold(threshold: Option<Duration>) -> u64 {
    threshold.map_or(0, |threshold| {
        u64::try_from(threshold.as_micros())
            .unwrap_or(u64::MAX)
            .min(u64::MAX - 1)
            + 1
    })
}

pub(crate) const fn decode_threshold(raw: u64) -> Option<Duration> {
    if raw == 0 {
        None
    } else {
        Some(Duration::from_micros(raw - 1))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum MonitorTask {
    JsBlockingReports { epoch: u64, interval: Duration },
    MemorySamples { epoch: u64, config: MemoryConfig },
}

impl MonitorTask {
    pub(crate) async fn serve(self, host: HostOutbox, mut sample_memory: impl FnMut(u64)) {
        match self {
            Self::JsBlockingReports { epoch, interval } => {
                serve_js_blocking_reports(host, epoch, interval).await;
            }
            Self::MemorySamples { epoch, config } => {
                serve_memory_samples(host, epoch, config, || sample_memory(epoch)).await;
            }
        }
    }
}

pub(crate) struct MonitorEpoch {
    host: HostOutbox,
    epochs: watch::Receiver<u64>,
    epoch: u64,
    current: fn(&PipelineRegistry, u64) -> bool,
}

impl MonitorEpoch {
    pub(crate) fn new(
        host: HostOutbox,
        epoch: u64,
        current: fn(&PipelineRegistry, u64) -> bool,
    ) -> Self {
        let epochs = host.registry().monitor_epochs();
        Self {
            host,
            epochs,
            epoch,
            current,
        }
    }

    pub(crate) fn is_current(&self) -> bool {
        (self.current)(self.host.registry(), self.epoch)
    }

    pub(crate) async fn run<F: Future>(&mut self, until: F) -> Option<F::Output> {
        let mut until = pin!(until);
        loop {
            tokio::select! {
                biased;
                () = self.host.token().cancelled() => return None,
                changed = self.epochs.changed() => {
                    if changed.is_err() || !self.is_current() {
                        return None;
                    }
                }
                output = &mut until => return Some(output),
            }
        }
    }

    pub(crate) async fn changed(&mut self) -> bool {
        tokio::select! {
            biased;
            () = self.host.token().cancelled() => false,
            changed = self.epochs.changed() => changed.is_ok(),
        }
    }
}

async fn serve_js_blocking_reports(host: HostOutbox, epoch: u64, interval: Duration) {
    let mut task = MonitorEpoch::new(
        host.clone(),
        epoch,
        PipelineRegistry::js_blocking_is_current,
    );
    let anchor = loop {
        if !task.is_current() {
            return;
        }
        if let Some(anchor) = host.registry().js_blocking_anchor(epoch) {
            break anchor;
        }
        if !task.changed().await {
            return;
        }
    };
    let mut deadline = anchor;
    let mut index: u32 = 1;
    loop {
        let Some(next) = deadline.checked_add(interval.saturating_mul(index)) else {
            return;
        };
        deadline = next;
        if task
            .run(crate::clock::sleep_until(deadline))
            .await
            .is_none()
        {
            return;
        }
        if host.registry().report_js_blocking(epoch) {
            host.wake();
        }
        index = index.saturating_add(1);
    }
}
